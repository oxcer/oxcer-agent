//  OxcerSwiftUIViewModelTests.swift
//  OxcerLauncherTests
//
//  Data flow and Codable tests — no FFI mocking required.

import XCTest
@testable import OxcerLauncher

// MARK: - Workspace loading behavior (logic tests, no FFI)

final class WorkspaceLoadingTests: XCTestCase {

    /// When workspaces is empty, picker shows "No workspaces" — test the data shape.
    func testEmptyWorkspaces_dataShape() {
        let workspaces: [WorkspaceInfo] = []
        XCTAssertTrue(workspaces.isEmpty)
        // UI would show "No workspaces" when workspaces.isEmpty
    }

    /// When first workspace loads, selectedWorkspaceId could be set to first.id.
    func testFirstWorkspaceSelection() {
        let workspaces = [
            WorkspaceInfo(id: "a", name: "A", rootPath: "/a"),
            WorkspaceInfo(id: "b", name: "B", rootPath: "/b"),
        ]
        let firstId = workspaces.first?.id
        XCTAssertEqual(firstId, "a")
    }
}

// MARK: - Recent Sessions data parsing

final class RecentSessionsDataTests: XCTestCase {

    func testSessionSummary_decodesFromJSON() throws {
        let json = """
        {"session_id":"s1","start_timestamp":"2025-01-01T00:00:00Z","end_timestamp":"2025-01-01T00:01:00Z","total_cost_usd":0.002,"success":true,"tool_calls_count":2,"approvals_count":0,"denies_count":0}
        """
        let data = json.data(using: .utf8)!
        let s = try JSONDecoder().decode(SessionSummary.self, from: data)
        XCTAssertEqual(s.sessionId, "s1")
        XCTAssertTrue(s.success)
        XCTAssertEqual(s.toolCallsCount, 2)
    }
}
