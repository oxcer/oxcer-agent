//  OxcerBackend.swift
//  OxcerLauncher
//
//  Service layer abstraction over the UniFFI Rust bridge (oxcer_ffi).
//  Keeps FFI details out of view models and makes mocking trivial for tests.

import Foundation

/// OxcerBackend
/// Implemented:
/// - Thin async/await wrappers over the UniFFI-generated global functions (listWorkspaces, listSessions, etc.).
/// - Mirrors the Rust-facing contracts without changing any payloads or signatures.
///
/// ensureLocalModel semantics:
/// - Idempotent: if the model is already available, returns quickly without re-downloading or reallocating.
/// - Safe to retry after failure: call again to attempt a clean download.
/// - Throws with a user-friendly description on unrecoverable failure (no network, disk full, etc.).
protocol OxcerBackend {
    /// Zero-cost FFI warm-up. Triggers dylib load and static runtime init. Call first in AppViewModel.init.
    func ping() -> String
    func ensureLocalModel(appConfigDir: String, onProgress: @escaping (Double, String) -> Void) async throws
    func listWorkspaces(appConfigDir: String) async throws -> [WorkspaceInfo]
    func listSessions(appConfigDir: String) async throws -> [SessionSummary]
    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent]
    func runAgentTask(payload: AgentRequestPayload) async throws -> AgentResponse
}

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

/// DefaultOxcerBackend
/// Implemented:
/// - Forwards calls to UniFFI-generated functions using Swift concurrency.
/// - Synchronous FFI functions (listWorkspaces, listSessions, loadSessionLog) are called
///   directly with `try`; no Task.detached needed as they return quickly.
/// - Asynchronous FFI functions (ensureLocalModel, runAgentTask) are called with `try await`.
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

    func runAgentTask(payload: AgentRequestPayload) async throws -> AgentResponse {
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
        try await Task.sleep(nanoseconds: 100_000_000) // 0.1s
        onProgress(1.0, "Model Ready!")
    }

    func listWorkspaces(appConfigDir: String) async throws -> [WorkspaceInfo] {
        []
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
        try await Task.sleep(nanoseconds: 500_000_000)
        return AgentResponse(ok: true, answer: "This is a mock response from Oxcer Preview.", error: nil)
    }
}
