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
    func listWorkspaces(appConfigDir: String) async throws -> Int32
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
/// - Forwards calls to UniFFI-generated functions (global listWorkspaces, listSessions, etc.) using Swift concurrency.
/// - Offloads blocking FFI work to a detached task so the main actor stays responsive.
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
        try await Task.detached(priority: .userInitiated) {
            try await listSessions(appConfigDir: appConfigDir)
        }.value
    }

    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent] {
        try await Task.detached(priority: .userInitiated) {
            try await loadSessionLog(sessionId: sessionId, appConfigDir: appConfigDir)
        }.value
    }

    func runAgentTask(payload: AgentRequestPayload) async throws -> AgentResponse {
        try await Task.detached(priority: .userInitiated) {
            try await runAgentTask(payload: payload)
        }.value
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
        try await Task.sleep(nanoseconds: 500_000_000)
        return AgentResponse(ok: true, answer: "This is a mock response from Oxcer Preview.", error: nil)
    }
}
