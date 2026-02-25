//  SessionModels.swift
//  OxcerLauncher
//
//  Session model. ConversationSession owns per-session data and lifecycle.
//  AppViewModel creates and manages instances; views observe via @ObservedObject.

import Foundation
import SwiftUI

// MARK: - AgentPhase

/// Describes what the agent step loop is currently doing.
/// Updated by `AgentRunner` via `AppViewModel.runAgentRequest`'s `onPhaseChanged` callback.
/// Always resets to `.idle` in the defer block when a request finishes.
enum AgentPhase: Equatable {
    case idle
    case thinking
    case executingTool(name: String)

    /// Human-readable label shown in `AgentPhaseIndicator`.
    var displayLabel: String {
        switch self {
        case .idle:
            return ""
        case .thinking:
            return "Oxcer is thinking"
        case .executingTool(let name):
            switch name {
            case "fs_list_dir":   return "Listing files"
            case "fs_read_file":  return "Reading file"
            case "fs_write_file": return "Writing file"
            case "fs_delete":     return "Deleting file"
            case "fs_rename":     return "Renaming file"
            case "fs_move":       return "Moving file"
            case "shell_run":     return "Running command"
            case "llm_generate":  return "Generating"
            default:
                let pretty = name.replacingOccurrences(of: "_", with: " ")
                return "Running \(pretty)"
            }
        }
    }

    /// SF Symbols icon for the executing-tool state; nil for .thinking and .idle.
    var toolIconName: String? {
        guard case .executingTool(let name) = self else { return nil }
        switch name {
        case "fs_list_dir":               return "folder"
        case "fs_read_file":              return "doc.text"
        case "fs_write_file":             return "pencil"
        case "fs_delete":                 return "trash"
        case "fs_rename", "fs_move":      return "arrow.right.doc.on.clipboard"
        case "shell_run":                 return "terminal"
        case "llm_generate":              return "brain"
        default:                          return "bolt"
        }
    }
}

// MARK: - Conversation Session

/// Owns per-session state: messages, streaming buffer, approval gate, and the generation task handle.
///
/// Lifecycle contract:
///   1. AppViewModel creates an instance and appends it to `sessions`.
///   2. `runAgentRequest(session:taskText:)` sets `session.currentTask` and calls lifecycle methods.
///   3. All lifecycle methods must be called on the MainActor (enforced by @MainActor class).
///   4. `currentTask` is always set back to `nil` in a `defer` block — it never leaks.
@MainActor
final class ConversationSession: ObservableObject, Identifiable {
    let id: UUID
    let createdAt: Date

    /// Displayed in the sidebar. Auto-set from the first user message; explicitly set by renameSession.
    @Published private(set) var title: String

    /// Pinned sessions are sorted before all unpinned sessions in the sidebar.
    @Published var isPinned: Bool = false

    /// Updated whenever a message is appended. Used for sidebar sort order and the time label.
    @Published private(set) var lastUpdated: Date

    /// Ordered message history for the session (capped at maxMessagesCount).
    @Published private(set) var messages: [ChatMessage] = []

    /// Non-nil while the agent is generating a response.
    /// `""` (empty string) = thinking state (model hasn't produced text yet).
    /// Growing string = text being streamed into the bubble.
    @Published private(set) var streamingAnswer: String? = nil

    /// True from the moment generation begins until the task fully exits (including defer cleanup).
    /// Drives the Send/Stop toggle in ChatInputBar.
    @Published private(set) var isGenerating: Bool = false

    /// Non-nil while the agent step loop is suspended waiting for user approval of a tool call.
    /// AppViewModel writes this; the ApprovalOverlay reads it via SessionApprovalLayer.
    @Published var pendingApproval: ApprovalRequest?

    /// Current phase of the agent step loop. Updated by AgentRunner via onPhaseChanged;
    /// always reset to .idle in the defer block of AppViewModel.runAgentRequest.
    @Published private(set) var agentPhase: AgentPhase = .idle

    /// Handle to the in-flight generation task. Written and cancelled by AppViewModel.
    /// Not @Published — AppViewModel manages cancellation; views use `isGenerating` instead.
    var currentTask: Task<Void, Never>?

    private let maxMessagesCount = 500

    init(id: UUID = UUID(), title: String = "New Chat") {
        self.id = id
        self.title = title
        let now = Date()
        self.createdAt = now
        self.lastUpdated = now
    }

    // MARK: - Title

    /// Auto-title this session from the first user message. Subsequent calls are no-ops.
    func setTitleIfDefault(from text: String) {
        guard title == "New Chat" else { return }
        let preview = text.prefix(40)
        title = preview.count < text.count ? "\(preview)..." : String(preview)
    }

    /// Explicitly rename this session. Trims whitespace; ignores empty strings.
    func rename(to newTitle: String) {
        let trimmed = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        title = trimmed
    }

    // MARK: - Message history

    /// Appends a message, updates `lastUpdated`, and trims the history to the cap.
    func appendMessage(_ message: ChatMessage) {
        messages.append(message)
        lastUpdated = Date()
        if messages.count > maxMessagesCount {
            messages.removeFirst(messages.count - maxMessagesCount)
        }
    }

    // MARK: - Generation lifecycle

    /// Called by AppViewModel at the start and end of each generation run.
    func markGenerating(_ generating: Bool) {
        isGenerating = generating
    }

    // MARK: - Streaming helpers

    /// Enter the "thinking" state before the agent has produced any text.
    func beginStreaming() {
        streamingAnswer = ""
    }

    /// Append a text chunk to the in-progress assistant message.
    func appendStreaming(_ chunk: String) {
        streamingAnswer = (streamingAnswer ?? "") + chunk
    }

    /// Commit the streamed text as a permanent ChatMessage and exit streaming state.
    /// A no-op if there is nothing to commit (e.g. generation was cancelled before any text arrived).
    func finalizeStreaming() {
        guard let text = streamingAnswer else { return }
        streamingAnswer = nil
        let content = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !content.isEmpty {
            appendMessage(ChatMessage(id: UUID(), role: "assistant", content: content))
        }
    }

    /// Clear streaming state without adding a message.
    /// Use when the caller will append its own error message.
    func cancelStreaming() {
        streamingAnswer = nil
    }

    // MARK: - Phase

    /// Update the displayed agent phase. Always called on the MainActor via Task { @MainActor }.
    func updatePhase(_ phase: AgentPhase) {
        agentPhase = phase
    }
}
