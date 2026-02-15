//  SessionModels.swift
//  OxcerLauncher
//
//  Session-scoped types and view models. Ensures only one session's heavy data
//  (messages, sessionEvents) is in memory at a time.

import Foundation
import SwiftUI

// MARK: - Sidebar Session Item (Lightweight)

/// Lightweight representation for the sidebar sessions list.
/// Contains only metadata — no full message history or log text.
struct SidebarSessionItem: Identifiable {
    let id: String
    let title: String
    let createdAt: String
    let updatedAt: String
    /// Optional preview; backend may not provide. Empty string if unavailable.
    let lastMessagePreview: String

    /// Maps from FFI SessionSummary. Backend does not provide title or lastMessagePreview.
    static func from(_ s: SessionSummary) -> SidebarSessionItem {
        SidebarSessionItem(
            id: s.sessionId,
            title: shortSessionIdForDisplay(s.sessionId),
            createdAt: s.startTimestamp,
            updatedAt: s.endTimestamp,
            lastMessagePreview: ""
        )
    }
}

private func shortSessionIdForDisplay(_ id: String) -> String {
    if id.count <= 12 { return id }
    return String(id.prefix(6)) + "…" + String(id.suffix(4))
}

// MARK: - Session Detail View Model (Session Lifetime)

/// Owns heavy per-session data: messages and sessionEvents.
/// Only ONE instance should be alive at a time; switching sessions must release the previous one.
@MainActor
final class SessionDetailViewModel: ObservableObject {
    /// nil for new chat; non-nil when viewing a historical session.
    let sessionId: String?

    @Published private(set) var messages: [ChatMessage] = []
    @Published private(set) var sessionEvents: [LogEvent] = []
    @Published var isSessionEventsLoading: Bool = false

    private let maxMessagesCount = 500
    private let maxSessionEventsCount = 2000

    private var loadSessionLogTask: Task<Void, Never>?

    init(sessionId: String?) {
        self.sessionId = sessionId
    }

    /// Appends a message and trims to cap (keeps most recent).
    func appendMessage(_ message: ChatMessage) {
        messages.append(message)
        if messages.count > maxMessagesCount {
            messages.removeFirst(messages.count - maxMessagesCount)
        }
    }

    /// Loads session log from backend. Call when sessionId is non-nil.
    func loadSessionLog(sessionId: String, appConfigDir: String?, backend: OxcerBackend) {
        loadSessionLogTask?.cancel()
        guard let dir = appConfigDir else { return }
        isSessionEventsLoading = true
        loadSessionLogTask = Task {
            defer { isSessionEventsLoading = false }
            do {
                let events = try await backend.loadSessionLog(sessionId: sessionId, appConfigDir: dir)
                guard !Task.isCancelled else { return }
                sessionEvents = Array(events.prefix(maxSessionEventsCount))
            } catch {
                sessionEvents = []
            }
        }
    }

    /// Cancel any in-flight load. Call before releasing this view model.
    func cancelLoad() {
        loadSessionLogTask?.cancel()
    }
}
