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
/// TODO:
/// - Add additional methods as new backend capabilities are exposed from Rust.
protocol OxcerBackend {
    func listWorkspaces(appConfigDir: String) async throws -> [WorkspaceInfo]
    func listSessions(appConfigDir: String) async throws -> [SessionSummary]
    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent]
    func runAgentTask(payload: AgentRequestPayload) async throws -> AgentResponse
}

/// DefaultOxcerBackend
/// Implemented:
/// - Forwards calls to UniFFI-generated functions (global listWorkspaces, listSessions, etc.) using Swift concurrency.
/// - Offloads blocking FFI work to a detached task so the main actor stays responsive.
struct DefaultOxcerBackend: OxcerBackend {
    func listWorkspaces(appConfigDir: String) async throws -> [WorkspaceInfo] {
        try await Task.detached(priority: .userInitiated) {
            try await listWorkspaces(appConfigDir: appConfigDir)
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
    func listWorkspaces(appConfigDir: String) async throws -> [WorkspaceInfo] {
        [
            WorkspaceInfo(id: "mock-1", name: "Demo Workspace", rootPath: "/Users/demo/project")
        ]
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
