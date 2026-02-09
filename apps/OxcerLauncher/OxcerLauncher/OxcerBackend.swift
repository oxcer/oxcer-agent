//  OxcerBackend.swift
//  OxcerLauncher
//
//  Service layer abstraction over the OxcerFFI Rust bridge.
//  Keeps FFI details out of view models and makes mocking trivial for tests.

import Foundation

/// OxcerBackend
/// Implemented:
/// - Thin async/await wrappers over the existing synchronous OxcerFFI API.
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
/// - Forwards calls directly to OxcerFFI using Swift concurrency.
/// - Offloads blocking FFI work to a detached task so the main actor stays responsive.
struct DefaultOxcerBackend: OxcerBackend {
    func listWorkspaces(appConfigDir: String) async throws -> [WorkspaceInfo] {
        try await Task.detached(priority: .userInitiated) {
            try OxcerFFI.listWorkspaces(appConfigDir: appConfigDir)
        }.value
    }

    func listSessions(appConfigDir: String) async throws -> [SessionSummary] {
        try await Task.detached(priority: .userInitiated) {
            try OxcerFFI.listSessions(appConfigDir: appConfigDir)
        }.value
    }

    func loadSessionLog(sessionId: String, appConfigDir: String) async throws -> [LogEvent] {
        try await Task.detached(priority: .userInitiated) {
            try OxcerFFI.loadSessionLog(sessionId: sessionId, appConfigDir: appConfigDir)
        }.value
    }

    func runAgentTask(payload: AgentRequestPayload) async throws -> AgentResponse {
        try await Task.detached(priority: .userInitiated) {
            try OxcerFFI.agentRequest(payload)
        }.value
    }
}

