//  OxcerBackend.swift
//  OxcerLauncher
//
//  Service layer abstraction over the UniFFI Rust bridge (oxcer_ffi).
//  Keeps FFI details out of view models and makes mocking trivial for tests.
//
//  # Agent execution paths
//
//  DefaultOxcerBackend.runAgentTask uses SwiftAgentExecutor, which drives the
//  ffi_agent_step() loop from Swift. This gives full visibility into every tool
//  call, dispatches LlmGenerate → generateText() (phi-3-mini), FS reads →
//  FileManager, and returns descriptive errors for mutating FS / shell ops.
//
//  A direct Rust-side path (FfiLlmExecutor inside run_agent_task) is also
//  available via runAgentTaskDirect(payload:) for quick tests and fallback.

import Foundation

/// OxcerBackend
/// - Thin async/await wrappers over UniFFI-generated globals (listWorkspaces, etc.).
/// - Mirrors the Rust-facing contracts without changing payloads or signatures.
///
/// ensureLocalModel semantics:
/// - Idempotent: subsequent calls no-op if model files are already present.
/// - Safe to retry after failure: call again to attempt a clean download.
/// - Throws with a user-friendly description on unrecoverable failure.
protocol OxcerBackend {
    /// Zero-cost FFI warm-up. Triggers dylib load and static runtime init.
    func ping() -> String
    func ensureLocalModel(appConfigDir: String, onProgress: @escaping (Double, String) -> Void) async throws
    func listWorkspaces(appConfigDir: String) async throws -> Int32
    func listSessions(appConfigDir: String) async throws -> [SessionSummary]
    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent]
    /// Runs the agent task via the Swift step-driven executor.
    /// LlmGenerate → phi-3-mini, FS reads → FileManager, shell/mutating FS → error.
    func runAgentTask(payload: AgentRequestPayload) async throws -> AgentResponse
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

/// DefaultOxcerBackend
///
/// - Forwards listing/session calls to UniFFI-generated global functions.
/// - Runs agent tasks via SwiftAgentExecutor (step-loop with ffi_agent_step()).
/// - Offloads blocking FFI work to detached tasks so the main actor stays responsive.
struct DefaultOxcerBackend: OxcerBackend {

    func ping() -> String {
        OxcerLauncher.ping()
    }

    func ensureLocalModel(appConfigDir: String, onProgress: @escaping (Double, String) -> Void) async throws {
        let callback = SwiftDownloadCallback(onProgress: onProgress)
        try await OxcerLauncher.ensureLocalModel(appConfigDir: appConfigDir, callback: callback)
    }

    func listWorkspaces(appConfigDir: String) async throws -> Int32 {
        return await Task.detached(priority: .userInitiated) {
            OxcerLauncher.listWorkspaces(appConfigDir: appConfigDir)
        }.value
    }

    func listSessions(appConfigDir: String) async throws -> [SessionSummary] {
        // listSessions is synchronous in Rust; run it off the main actor.
        try await Task.detached(priority: .userInitiated) {
            try OxcerLauncher.listSessions(appConfigDir: appConfigDir)
        }.value
    }

    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent] {
        try await Task.detached(priority: .userInitiated) {
            try OxcerLauncher.loadSessionLog(sessionId: sessionId, appConfigDir: appConfigDir)
        }.value
    }

    // MARK: - Agent execution (Swift step-driven path)

    /// Runs the agent via SwiftAgentExecutor:
    ///   1. ffi_agent_step() drives the orchestrator (routing + planning in Rust).
    ///   2. LlmGenerate intents → generateText() → phi-3-mini inference.
    ///   3. FS read intents → FileManager (safe in Xcode).
    ///   4. Mutating FS / shell intents → descriptive error returned to UI.
    func runAgentTask(payload: AgentRequestPayload) async throws -> AgentResponse {
        return try await SwiftAgentExecutor().runTask(payload: payload)
    }

    // MARK: - Direct Rust path (fallback / testing)

    /// Calls run_agent_task() directly in Rust (FfiLlmExecutor).
    /// Handles LlmGenerate via the global phi-3-mini engine inside Rust.
    /// FS/shell intents return an error; no Swift-side dispatch.
    func runAgentTaskDirect(payload: AgentRequestPayload) async throws -> AgentResponse {
        try await OxcerLauncher.runAgentTask(payload: payload)
    }
}

// MARK: - Mock Implementation (For SwiftUI Previews)

struct MockOxcerBackend: OxcerBackend {
    func ping() -> String {
        "pong"
    }

    func ensureLocalModel(appConfigDir: String, onProgress: @escaping (Double, String) -> Void) async throws {
        onProgress(0.0, "Starting download...")
        try await Task.sleep(nanoseconds: 100_000_000) // 0.1 s
        onProgress(1.0, "Model Ready!")
    }

    func listWorkspaces(appConfigDir: String) async throws -> Int32 {
        42
    }

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

    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent] {
        []
    }

    func runAgentTask(payload: AgentRequestPayload) async throws -> AgentResponse {
        try await Task.sleep(nanoseconds: 500_000_000) // 0.5 s
        return AgentResponse(ok: true, answer: "This is a mock response from Oxcer Preview.", error: nil)
    }
}
