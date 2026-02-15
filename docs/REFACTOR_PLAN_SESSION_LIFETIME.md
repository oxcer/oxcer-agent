# Refactor Plan: Session Lifetime Architecture

## Overview

Move heavy per-session data (messages, sessionEvents) out of AppViewModel into a session-scoped `SessionDetailViewModel`. Only one `SessionDetailViewModel` is alive at a time; switching sessions or "New Chat" releases the previous one.

---

## File Changes Summary

| File | Action |
|------|--------|
| `SessionModels.swift` | **NEW** — SidebarSessionItem, SessionDetailViewModel |
| `ContentView.swift` | **MODIFY** — AppViewModel, DetailView, ContentView body |
| `OxcerBackend.swift` | No change (FFI API unchanged) |
| `project.pbxproj` | **MODIFY** — Add SessionModels.swift to build |

---

## 1. SessionModels.swift (NEW)

**Location:** `apps/OxcerLauncher/OxcerLauncher/SessionModels.swift`

**Contents:**

### SidebarSessionItem (lightweight, for sidebar list)
- `id: String` — session identifier
- `title: String` — display title (derived from sessionId)
- `createdAt: String` — from FFI startTimestamp
- `updatedAt: String` — from FFI endTimestamp
- `lastMessagePreview: String?` — optional; FFI doesn't provide, use empty/nil

Mapped from FFI `SessionSummary` in `loadSessions()`.

### SessionDetailViewModel (session-scoped, owns heavy data)
- `sessionId: String?` — nil for new chat
- `@Published messages: [ChatMessage]`
- `@Published sessionEvents: [LogEvent]`
- `@Published isSessionEventsLoading: Bool`
- `maxMessagesCount`, `maxSessionEventsCount` (constants)
- `appendMessage(_:)`, `loadSessionLog(sessionId:appConfigDir:backend:)` 
- `backend: OxcerBackend`, `appConfigDir: String?` (injected for loading)

---

## 2. AppViewModel Refactor (ContentView.swift)

**Remove from AppViewModel:**
- `messages`
- `sessionEvents`
- `selectedSessionId` (derive from activeSessionDetail?.sessionId)
- `loadSessionLogTask`
- `appendMessage`, `maxMessagesCount`, `maxSessionEventsCount`

**Add to AppViewModel:**
- `activeSessionDetail: SessionDetailViewModel?` — only one alive at a time
- `sessions: [SidebarSessionItem]` — replaces `[SessionSummary]`, mapped from backend

**Flow changes:**
- `startNewChat()`: `activeSessionDetail = nil` (releases previous)
- `selectSession(id)`: `activeSessionDetail = nil`; create new `SessionDetailViewModel(sessionId: id)`; set `activeSessionDetail`; call `loadSessionLog` on it
- `sendMessage(text)`: if `activeSessionDetail == nil`, create new empty `SessionDetailViewModel(sessionId: nil)`; append user message; `runAgentRequest` appends response to `activeSessionDetail`
- `runAgentRequest`: append assistant message to `activeSessionDetail`
- `loadSessions`: map FFI `[SessionSummary]` → `[SidebarSessionItem]`

---

## 3. View Hierarchy (ContentView.swift)

**SidebarView:**
- `sessions: [SidebarSessionItem]`
- `selectedSessionId: String?` = `viewModel.activeSessionDetail?.sessionId`
- `selectSession`, `startNewChat` unchanged from caller's perspective

**DetailView:**
- When `activeSessionDetail == nil`: show `EmptyStateView` (welcome)
- When `activeSessionDetail != nil`: show `ChatDetailView(sessionDetail: activeSessionDetail!, ...)` 
- `ChatDetailView` reads `messages` and `sessionEvents` from `sessionDetail`, not from AppViewModel
- `onSend` routes to AppViewModel.sendMessage (which uses activeSessionDetail)

**ContentView body:**
- Pass `activeSessionDetail` to DetailView
- Pass `sessions` (SidebarSessionItem array)
- Pass `selectedSessionId` = `activeSessionDetail?.sessionId`

---

## 4. Data Flow

```
Backend.listSessions() → [SessionSummary] (FFI)
    → map to [SidebarSessionItem] → AppViewModel.sessions

Backend.loadSessionLog(sessionId) → [LogEvent]
    → SessionDetailViewModel.sessionEvents (not AppViewModel)

User sends message:
    AppViewModel.sendMessage → activeSessionDetail.appendMessage (user)
    → runAgentRequest → backend.runAgentTask
    → activeSessionDetail.appendMessage (assistant)
```

---

## 5. Memory Guarantees

- **App lifetime:** sessions (lightweight SidebarSessionItem), workspaces, backend, model ready state
- **Session lifetime:** ONE SessionDetailViewModel with messages + sessionEvents
- **On startNewChat:** `activeSessionDetail = nil` → previous VM deallocated
- **On selectSession:** `activeSessionDetail = nil` then new VM → previous VM deallocated
