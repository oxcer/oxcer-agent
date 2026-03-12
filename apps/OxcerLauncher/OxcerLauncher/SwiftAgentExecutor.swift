//  SwiftAgentExecutor.swift
//  OxcerLauncher
//
//  Executes FfiToolIntent items emitted by ffi_agent_step.
//  Called by AgentRunner for every "need_tool" outcome.
//
//  Tool kinds (matching Rust ToolCallIntent variants in oxcer-core/src/orchestrator.rs):
//    llm_generate   -> OxcerLauncher.generateText(prompt:)
//    fs_list_dir    -> FileManager.contentsOfDirectory
//    fs_read_file   -> String(contentsOfFile:)
//    fs_write_file  -> Data(base64:).write (atomic)
//    fs_delete      -> FileManager.removeItem
//    fs_rename      -> FileManager.moveItem (same workspace root)
//    fs_move        -> FileManager.moveItem (cross-workspace)
//    shell_run      -> /bin/sh -c cmd  (off cooperative thread pool)

import Foundation
import OSLog

private let execLogger = Logger(subsystem: "com.oxcer.launcher", category: "AgentExecutor")

// MARK: - LLM timeout error

/// Thrown when `generateText` does not complete within `config.llmTimeoutSeconds`.
/// `SwiftAgentExecutor.execute(intent:)` converts this into an `FfiStepResult(ok: false, …)`
/// so the orchestrator can report it rather than hanging the step loop.
enum LLMTimeoutError: LocalizedError {
    case exceeded(TimeInterval)

    var errorDescription: String? {
        if case .exceeded(let s) = self {
            return "LLM did not respond within \(Int(s)) seconds"
        }
        return nil
    }
}

// MARK: - SwiftAgentExecutor

struct SwiftAgentExecutor {

    /// Per-backend generation parameters. Defaults to `ModelBackendConfig.current()` so callers
    /// that don't set it explicitly still get the correct timeout and maxSteps values.
    var config: ModelBackendConfig = .current()

    /// Returns a user-facing message for LLM generation failures.
    /// Deliberately omits internal model names and implementation details.
    static func makeModelErrorMessage(_ details: String) -> String {
        "Oxcer had trouble generating a reply (details: \(details))"
    }

    /// Dispatch one intent, execute it, and return the FfiStepResult for the next ffi_agent_step call.
    /// Never throws — failures are captured as `FfiStepResult(ok: false, error: ...)` so that the
    /// orchestrator can incorporate the error into its reasoning rather than crashing the whole run.
    func execute(intent: FfiToolIntent, sessionId: String? = nil) async -> FfiStepResult {
        let t0 = Date()
        let sid = sessionId ?? "unknown"
        execLogger.debug("execute intent=\(intent.kind, privacy: .public) sid=\(sid, privacy: .public)")
        do {
            let payloadJson = try await dispatch(intent: intent)
            let elapsed = -t0.timeIntervalSinceNow
            execLogger.info("execute ok intent=\(intent.kind, privacy: .public) elapsed=\(String(format: "%.3f", elapsed), privacy: .public)s payload=\(payloadJson.count, privacy: .public)ch sid=\(sid, privacy: .public)")
            return FfiStepResult(ok: true, payloadJson: payloadJson, error: nil)
        } catch {
            let elapsed = -t0.timeIntervalSinceNow
            execLogger.error("execute failed intent=\(intent.kind, privacy: .public) elapsed=\(String(format: "%.3f", elapsed), privacy: .public)s sid=\(sid, privacy: .public) err=\(error.localizedDescription, privacy: .public)")
            return FfiStepResult(ok: false, payloadJson: nil, error: error.localizedDescription)
        }
    }

    // MARK: - Dispatch

    private func dispatch(intent: FfiToolIntent) async throws -> String {
        switch intent.kind {
        case "llm_generate":  return try await handleLlmGenerate(intent.intentJson)
        case "fs_list_dir":   return try handleFsListDir(intent.intentJson)
        case "fs_read_file":  return try handleFsReadFile(intent.intentJson)
        case "fs_write_file": return try handleFsWriteFile(intent.intentJson)
        case "fs_delete":     return try handleFsDelete(intent.intentJson)
        case "fs_rename":     return try handleFsRename(intent.intentJson)
        case "fs_move":       return try handleFsMove(intent.intentJson)
        case "fs_create_dir": return try handleFsCreateDir(intent.intentJson)
        case "shell_run":     return try await handleShellRun(intent.intentJson)
        default:
            throw ExecutorError.unknownKind(intent.kind)
        }
    }

    // MARK: - LLM

    private func handleLlmGenerate(_ json: String) async throws -> String {
        let intent = try decode(LlmGenerateIntent.self, from: json)
        execLogger.debug("LlmGenerate prompt.count=\(intent.task.count, privacy: .public) timeout=\(config.llmTimeoutSeconds, privacy: .public)s")
        do {
            let text = try await withTimeout(seconds: config.llmTimeoutSeconds) {
                try await OxcerLauncher.generateText(prompt: intent.task)
            }
            if text.isEmpty {
                execLogger.warning("LlmGenerate generateText returned empty string")
            } else {
                execLogger.info("LlmGenerate ok text.count=\(text.count, privacy: .public)")
            }
            return try encode(TextPayload(text: text))
        } catch {
            execLogger.error("LlmGenerate generateText threw: \(error.localizedDescription, privacy: .public)")
            throw error
        }
    }

    // MARK: - Timeout helper

    /// Races `operation` against a deadline of `seconds`.
    /// If the deadline fires first, throws `LLMTimeoutError.exceeded(seconds)` and cancels the group.
    ///
    /// **Cancellation caveat**: cancelling the Swift Task does NOT interrupt a Rust `spawn_blocking`
    /// thread already running inside `generateText`. The thread continues until it finishes, but Swift
    /// stops waiting for it — the UI unblocks immediately. Acceptable for v0.1.
    private func withTimeout<T: Sendable>(
        seconds: TimeInterval,
        operation: @escaping @Sendable () async throws -> T
    ) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await operation() }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                throw LLMTimeoutError.exceeded(seconds)
            }
            defer { group.cancelAll() }
            return try await group.next()!
        }
    }

    // MARK: - Filesystem

    private func handleFsListDir(_ json: String) throws -> String {
        let intent = try decode(FsPathIntent.self, from: json)
        let dir = URL(fileURLWithPath: intent.workspaceRoot)
            .appendingPathComponent(intent.relPath)
            .standardized
        execLogger.debug("fs_list_dir path=\(dir.path, privacy: .public)")
        let entries = try FileManager.default.contentsOfDirectory(atPath: dir.path)
        execLogger.info("fs_list_dir entries=\(entries.count, privacy: .public) path=\(dir.path, privacy: .public)")
        return try encode(EntriesPayload(entries: entries, dirURL: dir))
    }

    private func handleFsReadFile(_ json: String) throws -> String {
        let intent = try decode(FsPathIntent.self, from: json)
        let path = URL(fileURLWithPath: intent.workspaceRoot)
            .appendingPathComponent(intent.relPath)
            .standardized
            .path
        execLogger.debug("fs_read_file path=\(path, privacy: .public)")
        let text = try String(contentsOfFile: path, encoding: .utf8)
        execLogger.info("fs_read_file chars=\(text.count, privacy: .public) path=\(path, privacy: .public)")
        return try encode(TextPayload(text: text))
    }

    private func handleFsWriteFile(_ json: String) throws -> String {
        let intent = try decode(FsWriteIntent.self, from: json)
        guard let fileData = Data(base64Encoded: intent.contentsBase64) else {
            throw ExecutorError.base64DecodeFailed
        }
        let path = URL(fileURLWithPath: intent.workspaceRoot).appendingPathComponent(intent.relPath)
        try FileManager.default.createDirectory(
            at: path.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try fileData.write(to: path, options: .atomic)
        return try encode(OkPayload())
    }

    private func handleFsDelete(_ json: String) throws -> String {
        let intent = try decode(FsPathIntent.self, from: json)
        let path = URL(fileURLWithPath: intent.workspaceRoot).appendingPathComponent(intent.relPath).path
        try FileManager.default.removeItem(atPath: path)
        return try encode(OkPayload())
    }

    private func handleFsRename(_ json: String) throws -> String {
        let intent = try decode(FsRenameIntent.self, from: json)
        let src = URL(fileURLWithPath: intent.workspaceRoot).appendingPathComponent(intent.relPath).path
        let dst = URL(fileURLWithPath: intent.workspaceRoot).appendingPathComponent(intent.newRelPath).path
        try FileManager.default.moveItem(atPath: src, toPath: dst)
        return try encode(OkPayload())
    }

    private func handleFsMove(_ json: String) throws -> String {
        let intent = try decode(FsMoveIntent.self, from: json)
        let src = URL(fileURLWithPath: intent.workspaceRoot).appendingPathComponent(intent.relPath).path
        let dst = URL(fileURLWithPath: intent.destWorkspaceRoot).appendingPathComponent(intent.destRelPath).path
        try FileManager.default.moveItem(atPath: src, toPath: dst)
        return try encode(OkPayload())
    }

    private func handleFsCreateDir(_ json: String) throws -> String {
        let intent = try decode(FsPathIntent.self, from: json)
        let dir = URL(fileURLWithPath: intent.workspaceRoot)
            .appendingPathComponent(intent.relPath)
            .standardized
        execLogger.debug("fs_create_dir path=\(dir.path, privacy: .public)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        execLogger.info("fs_create_dir ok path=\(dir.path, privacy: .public)")
        return try encode(OkPayload())
    }

    // MARK: - Shell

    /// Runs `/bin/sh -c cmd` on a GCD thread so `waitUntilExit()` never blocks
    /// the Swift concurrency cooperative pool.
    private func handleShellRun(_ json: String) async throws -> String {
        let intent = try decode(ShellRunIntent.self, from: json)

        guard let cmd = intent.params.cmd else {
            // Return a non-fatal error payload: the orchestrator can reason about it.
            return try encode(ShellPayload(
                stdout: "",
                stderr: "shell_run: no 'cmd' in params for commandId '\(intent.commandId)'",
                exitCode: 1
            ))
        }

        let root = intent.workspaceRoot

        // Move the blocking waitUntilExit off the cooperative pool.
        let (stdout, stderr, exitCode) = try await withCheckedThrowingContinuation { continuation in
            DispatchQueue.global(qos: .utility).async {
                do {
                    let process = Process()
                    process.executableURL = URL(fileURLWithPath: "/bin/sh")
                    process.arguments = ["-c", cmd]
                    if !root.isEmpty {
                        process.currentDirectoryURL = URL(fileURLWithPath: root)
                    }
                    let outPipe = Pipe()
                    let errPipe = Pipe()
                    process.standardOutput = outPipe
                    process.standardError = errPipe
                    try process.run()
                    process.waitUntilExit()
                    let out = String(data: outPipe.fileHandleForReading.readDataToEndOfFile(),
                                    encoding: .utf8) ?? ""
                    let err = String(data: errPipe.fileHandleForReading.readDataToEndOfFile(),
                                    encoding: .utf8) ?? ""
                    continuation.resume(returning: (out, err, Int(process.terminationStatus)))
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }

        return try encode(ShellPayload(stdout: stdout, stderr: stderr, exitCode: exitCode))
    }

    // MARK: - Codable helpers

    /// Decodes an intent from its JSON string using snake_case → camelCase conversion.
    /// Throws `ExecutorError.intentDecoding` with the underlying `DecodingError` on failure,
    /// giving a specific field path rather than a generic "missing field" message.
    private func decode<T: Decodable>(_ type: T.Type, from json: String) throws -> T {
        guard let data = json.data(using: .utf8) else {
            throw ExecutorError.malformedUtf8
        }
        do {
            return try Self.intentDecoder.decode(type, from: data)
        } catch let decodingError as DecodingError {
            throw ExecutorError.intentDecoding(decodingError)
        }
    }

    private func encode<T: Encodable>(_ value: T) throws -> String {
        let data = try JSONEncoder().encode(value)
        return String(data: data, encoding: .utf8) ?? "{}"
    }

    private static let intentDecoder: JSONDecoder = {
        let d = JSONDecoder()
        d.keyDecodingStrategy = .convertFromSnakeCase
        return d
    }()
}

// MARK: - Intent models (Codable, matching Rust ToolCallIntent fields)
// `convertFromSnakeCase` handles workspace_root → workspaceRoot etc. automatically.

private struct LlmGenerateIntent: Decodable {
    let task: String
    // `strategy` and `system_hint` present in JSON but unused by the executor.
}

private struct FsPathIntent: Decodable {
    let workspaceRoot: String
    let relPath: String
}

private struct FsWriteIntent: Decodable {
    let workspaceRoot: String
    let relPath: String
    let contentsBase64: String
}

private struct FsRenameIntent: Decodable {
    let workspaceRoot: String
    let relPath: String
    let newRelPath: String
}

private struct FsMoveIntent: Decodable {
    let workspaceRoot: String
    let relPath: String
    let destWorkspaceRoot: String
    let destRelPath: String
}

private struct ShellRunIntent: Decodable {
    let commandId: String
    let workspaceRoot: String
    let params: Params

    /// Rust's `serde_json::Value` params. Only `cmd` is used today.
    struct Params: Decodable {
        let cmd: String?
    }
}

// MARK: - Payload models (Encodable, returned to Rust orchestrator)

private struct TextPayload: Encodable { let text: String }

/// Payload returned by `fs_list_dir`.
///
/// - `entries`: alphabetically sorted filenames (stable order for display).
/// - `sortedByModified`: filenames sorted newest-first by modification date.
///   Consumed by `next_action` in Rust to resolve the `{{MOST_RECENT_FILE}}`
///   placeholder in a subsequent `FsReadFile` step.
/// - `text`: newline-joined alphabetical listing substituted into `{{FS_RESULT}}`
///   in `LlmGenerate` tasks.
private struct EntriesPayload: Encodable {
    let entries: [String]
    let sortedByModified: [String]
    let text: String

    init(entries: [String], dirURL: URL) {
        let fm = FileManager.default
        // Pair each filename with its modification date (distantPast on error).
        let withDates: [(String, Date)] = entries.map { name in
            let url = dirURL.appendingPathComponent(name)
            let date = (try? fm.attributesOfItem(atPath: url.path)[.modificationDate] as? Date) ?? .distantPast
            return (name, date)
        }
        self.sortedByModified = withDates.sorted { $0.1 > $1.1 }.map(\.0)
        self.entries = entries.sorted()
        self.text = self.entries.joined(separator: "\n")
    }
}

private struct OkPayload: Encodable { let ok: Bool = true }

private struct ShellPayload: Encodable {
    let stdout: String
    let stderr: String
    let exitCode: Int
}

// MARK: - Executor errors

enum ExecutorError: LocalizedError {
    case unknownKind(String)
    case malformedUtf8
    case intentDecoding(DecodingError)
    case base64DecodeFailed

    var errorDescription: String? {
        switch self {
        case .unknownKind(let k):
            return "SwiftAgentExecutor: unknown tool kind '\(k)'"
        case .malformedUtf8:
            return "SwiftAgentExecutor: intent JSON is not valid UTF-8"
        case .intentDecoding(let e):
            return "SwiftAgentExecutor: intent JSON decoding failed — \(e.localizedDescription)"
        case .base64DecodeFailed:
            return "SwiftAgentExecutor: contentsBase64 is not valid base64"
        }
    }
}
