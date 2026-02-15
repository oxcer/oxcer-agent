# SwiftUI View Hierarchy & Memory Analysis — OxcerLauncher

## 1. Main Window Structure & Lifetimes

```
OxcerLauncherApp
└── WindowGroup
    └── RootWindowContent (@AppStorage appTheme, .id(appTheme))
        └── RootView
            └── @StateObject AppViewModel  ← WINDOW LIFETIME
                └── ContentView (@ObservedObject viewModel)
                    └── ZStack
                        ├── NavigationSplitView
                        │   ├── SidebarView (sidebar)
                        │   │   └── List(sessions) | shimmer placeholders
                        │   └── DetailView (detail)
                        │       └── chatHistoryView | emptyStateView
                        │           └── LazyVStack { messages + sessionEvents }
                        └── ModelDownloadOverlay (when !isModelReady)
```

**Lifetime notes:**
- **RootWindowContent** uses `.id(appTheme)` → when theme changes, SwiftUI treats it as a new view identity. RootView and its `@StateObject` AppViewModel are discarded and recreated. Good: releases all ViewModel state on theme change.
- **AppViewModel** lives from RootView creation until RootView is destroyed (window close or theme change). Effectively **app/window lifetime**.
- **DefaultOxcerBackend** is created in `AppViewModel.init(backend: DefaultOxcerBackend())` — one per AppViewModel. No global singleton.
- **DetailView** is always in the hierarchy (NavigationSplitView detail pane). It never disappears; there is no "navigate away" that would release its state.

---

## 2. Large Collections at AppViewModel Level

| Collection | Max Size | Lifetime | Needed at app lifetime? |
|------------|----------|----------|--------------------------|
| **messages** | 500 | App | **No** — belongs to current chat only. Reset on new chat / session change. |
| **sessions** | 500 | App | **Partial** — sidebar needs a list. Could be reduced or made lighter. |
| **sessionEvents** | 2000 | App | **No** — only needed when a session is selected. Should be empty when in "New Chat". |
| **workspaces** | Small (~10s) | App | Yes — needed for workspace selector. |

### Bug: sessionEvents not cleared on New Chat

**File:** `ContentView.swift`, `startNewChat()` (line ~361) and `selectSession` (line ~395)

```swift
func startNewChat() {
    selectedSessionId = nil
    messages = []
    // sessionEvents is NOT cleared — holds up to 2000 LogEvent with details strings
}
```

When the user clicks "New Chat", `sessionEvents` retains the previously selected session’s log (up to 2000 `LogEvent` with `details: String?` JSON). This is both incorrect (showing another session’s log) and wasteful.

---

## 3. List / ScrollView / LazyVStack Usage

### 3.1 Sidebar: `List(sessions, selection:)`

**File:** `ContentView.swift`, SidebarView (lines 534–551)

- **Data:** `sessions` — up to 500 `SessionSummary`
- **Item size:** ~100–200 bytes (strings + numbers)
- **SwiftUI behavior:** `List` lazily materializes rows; off-screen rows are reused. Data array is fully in memory.
- **Identity:** `SessionSummary` is `Identifiable` via `sessionId`. Good for diffing.
- **Retention:** 500 `SessionListRow` view structs are not created at once; `List` recycles. The 500 `SessionSummary` values stay in `AppViewModel.sessions`.

### 3.2 Detail: `LazyVStack` with messages + sessionEvents

**File:** `ContentView.swift`, DetailView.chatHistoryView (lines 665–690)

```swift
LazyVStack(alignment: .leading, spacing: 16) {
    ForEach(messages) { msg in ... }           // up to 500
    if !sessionEvents.isEmpty {
        ForEach(sessionEvents, id: \.timestamp) { event in ... }  // up to 2000
    }
}
```

- **Data:** `messages` (≤500) + `sessionEvents` (≤2000) — up to 2500 items
- **Item size:** `ChatMessage` small; `LogEvent` larger due to `details: String?`
- **Identity issue:** `ForEach(sessionEvents, id: \.timestamp)` — timestamps can repeat. Non-unique ids hurt SwiftUI diffing and can cause extra churn/retention.
- **LazyVStack:** Only materializes visible rows; off-screen views are released. Data arrays stay in memory.
- **LogEvent.details:** Can hold large JSON strings. 2000 events × variable-size details → significant memory.

### 3.3 Combined retention

At steady state (session selected with full log):

- **In memory:** 500 messages + 2000 sessionEvents + 500 sessions ≈ 3000 collection elements
- **View hierarchy:** Only visible rows materialized (tens of views). Data remains in `AppViewModel` for the whole window lifetime.

---

## 4. Concrete Plan (Incremental)

### Phase A: Clear sessionEvents on New Chat (small, high impact)

**File:** `ContentView.swift`, `startNewChat()`

**Change:** Clear `sessionEvents` when starting a new chat.

```swift
func startNewChat() {
    selectedSessionId = nil
    messages = []
    sessionEvents = []   // ADD: release up to 2000 LogEvent immediately
}
```

**Impact:** When switching to New Chat, up to 2000 `LogEvent` (including `details`) are released. Also fixes the UX bug of showing another session’s log.

---

### Phase B: Don’t show session log in New Chat (logic fix)

**File:** `ContentView.swift`, DetailView.chatHistoryView

**Current:** Session Log is shown when `!sessionEvents.isEmpty`, regardless of `selectedSessionId`.

**Change:** Only show Session Log when a session is selected. Pass `selectedSessionId` into `DetailView` (or derive from a parent) and gate:

```swift
// In chatHistoryView, change:
if !sessionEvents.isEmpty && selectedSessionId != nil {
    // Session Log section
}
```

Or: after Phase A, `sessionEvents` will be empty in New Chat, so the section will not appear. Phase A alone may be enough; Phase B makes the intent explicit.

---

### Phase C: Fix ForEach identity for sessionEvents

**File:** `ContentView.swift`, line 680

**Current:** `ForEach(sessionEvents, id: \.timestamp)` — timestamps can collide.

**Change:** Use a stable unique id. Options:

1. If `LogEvent` can have a synthetic index: `ForEach(Array(sessionEvents.enumerated()), id: \.offset)` — but offset changes when array changes.
2. Create a composite: `ForEach(sessionEvents, id: \.self)` if `LogEvent` is `Hashable` (it is).
3. Or use `sessionId + timestamp + component + action` if that is unique.

`LogEvent` is `Hashable`. Using `id: \.self` is safe and avoids timestamp collisions. Slightly more expensive hashing, but acceptable for 2000 items.

```swift
ForEach(sessionEvents, id: \.self) { event in
    LogEventBubble(event: event)
}
```

---

### Phase D: Reduce sessions cap for sidebar (optional)

**File:** `ContentView.swift`, `maxSessionsCount = 500`

**Current:** 500 sessions in memory.

**Change:** Lower to 50–100 for the initial load. Add "Load more" or pagination if needed. Most users only need recent sessions.

```swift
private let maxSessionsCount = 100  // was 500
```

---

### Phase E: Move sessionEvents to screen/selection lifetime (larger refactor)

**Goal:** `sessionEvents` is only in memory while a session is selected and its detail is visible.

**Approach:** Introduce a `SessionDetailState` (or child ViewModel) owned by the detail content:

1. Create `SessionDetailViewModel: ObservableObject` holding `sessionEvents: [LogEvent]`.
2. Use `@StateObject` in a wrapper view that is only created when `selectedSessionId != nil`.
3. When `selectedSessionId` becomes `nil`, the wrapper is removed and `SessionDetailViewModel` is deallocated.

**Structure sketch:**

```swift
// New: SessionDetailContainer
struct SessionDetailContainer: View {
    let sessionId: String?
    let messages: [ChatMessage]
    ...
    
    var body: some View {
        if let id = sessionId {
            SessionDetailLoadedView(sessionId: id, messages: messages, ...)
                // This view owns @StateObject SessionDetailViewModel
                // Loads sessionEvents on appear, holds only for this session
        } else {
            NewChatDetailView(messages: messages, ...)
                // No sessionEvents at all
        }
    }
}
```

DetailView would use `SessionDetailContainer` instead of receiving `sessionEvents` from AppViewModel. `sessionEvents` would then live only in `SessionDetailViewModel` and be released when switching away.

---

### Phase F: Trim sessionEvents when window goes to background (optional)

**File:** `ContentView.swift` or `RootView`

**Change:** When `scenePhase == .background`, optionally clear or trim `sessionEvents` (e.g. keep last 100) to reduce memory while the app is backgrounded. Restore on return if the user was viewing that session (or reload on next `scenePhase == .active`).

This is more involved (reload logic, UX) and lower priority than Phases A–C.

---

## 5. Summary: Recommended Order

| Phase | Change | Effort | Impact |
|-------|--------|--------|--------|
| **A** | Clear `sessionEvents` in `startNewChat()` | 1 line | High — fixes leak + UX |
| **B** | Gate Session Log on `selectedSessionId` | Optional | Redundant if A done |
| **C** | `ForEach(sessionEvents, id: \.self)` | 1 line | Medium — better diffing |
| **D** | Lower `maxSessionsCount` to 100 | 1 line | Low–medium |
| **E** | SessionDetailViewModel / screen-scoped sessionEvents | Refactor | High — proper lifecycle |
| **F** | Trim on background | Optional | Medium |

Start with **Phase A** and **Phase C**. Add **Phase D** if 500 sessions is unnecessary. Consider **Phase E** if memory remains an issue after the smaller changes.
