//  SwiftAgentExecutor.swift
//  OxcerLauncher
//
//  Swift-side step-driven agent executor.
//
//  # Architecture
//
//  The Rust FFI exposes two complementary agent APIs:
//
//  1. runAgentTask(payload:) – blocking loop using FfiLlmExecutor.
//     Handles LlmGenerate intents (phi-3-mini) entirely inside Rust.
//     FS/Shell intents return a descriptive error (not supported via this path).
//
//  2. ffiAgentStep(step:) – one synchronous orchestrator step.
//     Swift drives the loop and dispatches each ToolCallIntent:
//       - llm_generate  → generateText() (Rust phi-3-mini)
//       - fs_list_dir   → FileManager (read-only, safe in Xcode)
//       - fs_read_file  → FileManager (read-only, safe in Xcode)
//       - fs_write_file / fs_delete / fs_rename / fs_move
//                       → error (mutating FS requires explicit user approval)
//       - shell_run     → error (security restriction)
//
//  SwiftAgentExecutor uses path #2, giving Swift full visibility into every
//  tool call before it executes. This is the extensibility point for adding
//  native FS write / approval UI / progress streaming in future sprints.
//
//  # phi-3-mini prompt format
//  <|system|>{hint}<|end|>\n<|user|>{task}<|end|>\n<|assistant|>

import Foundation

// MARK: - Tool intent deserialization helpers (private to this file)

private struct ToolIntentKind: Decodable {
    let kind: String
}

private struct LlmGenerateIntent: Decodable {
    let task: String
    let system_hint: String?
}

private struct FsListDirIntent: Decodable {
    let workspace_root: String
    let rel_path: String
}

private struct FsReadFileIntent: Decodable {
    let workspace_root: String
    let rel_path: String
}

// MARK: - JSON encoding helpers (private to this file)

/// Encodes a `{kind: "ok", payload: {key: value}}` step result.
private func stepResultOk(_ payload: [String: String]) -> String {
    var inner = "{"
    inner += payload.map { k, v in
        "\"\(jsonEscape(k))\": \"\(jsonEscape(v))\""
    }.joined(separator: ", ")
    inner += "}"
    return "{\"kind\": \"ok\", \"payload\": \(inner)}"
}

/// Encodes a `{kind: "err", message: "..."}` step result.
private func stepResultErr(_ message: String) -> String {
    return "{\"kind\": \"err\", \"message\": \"\(jsonEscape(message))\"}"
}

/// Minimal JSON string escaping (backslash + double-quote).
private func jsonEscape(_ s: String) -> String {
    s.replacingOccurrences(of: "\\", with: "\\\\")
     .replacingOccurrences(of: "\"", with: "\\\"")
     .replacingOccurrences(of: "\n", with: "\\n")
     .replacingOccurrences(of: "\r", with: "\\r")
     .replacingOccurrences(of: "\t", with: "\\t")
}

// MARK: - SwiftAgentExecutor

/// Drives the orchestrator step loop from Swift.
///
/// Usage:
/// ```swift
/// let response = try await SwiftAgentExecutor().runTask(payload: payload)
/// ```
struct SwiftAgentExecutor {

    /// Maximum orchestrator iterations before declaring a loop.
    private let maxIterations: Int

    init(maxIterations: Int = 20) {
        self.maxIterations = maxIterations
    }

    // MARK: - Public entry point

    /// Run an agent task to completion, driving the step loop from Swift.
    ///
    /// - Returns: An `AgentResponse` with `ok: true` and an `answer` on success,
    ///            or `ok: false` and an `error` on failure.
    func runTask(payload: AgentRequestPayload) async throws -> AgentResponse {
        let task = payload.taskDescription.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !task.isEmpty else {
            return AgentResponse(ok: false, answer: nil, error: "task_description is required")
        }

        var sessionJson = ""
        var lastResultJson: String? = nil

        for iteration in 0..<maxIterations {
            let step = FfiAgentStep(
                sessionJson: sessionJson,
                taskDescription: task,
                lastResultJson: lastResultJson,
                configJson: nil          // use orchestrator defaults
            )

            // ffiAgentStep is synchronous/blocking; run off the main actor
            let stepResult = await Task.detached(priority: .userInitiated) {
                ffiAgentStep(step: step)
            }.value

            sessionJson = stepResult.sessionJson

            switch stepResult.status {

            case "complete":
                let answer = stepResult.finalAnswer ?? "(no answer)"
                print("[SwiftAgentExecutor] ✓ Complete after \(iteration + 1) iteration(s). Answer length: \(answer.count)")
                return AgentResponse(ok: true, answer: answer, error: nil)

            case "error":
                let msg = stepResult.errorMessage ?? "Unknown orchestrator error"
                print("[SwiftAgentExecutor] ✗ Error: \(msg)")
                return AgentResponse(ok: false, answer: nil, error: msg)

            case "awaiting_approval":
                // Development mode: auto-approve.
                // TODO: surface approval UI before production release.
                let approvedPayload = "{\"approved\": true}"
                lastResultJson = "{\"kind\": \"ok\", \"payload\": \(approvedPayload)}"
                print("[SwiftAgentExecutor] ⚠ Approval requested (request_id: \(stepResult.requestId ?? "?")). Auto-approving in dev mode.")

            case "need_tool":
                guard let intentJson = stepResult.intentJson else {
                    return AgentResponse(ok: false, answer: nil, error: "Orchestrator returned need_tool with missing intent_json")
                }
                let resultJson = await executeTool(intentJson: intentJson)
                lastResultJson = resultJson

            default:
                return AgentResponse(ok: false, answer: nil, error: "Unexpected orchestrator status: \(stepResult.status)")
            }
        }

        return AgentResponse(
            ok: false,
            answer: nil,
            error: "Agent loop exceeded \(maxIterations) iterations — possible infinite loop detected."
        )
    }

    // MARK: - Tool dispatch

    private func executeTool(intentJson: String) async -> String {
        guard let data = intentJson.data(using: .utf8),
              let kind = try? JSONDecoder().decode(ToolIntentKind.self, from: data) else {
            return stepResultErr("Malformed intent JSON from orchestrator")
        }

        print("[SwiftAgentExecutor] → executing tool: \(kind.kind)")

        switch kind.kind {
        case "llm_generate":
            return await executeLlmGenerate(data: data)

        case "fs_list_dir":
            return executeFsListDir(data: data)

        case "fs_read_file":
            return executeFsReadFile(data: data)

        case "fs_write_file", "fs_delete", "fs_rename", "fs_move":
            // Mutating FS operations require explicit user approval.
            // Wire an approval UI here in a future sprint.
            return stepResultErr(
                "Mutating FS tool '\(kind.kind)' requires explicit user approval. " +
                "Approval UI is not yet implemented in OxcerLauncher."
            )

        case "shell_run":
            return stepResultErr(
                "ShellRun is blocked in the Swift executor for security. " +
                "Use the Tauri backend for shell operations."
            )

        default:
            return stepResultErr("Unknown tool kind: '\(kind.kind)'")
        }
    }

    // MARK: - LlmGenerate

    private func executeLlmGenerate(data: Data) async -> String {
        guard let intent = try? JSONDecoder().decode(LlmGenerateIntent.self, from: data) else {
            return stepResultErr("Failed to decode LlmGenerateIntent")
        }

        let prompt = buildPhi3Prompt(task: intent.task, systemHint: intent.system_hint)
        print("[SwiftAgentExecutor]   phi-3-mini prompt length: \(prompt.count) chars")

        do {
            // generateText() is async and runs on Rust's blocking thread pool.
            let generated = try await OxcerLauncher.generateText(prompt: prompt)
            print("[SwiftAgentExecutor]   phi-3-mini response length: \(generated.count) chars")
            return stepResultOk(["text": generated])
        } catch {
            return stepResultErr("phi-3-mini generation failed: \(error.localizedDescription)")
        }
    }

    /// Builds a phi-3-mini instruct prompt.
    ///   <|system|>{hint}<|end|>\n<|user|>{task}<|end|>\n<|assistant|>
    private func buildPhi3Prompt(task: String, systemHint: String?) -> String {
        var prompt = ""
        if let hint = systemHint, !hint.isEmpty {
            prompt += "<|system|>\(hint)<|end|>\n"
        }
        prompt += "<|user|>\(task)<|end|>\n<|assistant|>"
        return prompt
    }

    // MARK: - FS read-only tools

    private func executeFsListDir(data: Data) -> String {
        guard let intent = try? JSONDecoder().decode(FsListDirIntent.self, from: data) else {
            return stepResultErr("Failed to decode FsListDirIntent")
        }
        let path = (intent.workspace_root as NSString).appendingPathComponent(intent.rel_path)
        do {
            let items = try FileManager.default.contentsOfDirectory(atPath: path)
            let listing = items.sorted().joined(separator: "\n")
            return stepResultOk(["text": listing.isEmpty ? "(empty directory)" : listing])
        } catch {
            return stepResultErr("fs_list_dir failed at '\(path)': \(error.localizedDescription)")
        }
    }

    private func executeFsReadFile(data: Data) -> String {
        guard let intent = try? JSONDecoder().decode(FsReadFileIntent.self, from: data) else {
            return stepResultErr("Failed to decode FsReadFileIntent")
        }
        let path = (intent.workspace_root as NSString).appendingPathComponent(intent.rel_path)
        do {
            let content = try String(contentsOfFile: path, encoding: .utf8)
            return stepResultOk(["text": content])
        } catch {
            return stepResultErr("fs_read_file failed at '\(path)': \(error.localizedDescription)")
        }
    }
}
