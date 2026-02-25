//  AgentRunner.swift
//  OxcerLauncher
//
//  Self-contained step loop over ffi_agent_step + SwiftAgentExecutor.
//  AppViewModel calls AgentRunner.run(env:) and only handles the final String / error.
//
//  Extracting this from AppViewModel keeps the view model focused on UI state
//  and makes the loop logic independently testable without a live backend.

import Foundation
import OSLog

private let agentLogger = Logger(subsystem: "com.oxcer.launcher", category: "AgentRunner")

// MARK: - AgentEnvironment

/// Context bundle for one agent request.
///
/// Replaces the 6-parameter agentStep call-site with a single value type that
/// can be constructed once and passed to AgentRunner, mock backends, and tests
/// without positional-argument mistakes.
struct AgentEnvironment {
    let taskDescription: String
    let workspaceId: String?
    let workspaceRoot: String?
    let appConfigDir: String?
}

// MARK: - AgentRunnerError

/// Structural failures that terminate the step loop.
/// Distinct from FFI errors (OxcerFFIError) which propagate via `throws` from `backend.agentStep`.
enum AgentRunnerError: LocalizedError {
    /// Rust returned "need_tool" but the `intent` field was nil — indicates a Rust bug.
    case missingIntent
    /// The orchestrator returned a status string that this version of Swift doesn't recognise.
    /// Forward-compatibility: add the new case to the switch in AgentRunner.run(env:) when Rust adds one.
    case unknownStatus(String)
    /// The agent did not reach "complete" within the allowed step budget.
    case stepLimitExceeded(Int)

    var errorDescription: String? {
        switch self {
        case .missingIntent:
            return "Agent returned need_tool without an intent payload (Rust bug)"
        case .unknownStatus(let s):
            return "Unknown agent step status: '\(s)'. Update AgentRunner to handle it."
        case .stepLimitExceeded(let n):
            return "Agent did not complete within \(n) steps"
        }
    }
}

// MARK: - Tool kinds requiring approval

/// Tool intent kinds that require explicit user approval before the executor is invoked.
/// `llm_generate` is excluded — it is pure computation with no filesystem or shell access.
private let approvalRequiredKinds: Set<String> = [
    "fs_list_dir", "fs_read_file", "fs_write_file",
    "fs_delete", "fs_rename", "fs_move", "shell_run",
]

// MARK: - AgentRunner

/// Drives the ffi_agent_step loop, delegating tool execution to SwiftAgentExecutor.
///
/// **Call contract:**
///   1. First call: `sessionJson = nil`, `lastResult = nil`.
///   2. On "need_tool": if the intent kind is in `approvalRequiredKinds`, `onApprovalNeeded`
///      is called first. On approval the executor runs; on denial an error result is fed back.
///   3. On "complete": `run(env:)` returns `finalAnswer`.
///   4. On "awaiting_approval" (Rust-driven, e.g. security policy engine): `onApprovalNeeded`
///      is called and the user's decision is fed back as `FfiStepResult`.
///
/// **Thread safety:** `backend.agentStep` is synchronous on the caller's actor.
/// `executor.execute(intent:)` is `async`. `onApprovalNeeded` is `async` and suspends
/// the loop (freeing the main actor for UI events) until the user responds.
struct AgentRunner {
    let backend: OxcerBackend
    let executor: SwiftAgentExecutor
    var maxSteps: Int = 20

    /// Called whenever the agent transitions between phases (thinking / executing a tool / idle).
    /// The closure is invoked on the caller's actor; AppViewModel hops to @MainActor before
    /// mutating session state.
    var onPhaseChanged: ((AgentPhase) -> Void)?

    /// Backend configuration driving timeout, maxSteps, and model parameters.
    /// Defaults to `ModelBackendConfig.current()` so callers that don't set it explicitly
    /// still get the correct per-backend values.
    var config: ModelBackendConfig = .current()

    /// Called when a tool intent requires user approval before execution.
    ///
    /// Receives `(requestId, humanReadableSummary)` and returns `true` to approve or
    /// `false` to deny.  When `nil`, all requests are auto-approved — the original
    /// behaviour used by tests and SwiftUI previews.
    var onApprovalNeeded: ((String, String) async -> Bool)?

    /// Run the agent to completion and return the final answer string.
    ///
    /// Throws `AgentRunnerError` for loop-structural failures.
    /// Re-throws `OxcerFFIError` / `OxcerError` from `backend.agentStep`.
    func run(env: AgentEnvironment) async throws -> String {
        var sessionJson: String? = nil
        var sessionId: String? = nil
        var lastResult: FfiStepResult? = nil
        let runStart = Date()

        for step in 1...maxSteps {
            // Honour Swift structured concurrency cancellation (e.g. user pressed Stop).
            // Checked at the top of every iteration so that even synchronous FFI steps
            // (backend.agentStep has no await) stop at a known, clean boundary.
            try Task.checkCancellation()

            let sid = sessionId ?? "unknown"
            agentLogger.debug("step \(step, privacy: .public)/\(self.maxSteps, privacy: .public) sid=\(sid, privacy: .public) task=\(env.taskDescription.prefix(60), privacy: .public)")

            // Signal that the model is generating the next step.
            setPhase(.thinking)

            let outcome = try backend.agentStep(
                env: env,
                sessionJson: sessionJson,
                lastResult: lastResult
            )
            sessionJson = outcome.session.sessionJson
            if sessionId == nil { sessionId = extractSessionId(from: sessionJson) }
            let sid2 = sessionId ?? "unknown"

            switch outcome.status {
            case "need_tool":
                guard let intent = outcome.intent else {
                    throw AgentRunnerError.missingIntent
                }
                agentLogger.info("need_tool kind=\(intent.kind, privacy: .public) sid=\(sid2, privacy: .public)")

                // Gate: request user approval before any filesystem or shell tool executes.
                if approvalRequiredKinds.contains(intent.kind) {
                    let reqId = "step-\(step)"
                    let summary = approvalSummary(for: intent)
                    let approved = await requestApproval(requestId: reqId, summary: summary)
                    if !approved {
                        agentLogger.info("denied intent=\(intent.kind, privacy: .public) step=\(step, privacy: .public) sid=\(sid2, privacy: .public)")
                        // Return an error result so the orchestrator can surface it.
                        lastResult = FfiStepResult(
                            ok: false,
                            payloadJson: nil,
                            error: "User denied: \(intent.kind.replacingOccurrences(of: "_", with: " "))"
                        )
                        agentLogger.debug("lastResult denied ok=false sid=\(sid2, privacy: .public)")
                        continue
                    }
                }

                setPhase(.executingTool(name: intent.kind))
                lastResult = await executor.execute(intent: intent, sessionId: sid2)
                if let lr = lastResult {
                    agentLogger.debug("lastResult ok=\(lr.ok, privacy: .public) payload=\(lr.payloadJson?.count ?? 0, privacy: .public)ch sid=\(sid2, privacy: .public)")
                }
                // Tool done — model will receive the result and think next iteration.
                setPhase(.thinking)

            case "complete":
                let rawAnswer = outcome.finalAnswer
                let answer = rawAnswer ?? ""
                let totalElapsed = -runStart.timeIntervalSinceNow
                agentLogger.info("complete steps=\(step, privacy: .public) elapsed=\(String(format: "%.3f", totalElapsed), privacy: .public)s sid=\(sid2, privacy: .public)")
                return answer

            case "awaiting_approval":
                // Rust-driven approval (triggered by the security policy engine for destructive ops).
                // In the current implementation the orchestrator reaches this branch when it receives
                // StepResult::ApprovalPending from an executor; the Swift executor never emits that,
                // so this branch is a forward-compatibility path for when the policy engine is wired.
                let reqId = outcome.approvalRequestId ?? "approval-\(step)"
                let summary = "Allow Oxcer to perform a requested action?"
                let approved = await requestApproval(requestId: reqId, summary: summary)
                agentLogger.info("awaiting_approval reqId=\(reqId, privacy: .public) approved=\(approved, privacy: .public) sid=\(sid2, privacy: .public)")
                lastResult = FfiStepResult(
                    ok: approved,
                    payloadJson: approved ? "{}" : nil,
                    error: approved ? nil : "User denied filesystem access"
                )

            default:
                throw AgentRunnerError.unknownStatus(outcome.status)
            }
        }

        let sid = sessionId ?? "unknown"
        agentLogger.error("step_limit_exceeded maxSteps=\(self.maxSteps, privacy: .public) sid=\(sid, privacy: .public)")
        throw AgentRunnerError.stepLimitExceeded(maxSteps)
    }

    // MARK: - Phase helper

    /// Forwards a phase change to `onPhaseChanged` if wired.
    /// No-op when the closure is nil (tests, previews, or callers that don't care about phase).
    private func setPhase(_ phase: AgentPhase) {
        onPhaseChanged?(phase)
    }

    // MARK: - Approval helpers

    /// Calls `onApprovalNeeded` if wired; otherwise auto-approves (tests/previews).
    private func requestApproval(requestId: String, summary: String) async -> Bool {
        guard let handler = onApprovalNeeded else {
            agentLogger.debug("auto_approve reqId=\(requestId, privacy: .public)")
            return true
        }
        return await handler(requestId, summary)
    }

    /// Extracts the Rust session_id from the opaque sessionJson blob.
    /// Only reads — never modifies — the JSON (contract: Swift must pass it back unchanged).
    private func extractSessionId(from json: String?) -> String? {
        guard let json,
              let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let sid = obj["session_id"] as? String
        else { return nil }
        return sid
    }

    /// Builds a human-readable approval prompt from a tool intent.
    private func approvalSummary(for intent: FfiToolIntent) -> String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path

        if let info = decodePathInfo(from: intent.intentJson) {
            // Reconstruct the full path that will be accessed.
            let full: String
            if info.relPath == "." || info.relPath.isEmpty {
                full = info.workspaceRoot
            } else {
                full = "\(info.workspaceRoot)/\(info.relPath)"
            }
            // Replace home dir prefix with "~" for readability.
            let display = full.hasPrefix(home)
                ? "~" + full.dropFirst(home.count)
                : full

            switch intent.kind {
            case "fs_list_dir":   return "Allow Oxcer to list files under: \(display)"
            case "fs_read_file":  return "Allow Oxcer to read: \(display)"
            case "fs_write_file": return "Allow Oxcer to write: \(display)"
            case "fs_delete":     return "Allow Oxcer to delete: \(display)"
            case "fs_rename":     return "Allow Oxcer to rename: \(display)"
            case "fs_move":       return "Allow Oxcer to move files from: \(display)"
            default: break
            }
        }

        if intent.kind == "shell_run", let cmd = decodeShellCmd(from: intent.intentJson) {
            let preview = cmd.count > 60 ? String(cmd.prefix(60)) + "…" : cmd
            return "Allow Oxcer to run: \(preview)"
        }

        let kindDisplay = intent.kind.replacingOccurrences(of: "_", with: " ")
        return "Allow Oxcer to perform: \(kindDisplay)?"
    }

    // MARK: - JSON helpers for approval summary

    private struct FsPathInfo: Decodable {
        let workspaceRoot: String
        let relPath: String
    }

    private func decodePathInfo(from json: String) -> FsPathInfo? {
        guard let data = json.data(using: .utf8) else { return nil }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try? decoder.decode(FsPathInfo.self, from: data)
    }

    private func decodeShellCmd(from json: String) -> String? {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let params = obj["params"] as? [String: Any],
              let cmd = params["cmd"] as? String
        else { return nil }
        return cmd
    }
}
