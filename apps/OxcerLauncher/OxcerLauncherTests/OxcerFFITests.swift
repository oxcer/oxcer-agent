//  OxcerFFITests.swift
//  OxcerLauncherTests
//
//  XCTest suite for the FFI layer — Stage 4 (live contract).
//
//  The virtual-memory sentinel is the primary guard against a recurrence of the
//  88 GB reservation bug caused by FFI type confusion (i32 read as RustBuffer).

import Darwin // mach_task_basic_info
import XCTest
@testable import OxcerLauncher

// MARK: - Memory utility

/// Returns the process virtual address space size in bytes via mach_task_basic_info.
/// Not a substitute for Instruments, but fast enough for a regression sentinel.
private func currentVirtualMemoryBytes() -> UInt64 {
    var info = mach_task_basic_info()
    var count = mach_msg_type_number_t(MemoryLayout<mach_task_basic_info>.size / MemoryLayout<integer_t>.size)
    let result = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: 1) {
            task_info(mach_task_self_, task_flavor_t(MACH_TASK_BASIC_INFO), $0, &count)
        }
    }
    return result == KERN_SUCCESS ? info.virtual_size : 0
}

// MARK: - Stage 4: Live contract
//
// Rust: pub fn list_workspaces(dir: String) -> Result<Vec<WorkspaceInfo>, OxcerError>
// Reads config.json from the supplied directory via list_workspaces_impl().

final class OxcerFFIStage4Tests: XCTestCase {

    func testListWorkspaces_validConfig_returnsWorkspaces() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("oxcer_test_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }

        let config = #"{"workspaces":[{"id":"ws1","name":"Test","root_path":"/tmp/proj"}]}"#
        try config.write(to: tmp.appendingPathComponent("config.json"),
                         atomically: true, encoding: .utf8)

        let list = try listWorkspaces(appConfigDir: tmp.path)
        XCTAssertEqual(list.count, 1)
        XCTAssertEqual(list[0].id, "ws1")
        XCTAssertEqual(list[0].name, "Test")
        XCTAssertEqual(list[0].rootPath, "/tmp/proj")
    }

    func testListWorkspaces_missingConfig_returnsEmpty() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("oxcer_test_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }

        let list = try listWorkspaces(appConfigDir: tmp.path)
        XCTAssertTrue(list.isEmpty)
    }

    func testListWorkspaces_malformedJson_throws() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("oxcer_test_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        try "not json".write(to: tmp.appendingPathComponent("config.json"),
                             atomically: true, encoding: .utf8)

        XCTAssertThrowsError(try listWorkspaces(appConfigDir: tmp.path))
    }

    /// Virtual-memory sentinel.
    ///
    /// Measures virtual address space growth across the FFI call.
    /// Growth over 50 MB signals type confusion (the original bug reserved ~88 GB).
    func testVirtualMemorySentinel_stage4() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("oxcer_test_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }

        let limitBytes: UInt64 = 50 * 1024 * 1024
        let before = currentVirtualMemoryBytes()
        _ = try listWorkspaces(appConfigDir: tmp.path)
        let after = currentVirtualMemoryBytes()
        let growth = after > before ? after - before : 0
        XCTAssertLessThan(growth, limitBytes,
            "Virtual memory grew by \(growth / 1024 / 1024) MB. " +
            "Threshold is 50 MB. Possible FFI type confusion — " +
            "check that the Swift bindings were regenerated after the last Rust change.")
    }
}

// MARK: - Existing FFI contract tests (unchanged)

final class OxcerFFIContractTests: XCTestCase {

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
        let list = try OxcerFFI.listSessions(appConfigDir: tmp.path)
        XCTAssertTrue(list.isEmpty)
    }

    /// loadSessionLog with clearly invalid session_id should throw.
    func testLoadSessionLog_nonexistentSession_throws() throws {
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("oxcer_test_\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmp) }
        XCTAssertThrowsError(
            try OxcerFFI.loadSessionLog(sessionId: "nonexistent-session-12345",
                                        appConfigDir: tmp.path)
        )
    }
}
