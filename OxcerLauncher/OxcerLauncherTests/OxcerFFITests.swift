//  OxcerFFITests.swift
//  OxcerLauncherTests
//
//  XCTest for FFI layer: workspace loading, session listing, error propagation.
//  Requires: Add OxcerLauncherTests target, set Host Application = OxcerLauncher,
//  and add this file to the test target.

import XCTest
@testable import OxcerLauncher

final class OxcerFFITests: XCTestCase {

    /// Happy path: valid config.json with workspaces returns list.
    func testListWorkspaces_validConfig_returnsWorkspaces() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("oxcer_test_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }

        let config = """
        {"workspaces":[{"id":"ws1","name":"Test","root_path":"/tmp/proj"}]}
        """
        try config.write(to: tmp.appendingPathComponent("config.json"), atomically: true, encoding: .utf8)
        let dir = tmp.path

        let list = try OxcerFFI.listWorkspaces(appConfigDir: dir)
        XCTAssertEqual(list.count, 1)
        XCTAssertEqual(list[0].id, "ws1")
        XCTAssertEqual(list[0].name, "Test")
        XCTAssertEqual(list[0].rootPath, "/tmp/proj")
    }

    /// Empty or missing config returns empty list (no throw).
    func testListWorkspaces_emptyConfig_returnsEmpty() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("oxcer_test_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        // No config.json — listWorkspaces returns [] (or empty)
        let dir = tmp.path
        let list = try OxcerFFI.listWorkspaces(appConfigDir: dir)
        XCTAssertTrue(list.isEmpty)
    }

    /// Invalid payload to agentRequest throws (e.g. empty task handled by Rust).
    func testAgentRequest_invalidPayload_propagatesError() {
        let payload = AgentRequestPayload(
            taskDescription: "",
            workspaceId: nil,
            workspaceRoot: nil,
            context: nil,
            appConfigDir: nil
        )
        XCTAssertThrowsError(try OxcerFFI.agentRequest(payload)) { error in
            XCTAssertTrue(error is OxcerFFIError)
        }
    }

    /// listSessions with empty logs dir returns empty array.
    func testListSessions_emptyLogsDir_returnsEmpty() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("oxcer_test_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        let dir = tmp.path
        let list = try OxcerFFI.listSessions(appConfigDir: dir)
        XCTAssertTrue(list.isEmpty)
    }

    /// loadSessionLog with empty sessionId — Rust uses "default" as filename; may succeed or fail.
    /// Passing clearly invalid session_id that maps to nonexistent file should throw.
    func testLoadSessionLog_nonexistentSession_throws() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("oxcer_test_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        // No session file exists
        XCTAssertThrowsError(try OxcerFFI.loadSessionLog(sessionId: "nonexistent-session-12345", appConfigDir: tmp.path)) { _ in }
    }
}
