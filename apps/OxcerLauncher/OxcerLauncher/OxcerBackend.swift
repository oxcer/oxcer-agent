//  OxcerBackend.swift
//  OxcerLauncher
//
//  Service layer abstraction over the UniFFI Rust bridge (oxcer_ffi).
//  Keeps FFI details out of view models and makes mocking trivial for tests.
//
//  Architecture:
//   - OxcerBackend protocol: what the app needs from the backend.
//   - DefaultOxcerBackend: thin wrappers over UniFFI-generated functions.
//   - MockOxcerBackend: for SwiftUI previews and unit tests.
//
//  The step-based agent API (agentStep) is the only path used by the chat UI.
//  The stub executor path (runAgentTaskDirect) lives only on DefaultOxcerBackend,
//  not on the protocol, so it cannot be called accidentally from the UI layer.

import Foundation

// MARK: - Protocol

protocol OxcerBackend {
    /// Zero-cost FFI warm-up. Triggers dylib load and static runtime init.
    func ping() -> String

    func ensureLocalModel(appConfigDir: String, onProgress: @escaping (Double, String) -> Void) async throws
    func listWorkspaces(appConfigDir: String) async throws -> [WorkspaceInfo]
    func listSessions(appConfigDir: String) async throws -> [SessionSummary]
    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent]

    /// Step-based agent API. Drives one orchestrator step.
    ///
    /// Call via `AgentRunner.run(env:)` — do not call this directly from the UI layer.
    /// - First call: `sessionJson: nil`, `lastResult: nil`.
    /// - Subsequent calls: pass `outcome.session.sessionJson` back unchanged.
    func agentStep(
        env: AgentEnvironment,
        sessionJson: String?,
        lastResult: FfiStepResult?
    ) throws -> FfiStepOutcome
}

// MARK: - DownloadCallback adapter

/// Wraps a closure as a DownloadCallback for the Rust FFI.
private final class SwiftDownloadCallback: DownloadCallback {
    let onProgressHandler: (Double, String) -> Void

    init(onProgress: @escaping (Double, String) -> Void) {
        self.onProgressHandler = onProgress
    }

    func onProgress(progress: Double, message: String) {
        onProgressHandler(progress, message)
    }
}

// MARK: - DefaultOxcerBackend

/// Forwards calls to UniFFI-generated functions.
///
/// Synchronous FFI functions (listWorkspaces, listSessions, loadSessionLog, agentStep)
/// are called with `try` directly — no Task.detached needed; they return quickly.
/// Asynchronous FFI functions (ensureLocalModel, generateText) are called with `try await`.
struct DefaultOxcerBackend: OxcerBackend {

    func ping() -> String {
        OxcerLauncher.ping()
    }

    func ensureLocalModel(appConfigDir: String, onProgress: @escaping (Double, String) -> Void) async throws {
        let callback = SwiftDownloadCallback(onProgress: onProgress)
        try await OxcerLauncher.ensureLocalModel(appConfigDir: appConfigDir, callback: callback)
    }

    func listWorkspaces(appConfigDir: String) async throws -> [WorkspaceInfo] {
        try OxcerLauncher.listWorkspaces(appConfigDir: appConfigDir)
    }

    func listSessions(appConfigDir: String) async throws -> [SessionSummary] {
        try OxcerLauncher.listSessions(appConfigDir: appConfigDir)
    }

    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent] {
        try OxcerLauncher.loadSessionLog(sessionId: sessionId, appConfigDir: appConfigDir)
    }

    func agentStep(
        env: AgentEnvironment,
        sessionJson: String?,
        lastResult: FfiStepResult?
    ) throws -> FfiStepOutcome {
        try OxcerLauncher.ffiAgentStep(
            taskDescription: env.taskDescription,
            workspaceId: env.workspaceId,
            workspaceRoot: env.workspaceRoot,
            appConfigDir: env.appConfigDir,
            sessionJson: sessionJson,
            lastResult: lastResult
        )
    }

    // MARK: Stub executor (debug/tests only — NOT on the protocol)

    /// Runs the agent via the Rust stub executor that always fails tool calls.
    /// Only useful for confirming that the FFI layer itself is wired up; do not call from the UI.
    func runAgentTaskDirect(payload: AgentRequestPayload) async throws -> AgentResponse {
        try await OxcerLauncher.runAgentTask(payload: payload)
    }
}

// MARK: - MockOxcerBackend (SwiftUI Previews)

struct MockOxcerBackend: OxcerBackend {

    func ping() -> String { "pong" }

    func ensureLocalModel(appConfigDir: String, onProgress: @escaping (Double, String) -> Void) async throws {
        onProgress(0.0, "Starting download...")
        try await Task.sleep(nanoseconds: 100_000_000)
        onProgress(1.0, "Model Ready!")
    }

    func listWorkspaces(appConfigDir: String) async throws -> [WorkspaceInfo] { [] }

    func listSessions(appConfigDir: String) async throws -> [SessionSummary] {
        [
            SessionSummary(
                sessionId: "mock-session-1",
                startTimestamp: "2025-01-01T12:00:00Z",
                endTimestamp: "2025-01-01T12:05:00Z",
                totalCostUsd: 0.005,
                success: true,
                toolCallsCount: 3,
                approvalsCount: 1,
                deniesCount: 0
            )
        ]
    }

    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent] { [] }

    /// Immediately returns "complete" so previews render without FFI.
    func agentStep(
        env: AgentEnvironment,
        sessionJson: String?,
        lastResult: FfiStepResult?
    ) throws -> FfiStepOutcome {
        FfiStepOutcome(
            status: "complete",
            intent: nil,
            approvalRequestId: nil,
            finalAnswer: "This is a mock response from Oxcer Preview.",
            session: FfiSessionState(sessionJson: "{}")
        )
    }
}
