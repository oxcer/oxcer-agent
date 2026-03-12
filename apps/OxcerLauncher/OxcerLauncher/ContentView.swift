//  ContentView.swift
//  OxcerLauncher
//
//  Refactored UI: ChatGPT-style layout with Toss-inspired dark theme

import AppKit
import SwiftUI

// MARK: - Theme (Toss-inspired)

/// OxcerTheme:
/// - Semantic colors from Assets (Light/Dark adaptive)
/// - accent, accentHover (brand colors, work in both modes)
/// - statusIdle, statusRunning, statusCompleted
/// - fastEaseOut (canonical simple transition animation)
struct OxcerTheme {
    // Semantic backgrounds (system Light/Dark aware)
    static let backgroundDark = Color("OxcerBackground")
    static let backgroundPanel = Color("OxcerPanel")
    static let sidebarBackground = Color("OxcerSurface")
    static let cardBackground = Color("OxcerCard")
    
    // Accent color for primary actions (brand blue)
    static let accent = Color(hex: "004CFF")
    static let accentHover = Color(hex: "4A7FFF")
    
    // Status colors
    static let statusIdle = Color("OxcerTextTertiary")
    static let statusRunning = accent
    static let statusCompleted = Color(hex: "00C853")
    
    // Text colors (semantic)
    static let textPrimary = Color("OxcerTextPrimary")
    static let textSecondary = Color("OxcerTextSecondary")
    static let textTertiary = Color("OxcerTextTertiary")
    static let textError = Color.red.opacity(0.9)
    
    // Borders and dividers (semantic)
    static let divider = Color("OxcerDivider")
    static let border = Color("OxcerBorder")
    static let hoverOverlay = Color("OxcerHoverOverlay")
    /// Text/icon on accent backgrounds (e.g. primary buttons) — white for visibility on blue.
    static let onAccent = Color("OxcerOnAccent")
    
    // Card styling
    static let cardCornerRadius: CGFloat = 12
    static let inputCornerRadius: CGFloat = 20
    
    // Animations: tuned springs for Toss-like motion.
    static let snappy: Animation = .spring(response: 0.3, dampingFraction: 0.7)
    static let bouncy: Animation = .spring(response: 0.4, dampingFraction: 0.6)
}

extension Color {
    init(hex: String) {
        let hex = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        var int: UInt64 = 0
        Scanner(string: hex).scanHexInt64(&int)
        let a, r, g, b: UInt64
        switch hex.count {
        case 3: // RGB (12-bit)
            (a, r, g, b) = (255, (int >> 8) * 17, (int >> 4 & 0xF) * 17, (int & 0xF) * 17)
        case 6: // RGB (24-bit)
            (a, r, g, b) = (255, int >> 16, int >> 8 & 0xFF, int & 0xFF)
        case 8: // ARGB (32-bit)
            (a, r, g, b) = (int >> 24, int >> 16 & 0xFF, int >> 8 & 0xFF, int & 0xFF)
        default:
            (a, r, g, b) = (255, 0, 0, 0)
        }
        self.init(
            .sRGB,
            red: Double(r) / 255,
            green: Double(g) / 255,
            blue: Double(b) / 255,
            opacity: Double(a) / 255
        )
    }
}

// MARK: - Global Motion & Micro-interactions

/// Bouncy, scalable button style used across the app.
/// Scales down slightly and dims while pressed, then springs back.
struct BouncyButtonStyle: ButtonStyle {
    var scale: CGFloat = 0.96
    var dimmingOpacity: Double = 0.18
    
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? scale : 1.0)
            .opacity(configuration.isPressed ? (1.0 - dimmingOpacity) : 1.0)
            .animation(OxcerTheme.bouncy, value: configuration.isPressed)
    }
}

/// Shimmer modifier for skeleton loading states.
/// Uses Task-driven animation (not repeatForever) so we can cancel when the view disappears.
/// Pauses animation when app is in background to avoid idle allocation.
struct ShimmerModifier: ViewModifier {
    @Environment(\.scenePhase) private var scenePhase
    @State private var phase: CGFloat = -0.6
    @State private var animationTask: Task<Void, Never>?

    func body(content: Content) -> some View {
        content
            .overlay(
                GeometryReader { proxy in
                    let width = proxy.size.width
                    let gradient = LinearGradient(
                        gradient: Gradient(stops: [
                            .init(color: .white.opacity(0.0), location: 0.0),
                            .init(color: .white.opacity(0.5), location: 0.5),
                            .init(color: .white.opacity(0.0), location: 1.0)
                        ]),
                        startPoint: .top,
                        endPoint: .bottom
                    )

                    Rectangle()
                        .fill(gradient)
                        .rotationEffect(.degrees(20))
                        .offset(x: width * phase)
                }
                .clipped()
            )
            .mask(content)
            .onAppear {
                startAnimationIfActive()
            }
            .onDisappear {
                animationTask?.cancel()
                animationTask = nil
            }
            .onChange(of: scenePhase) { newPhase in
                if newPhase != .active {
                    animationTask?.cancel()
                    animationTask = nil
                } else {
                    startAnimationIfActive()
                }
            }
    }

    private func startAnimationIfActive() {
        guard scenePhase == .active else { return }
        animationTask = Task { @MainActor in
            while !Task.isCancelled {
                withAnimation(.linear(duration: 1.1)) { phase = 1.4 }
                try? await Task.sleep(nanoseconds: 1_100_000_000)
                guard !Task.isCancelled else { break }
                phase = -0.6
                try? await Task.sleep(nanoseconds: 50_000_000)
            }
        }
    }
}

extension View {
    /// Applies a shimmer effect for skeleton loading.
    func shimmering() -> some View {
        modifier(ShimmerModifier())
    }
}

// MARK: - Message (for transactional chat display)

struct ChatMessage: Identifiable {
    let id: UUID
    let role: String // "user" | "assistant"
    let content: String
}

// MARK: - Approval Request

/// A pending tool-call approval that suspends the agent step loop.
///
/// Created by `AppViewModel.runAgentRequest` when `AgentRunner.onApprovalNeeded` fires.
/// The step loop remains suspended at a `CheckedContinuation` inside this struct until
/// the user calls `approve()` or `cancel()` from the UI.
///
/// `fileprivate init` ensures only `AppViewModel` (in this file) can create instances.
struct ApprovalRequest: Identifiable, Sendable {
    let id: UUID
    let requestId: String
    let summary: String
    private let continuation: CheckedContinuation<Bool, Never>

    fileprivate init(
        id: UUID,
        requestId: String,
        summary: String,
        continuation: CheckedContinuation<Bool, Never>
    ) {
        self.id = id
        self.requestId = requestId
        self.summary = summary
        self.continuation = continuation
    }

    /// Resume the step loop — allow the tool call to proceed.
    func approve() { continuation.resume(returning: true) }
    /// Resume the step loop — deny the tool call; the orchestrator receives an error.
    func cancel()  { continuation.resume(returning: false) }
}

// MARK: - Helpers

private func formatTimestamp(_ iso: String) -> String {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = formatter.date(from: iso) ?? ISO8601DateFormatter().date(from: iso) {
        let out = DateFormatter()
        out.dateStyle = .short
        out.timeStyle = .short
        return out.string(from: date)
    }
    return iso
}

/// Returns a human-readable, bucketed relative time string for display in the session sidebar.
///
/// Buckets (no per-second granularity):
///   < 60 s   → "just now"
///   < 60 min → "X min ago"
///   < 24 h   → "X hour(s) ago"
///   < 7 d    → "yesterday" | "X days ago"
///   < 30 d   → "X week(s) ago"
///   >= 30 d  → "X month(s) ago"
///
/// Pass `now` explicitly so callers inside TimelineView can use the timeline clock date.
private func relativeTimeDescription(from date: Date, now: Date = Date()) -> String {
    let elapsed = now.timeIntervalSince(date)
    guard elapsed >= 0 else { return "just now" }

    if elapsed < 60 { return "just now" }

    let cal = Calendar.current
    let components = cal.dateComponents([.minute, .hour, .day, .weekOfYear, .month], from: date, to: now)

    // Use month first so weeks don't over-count near the boundary.
    let months = max(components.month ?? 0, 0)
    if months >= 1 { return months == 1 ? "1 month ago" : "\(months) months ago" }

    let weeks = max(components.weekOfYear ?? 0, 0)
    if weeks >= 1 { return weeks == 1 ? "1 week ago" : "\(weeks) weeks ago" }

    let days = max(components.day ?? 0, 0)
    if days >= 1 { return days == 1 ? "yesterday" : "\(days) days ago" }

    let hours = max(components.hour ?? 0, 0)
    if hours >= 1 { return hours == 1 ? "1 hour ago" : "\(hours) hours ago" }

    let minutes = max(components.minute ?? 0, 1)
    return minutes == 1 ? "1 min ago" : "\(minutes) min ago"
}

// MARK: - Download Progress Throttling

/// Throttles download progress callbacks to avoid flooding the main queue during large downloads.
/// Forwards immediately when progress >= 1.0 (completion). Otherwise limits to ~4 updates/sec.
private final class DownloadProgressThrottler {
    private let lock = NSLock()
    private var lastUpdateTime: CFAbsoluteTime = 0
    private let interval: CFAbsoluteTime
    private let handler: (Double, String) -> Void

    init(interval: TimeInterval = 0.25, handler: @escaping (Double, String) -> Void) {
        self.interval = interval
        self.handler = handler
    }

    func forward(progress: Double, message: String) {
        lock.lock()
        let now = CFAbsoluteTimeGetCurrent()
        let shouldForward = progress >= 1.0 || (now - lastUpdateTime) >= interval
        if shouldForward {
            lastUpdateTime = now
        }
        lock.unlock()
        if shouldForward {
            handler(progress, message)
        }
    }
}

// MARK: - App View Model (single source of truth)

/// AppViewModel
/// App-lifetime state only. Per-session data lives in ConversationSession.
/// Multiple sessions can exist simultaneously; selectedSessionID tracks which is visible.
@MainActor
final class AppViewModel: ObservableObject {
    // Workspace state (app lifetime)
    @Published var workspaces: [WorkspaceInfo] = []
    @Published var selectedWorkspaceId: String?

    /// In-memory conversation sessions. The sidebar shows this array directly.
    @Published var sessions: [ConversationSession] = []

    /// ID of the currently visible session. nil = welcome/empty state.
    @Published var selectedSessionID: UUID?

    /// Convenience accessor for the currently visible session.
    var selectedSession: ConversationSession? {
        guard let id = selectedSessionID else { return nil }
        return sessions.first { $0.id == id }
    }

    // App config directory for FFI (set by root view onAppear)
    @Published var appConfigDir: String?

    /// Model download status for First Run installation wizard.
    @Published var isModelReady: Bool = false
    @Published var downloadProgress: Double = 0.0
    @Published var downloadMessage: String = "Initializing..."
    @Published var loadError: String?
    @Published var isRetrying: Bool = false

    // Backend service abstraction over OxcerFFI
    private let backend: OxcerBackend

    /// Ensures we only run the initial workspace load once.
    private var hasPerformedInitialLoad = false

    /// Process-lifetime flag: true once `ensureLocalModel` has completed successfully.
    ///
    /// INVARIANT — write ordering:
    ///   `hasAttemptedInit` is set to `true` only **after** `isModelReady = true` has already
    ///   been executed (see `checkAndPrepareModel`). This makes `hasAttemptedInit == true` a
    ///   safe proxy for "the model is ready in this process". Any newly-created `AppViewModel`
    ///   that reads `hasAttemptedInit == true` at init time may safely set its own
    ///   `isModelReady = true` without re-running setup.
    ///
    ///   Do NOT move or hoist the `hasAttemptedInit = true` write above `isModelReady = true`.
    ///   Doing so would allow a new AppViewModel's `init` to mark itself ready before the model
    ///   files are actually confirmed, enabling the chat UI prematurely.
    private static var hasAttemptedInit = false
    /// Serialises reads/writes of `hasAttemptedInit` across threads.
    private static let initLock = NSLock()
    private var isCheckingModel = false

    init(backend: OxcerBackend = DefaultOxcerBackend()) {
        self.backend = backend
        self.appConfigDir = Self.defaultAppConfigDir()
        // Restore model-ready state immediately if a previous AppViewModel instance already
        // completed setup. Prevents the onboarding overlay from flashing on window restore.
        // hasAttemptedInit is only set to true AFTER isModelReady = true, so this is safe.
        AppViewModel.initLock.lock()
        let alreadyReady = AppViewModel.hasAttemptedInit
        AppViewModel.initLock.unlock()
        if alreadyReady { isModelReady = true }
    }

    private static func defaultAppConfigDir() -> String? {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("Oxcer").path
    }

    /// Model preparation + initial data load. Call from .task or Retry button.
    /// Safe to retry after failure; idempotent when already ready.
    /// isModelReady flips to true only after ensureLocalModel confirms file integrity (lazy engine load).
    func checkAndPrepareModel() async {
        if isModelReady { return }

        AppViewModel.initLock.lock()
        if AppViewModel.hasAttemptedInit {
            AppViewModel.initLock.unlock()
            // Model was already prepared; init should have restored isModelReady, but set it
            // here too in case checkAndPrepareModel is called before init's guard runs.
            if !isModelReady { isModelReady = true }
            return
        }
        AppViewModel.initLock.unlock()

        if isCheckingModel { return }
        isCheckingModel = true
        defer { isCheckingModel = false }

        loadError = nil
        isRetrying = true
        downloadMessage = "Initializing..."

        guard let dir = appConfigDir else {
            loadError = "Configuration directory not available."
            isRetrying = false
            return
        }

        do {
            let throttler = DownloadProgressThrottler(interval: 0.25) { [weak self] progress, message in
                DispatchQueue.main.async {
                    self?.downloadProgress = progress
                    self?.downloadMessage = message
                }
            }
            try await backend.ensureLocalModel(appConfigDir: dir, onProgress: throttler.forward)
            // ORDERING INVARIANT: isModelReady must be set to true BEFORE hasAttemptedInit.
            // AppViewModel.init reads hasAttemptedInit and uses it as a proxy for "model ready".
            // Reversing this order would let new instances mark themselves ready prematurely.
            isModelReady = true
            isRetrying = false
            AppViewModel.hasAttemptedInit = true

            if !hasPerformedInitialLoad {
                hasPerformedInitialLoad = true
                await loadWorkspaces()
            }
        } catch {
            loadError = error.localizedDescription
            downloadMessage = "Critical error during setup."
            isRetrying = false
        }
    }

    // MARK: - Session management

    /// Create a fresh session, add it to the list sorted by recency, and select it.
    @discardableResult
    func createNewSession() -> ConversationSession {
        let session = ConversationSession()
        sessions.append(session)
        sortSessions()
        selectedSessionID = session.id
        return session
    }

    /// Select an existing session by value.
    func selectSession(_ session: ConversationSession) {
        selectedSessionID = session.id
    }

    /// Send a message in the currently selected session (creating one if the list is empty).
    func sendMessage(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        let session: ConversationSession
        if let existing = selectedSession {
            session = existing
        } else {
            session = createNewSession()
        }
        sendMessage(in: session, text: trimmed)
    }

    /// Append a user message to the given session and kick off agent generation.
    func sendMessage(in session: ConversationSession, text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        session.setTitleIfDefault(from: trimmed)
        session.appendMessage(ChatMessage(id: UUID(), role: "user", content: trimmed))
        sortSessions()
        session.currentTask = Task { await runAgentRequest(session: session, taskText: trimmed) }
    }

    // MARK: - Session list mutations

    /// Rename a session to an explicit title, overriding any auto-generated one.
    func renameSession(_ session: ConversationSession, to newTitle: String) {
        session.rename(to: newTitle)
    }

    /// Toggle the pinned state and re-sort so pinned sessions always appear first.
    func togglePin(for session: ConversationSession) {
        session.isPinned.toggle()
        sortSessions()
    }

    /// Remove a session. Stops any in-flight generation and advances the selection.
    func deleteSession(_ session: ConversationSession) {
        // Stop generation before removing so the continuation is not leaked.
        stopGeneration(for: session)

        // Advance selection before removal so index lookup is still valid.
        if selectedSessionID == session.id {
            if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
                if sessions.count > 1 {
                    // Prefer the item that will be visually below after deletion.
                    let newIdx = idx + 1 < sessions.count ? idx + 1 : idx - 1
                    selectedSessionID = sessions[newIdx].id
                } else {
                    selectedSessionID = nil
                }
            } else {
                selectedSessionID = nil
            }
        }

        sessions.removeAll { $0.id == session.id }
    }

    /// Sort sessions: pinned first, then by lastUpdated descending within each group.
    private func sortSessions() {
        sessions.sort {
            if $0.isPinned != $1.isPinned { return $0.isPinned }
            return $0.lastUpdated > $1.lastUpdated
        }
    }

    /// Cancel the in-flight generation task for the currently selected session.
    func stopGeneration() {
        guard let session = selectedSession else { return }
        stopGeneration(for: session)
    }

    /// Cancel the in-flight generation task for a specific session.
    ///
    /// Dismisses any pending approval first so the suspended CheckedContinuation is
    /// resumed before the task cancellation signal is sent — prevents a continuation leak.
    func stopGeneration(for session: ConversationSession) {
        if let pending = session.pendingApproval {
            session.pendingApproval = nil
            pending.cancel()
        }
        session.currentTask?.cancel()
        session.currentTask = nil
    }

    /// Called by the approval overlay when the user taps Approve (Return) or Cancel (Escape).
    ///
    /// Clears `pendingApproval` first so the overlay disappears immediately, then resumes
    /// the suspended `CheckedContinuation` to let the step loop continue.
    func respondToApproval(_ approved: Bool) {
        guard let session = selectedSession,
              let approval = session.pendingApproval else { return }
        session.pendingApproval = nil
        if approved { approval.approve() } else { approval.cancel() }
    }

    // MARK: - Data loading (FFI via backend service)

    /// Loads workspaces from backend (smoke test / initial setup).
    func loadWorkspaces() async {
        do {
            let result = try await backend.listWorkspaces(appConfigDir: "")
            print("FFI SMOKE TEST SUCCESS: \(result)")
        } catch {
            // Non-chat error; could log or show in UI
        }
    }

    // MARK: - Agent execution

    /// Runs the agent request end-to-end via AgentRunner (ffi_agent_step + SwiftAgentExecutor loop).
    ///
    /// IMPORTANT: Does NOT call ensureLocalModel — the model must already be ready
    /// (gated by checkAndPrepareModel + isModelReady guard in ContentView).
    ///
    /// All step-loop logic lives in AgentRunner. This method only:
    ///   1. Builds AgentEnvironment from current workspace state.
    ///   2. Calls AgentRunner.run(env:) and streams the final answer into the session.
    ///   3. Handles cancellation by committing partial text; real errors append an error message.
    func runAgentRequest(session: ConversationSession, taskText: String) async {
        let task = taskText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !task.isEmpty else { return }
        session.markGenerating(true)
        defer {
            session.currentTask = nil
            // Clear streaming state on any exit path (no-op if already finalized).
            session.cancelStreaming()
            // Dismiss any pending approval so the CheckedContinuation is not leaked.
            if let pending = session.pendingApproval {
                session.pendingApproval = nil
                pending.cancel()
            }
            // Reset agent phase so no stale label persists after the run.
            session.updatePhase(.idle)
            session.markGenerating(false)
            // Re-sort after generation ends so the session's updated lastUpdated is reflected.
            sortSessions()
        }

        let env = AgentEnvironment(
            taskDescription: task,
            workspaceId: selectedWorkspaceId,
            workspaceRoot: workspaces.first(where: { $0.id == selectedWorkspaceId })?.rootPath,
            appConfigDir: appConfigDir
        )

        // Show "thinking" indicator immediately.
        session.beginStreaming()

        do {
            // Select backend-specific parameters (timeout, maxSteps, temperature).
            let config = ModelBackendConfig.current()
            var executor = SwiftAgentExecutor()
            executor.config = config

            // Wire the approval gate: AgentRunner suspends here whenever a filesystem or
            // shell tool needs consent. The closure hops to the main actor to set
            // session.pendingApproval (showing the ApprovalOverlay), then suspends via a
            // CheckedContinuation until respondToApproval() is called from the UI.
            var runner = AgentRunner(backend: backend, executor: executor)
            runner.maxSteps = config.maxSteps
            runner.config = config
            // Propagate phase changes from the step loop to the session on the main actor.
            runner.onPhaseChanged = { [weak session] phase in
                Task { @MainActor [weak session] in
                    session?.updatePhase(phase)
                }
            }
            runner.onApprovalNeeded = { [weak session] requestId, summary in
                guard let session else { return false }
                return await withCheckedContinuation { continuation in
                    Task { @MainActor [weak session] in
                        guard let session else { continuation.resume(returning: false); return }
                        session.pendingApproval = ApprovalRequest(
                            id: UUID(),
                            requestId: requestId,
                            summary: summary,
                            continuation: continuation
                        )
                    }
                }
            }
            let answer = try await runner.run(env: env)
            let text = answer.isEmpty
                ? SwiftAgentExecutor.makeModelErrorMessage("empty response")
                : answer
            await streamReveal(text: text, session: session)
            session.finalizeStreaming()
        } catch is CancellationError {
            // User pressed Stop. Commit any partial text that was already revealed;
            // if the model had not yet produced any output finalizeStreaming is a silent no-op.
            session.finalizeStreaming()
        } catch {
            session.cancelStreaming()
            session.appendMessage(
                ChatMessage(id: UUID(), role: "assistant",
                            content: SwiftAgentExecutor.makeModelErrorMessage(error.localizedDescription)))
        }
    }

    /// Simulates token-by-token streaming by appending 30-character chunks with 15 ms delays.
    /// Produces a smooth reveal (~8-16 words/sec) without requiring server-sent events.
    private func streamReveal(text: String, session: ConversationSession) async {
        let chunkSize = 30
        var idx = text.startIndex
        while idx < text.endIndex {
            guard !Task.isCancelled else { break }
            let end = text.index(idx, offsetBy: chunkSize, limitedBy: text.endIndex) ?? text.endIndex
            session.appendStreaming(String(text[idx..<end]))
            idx = end
            if idx < text.endIndex {
                try? await Task.sleep(nanoseconds: 15_000_000) // 15 ms
            }
        }
    }
}

// MARK: - Sidebar View (ChatGPT-style)
// Receives data and closures directly to avoid computed view model re-creation.

struct SidebarView: View {
    let sessions: [ConversationSession]
    let selectedSessionId: UUID?
    let createNewSession: () -> Void
    let selectSession: (ConversationSession) -> Void
    let renameSession: (ConversationSession, String) -> Void
    let togglePin: (ConversationSession) -> Void
    let deleteSession: (ConversationSession) -> Void

    var body: some View {
        VStack(spacing: 0) {
            // New Chat button (top)
            Button {
                createNewSession()
            } label: {
                HStack(spacing: 10) {
                    Image(systemName: "square.and.pencil")
                        .font(.system(.body, weight: .medium))
                    Text("New Chat")
                        .font(.system(.body, weight: .medium))
                }
                .foregroundStyle(OxcerTheme.textPrimary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .background(
                    RoundedRectangle(cornerRadius: 10)
                        .fill(OxcerTheme.accent.opacity(0.15))
                )
                .contentShape(Rectangle())
            }
            .buttonStyle(BouncyButtonStyle())
            .padding(.horizontal, 12)
            .padding(.top, 12)
            .padding(.bottom, 8)

            // Sessions list (scrollable)
            List(sessions, selection: Binding(
                get: { selectedSessionId.flatMap { Set([$0]) } ?? [] },
                set: { ids in
                    if let id = ids.first,
                       let session = sessions.first(where: { $0.id == id }) {
                        selectSession(session)
                    } else {
                        createNewSession()
                    }
                }
            )) { session in
                SessionListRow(
                    session: session,
                    onCommitRename: { newTitle in renameSession(session, newTitle) },
                    onTogglePin: { togglePin(session) },
                    onDelete: { deleteSession(session) }
                )
                .tag(session.id)
            }
            .listStyle(.sidebar)
            .scrollContentBackground(.hidden)
            .background(Color.clear)

            Spacer(minLength: 0)

            // Bottom: User profile + Settings
            VStack(spacing: 0) {
                Divider()
                    .background(OxcerTheme.divider)

                HStack(spacing: 12) {
                    Circle()
                        .fill(OxcerTheme.accent.opacity(0.3))
                        .frame(width: 32, height: 32)
                        .overlay(
                            Text("O")
                                .font(.system(.subheadline, weight: .semibold))
                                .foregroundStyle(OxcerTheme.accent)
                        )

                    Text("Oxcer User")
                        .font(.system(.subheadline))
                        .foregroundStyle(OxcerTheme.textPrimary)
                        .lineLimit(1)
                        .truncationMode(.tail)

                    Spacer(minLength: 0)

                    SettingsLink {
                        Image(systemName: "gearshape")
                            .font(.system(.body))
                            .foregroundStyle(OxcerTheme.textSecondary)
                    }
                    .buttonStyle(BouncyButtonStyle(scale: 0.92, dimmingOpacity: 0.2))
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            }
            .background(OxcerTheme.sidebarBackground)
        }
        .frame(minWidth: 220)
        .background(OxcerTheme.sidebarBackground)
    }
}

/// List row for a conversation session in the sidebar.
///
/// Features:
///   - Context menu: Rename, Pin/Unpin, Delete.
///   - Inline rename: tapping "Rename" replaces the title Text with a TextField.
///     Pressing Return commits; pressing Escape cancels.
///   - Bucketed time label driven by a per-minute TimelineView — no per-second updates.
///   - Pin indicator and generating dot reflect live session state.
private struct SessionListRow: View {
    @ObservedObject var session: ConversationSession

    let onCommitRename: (String) -> Void
    let onTogglePin: () -> Void
    let onDelete: () -> Void

    @State private var isEditingTitle = false
    @State private var editingText = ""
    @FocusState private var fieldFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            // Title row: TextField when renaming, Text otherwise.
            HStack(spacing: 4) {
                if isEditingTitle {
                    TextField("Session title", text: $editingText)
                        .textFieldStyle(.plain)
                        .font(.system(.subheadline))
                        .foregroundStyle(OxcerTheme.textPrimary)
                        .focused($fieldFocused)
                        .onSubmit { commitRename() }
                        .onExitCommand { cancelRename() }
                } else {
                    Text(session.title)
                        .font(.system(.subheadline))
                        .foregroundStyle(OxcerTheme.textPrimary)
                        .lineLimit(1)
                }

                Spacer(minLength: 0)

                if session.isPinned {
                    Image(systemName: "pin.fill")
                        .font(.system(.caption2))
                        .foregroundStyle(OxcerTheme.accent.opacity(0.7))
                }
                if session.isGenerating {
                    Circle()
                        .fill(OxcerTheme.statusRunning)
                        .frame(width: 6, height: 6)
                }
            }

            // Time label — driven by a per-minute timeline so it never updates at per-second
            // resolution. The label also refreshes automatically when `session.lastUpdated`
            // changes (new message appended), because @ObservedObject re-renders the whole row.
            TimelineView(.everyMinute) { context in
                Text(relativeTimeDescription(from: session.lastUpdated, now: context.date))
                    .font(.system(.caption2))
                    .foregroundStyle(OxcerTheme.textTertiary)
                    .lineLimit(1)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 6)
        .padding(.horizontal, 4)
        .contextMenu {
            Button("Rename") { beginRename() }

            Button(session.isPinned ? "Unpin" : "Pin to Top") { onTogglePin() }

            Divider()

            Button("Delete", role: .destructive) { onDelete() }
        }
        .onChange(of: isEditingTitle) { _, editing in
            if editing { fieldFocused = true }
        }
    }

    private func beginRename() {
        editingText = session.title
        isEditingTitle = true
    }

    private func commitRename() {
        onCommitRename(editingText)
        isEditingTitle = false
    }

    private func cancelRename() {
        isEditingTitle = false
    }
}

// MARK: - Detail View (Chat History / Empty State)

/// Thin dispatcher: shows the empty/welcome state when no session is selected, otherwise
/// delegates to ActiveDetailView which observes the session directly.
///
/// WHY the split exists:
///   DetailView receives `session: ConversationSession?` as a plain `let` from ContentView.
///   A plain `let` on a SwiftUI struct does NOT subscribe to ObservableObject changes.
///   If ChatInputBar were rendered here, `isRunning: session?.isGenerating` would never
///   update when the session's isGenerating flips — the Stop button would never appear.
///   ActiveDetailView holds the session via @ObservedObject, so it re-renders on every
///   `session.objectWillChange` event (isGenerating, streamingAnswer, etc.).
struct DetailView: View {
    let session: ConversationSession?
    let onSend: (String) -> Void
    let onStop: () -> Void

    var body: some View {
        if let session = session {
            ActiveDetailView(session: session, onSend: onSend, onStop: onStop)
        } else {
            emptyStateView
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color("OxcerBackground"))
        }
    }

    private var emptyStateView: some View {
        VStack(spacing: 16) {
            Image(systemName: "sparkles")
                .font(.system(size: 56))
                .foregroundStyle(OxcerTheme.accent.opacity(0.8))
            Text("How can I help you?")
                .font(.system(.title2, design: .rounded, weight: .medium))
                .foregroundStyle(OxcerTheme.textSecondary)
        }
    }
}

/// Renders the chat content and input bar for one active session.
///
/// @ObservedObject ensures this view re-renders whenever session publishes —
/// including isGenerating (Stop/Send icon), streamingAnswer (live text), and
/// pendingApproval (approval overlay gating). No parent re-render is needed.
private struct ActiveDetailView: View {
    @ObservedObject var session: ConversationSession
    let onSend: (String) -> Void
    let onStop: () -> Void

    var body: some View {
        ZStack(alignment: .bottom) {
            ChatDetailContent(session: session)

            ChatInputBar(
                isRunning: session.isGenerating,
                onSend: onSend,
                onStop: onStop
            )
            .padding(.horizontal, 24)
            .padding(.bottom, 24)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color("OxcerBackground"))
    }
}

/// Observes a ConversationSession so it re-renders when messages or streamingAnswer change.
private struct ChatDetailContent: View {
    @ObservedObject var session: ConversationSession

    var body: some View {
        ZStack(alignment: .bottom) {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 16) {
                        ForEach(session.messages) { msg in
                            if msg.role == "user" {
                                TaskBubble(text: msg.content)
                            } else {
                                AssistantBubble(text: msg.content, isError: msg.content.hasPrefix("Error:"))
                            }
                        }

                        // Live in-progress assistant message (thinking/executing or streaming).
                        if let streaming = session.streamingAnswer {
                            Group {
                                if streaming.isEmpty {
                                    AgentPhaseIndicator(phase: session.agentPhase)
                                } else {
                                    AssistantBubble(text: streaming, isError: false)
                                }
                            }
                            .id("streaming_anchor")
                        }

                        Spacer(minLength: 100)
                            .id("scroll_bottom")
                    }
                    .padding(.horizontal, 24)
                    .padding(.vertical, 24)
                }
                .onChange(of: session.messages.count) { _ in
                    withAnimation(OxcerTheme.snappy) {
                        proxy.scrollTo("scroll_bottom", anchor: .bottom)
                    }
                }
                .onChange(of: session.streamingAnswer) { newValue in
                    guard newValue != nil else { return }
                    proxy.scrollTo("streaming_anchor", anchor: .bottom)
                }
            }

            // Fade-to-background gradient that hides message text sliding under the input bar.
            // allowsHitTesting(false) ensures scrolling and tap events pass through to the list.
            LinearGradient(
                colors: [Color("OxcerBackground").opacity(0), Color("OxcerBackground")],
                startPoint: .top,
                endPoint: .bottom
            )
            .frame(height: 80)
            .allowsHitTesting(false)
        }
    }
}

enum TaskStatus {
    case idle
    case running
    case completed
}

struct TaskStatusBadge: View {
    let status: TaskStatus
    
    var statusText: String {
        switch status {
        case .idle: return "Idle"
        case .running: return "Running"
        case .completed: return "Completed"
        }
    }
    
    var statusColor: Color {
        switch status {
        case .idle: return OxcerTheme.statusIdle
        case .running: return OxcerTheme.statusRunning
        case .completed: return OxcerTheme.statusCompleted
        }
    }
    
    var body: some View {
        Text(statusText)
            .font(.system(.caption2, weight: .medium))
            .foregroundStyle(statusColor)
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(
                Capsule()
                    .fill(statusColor.opacity(0.15))
            )
    }
}

struct TaskBubble: View {
    let text: String
    
    var body: some View {
        HStack {
            Text(text)
                .font(.system(.body))
                .foregroundStyle(OxcerTheme.textPrimary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(16)
        .background(
            RoundedRectangle(cornerRadius: OxcerTheme.cardCornerRadius)
                .fill(OxcerTheme.cardBackground)
                .overlay(
                    RoundedRectangle(cornerRadius: OxcerTheme.cardCornerRadius)
                        .stroke(OxcerTheme.border, lineWidth: 1)
                )
        )
    }
}

/// Assistant message bubble — no card background, text sits directly on the view background.
/// Renders markdown-aware blocks: plain paragraphs as `Text`, fenced code blocks as `CodeBlockView`.
/// Shows a "Copy all" button when the user hovers over the bubble (in addition to per-block copy).
struct AssistantBubble: View {
    let text: String
    let isError: Bool

    @State private var isHovering = false
    @State private var showCopied = false

    private var blocks: [AssistantBlock] { parseMarkdownBlocks(text) }

    @ViewBuilder
    private func blockView(_ block: AssistantBlock) -> some View {
        switch block {
        case .paragraph(let content):
            Text(content)
                .font(.system(.body))
                .foregroundStyle(OxcerTheme.textPrimary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .code(let language, let source):
            CodeBlockView(language: language, source: source)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if isError {
                Text(text)
                    .font(.system(.body))
                    .foregroundStyle(OxcerTheme.textError)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                ForEach(blocks) { block in
                    blockView(block)
                }
            }

            // "Copy all" action row — visible on hover or during the "Copied" flash.
            // Copies the raw text (all blocks); per-block copy is on CodeBlockView's header.
            if !isError && (isHovering || showCopied) {
                HStack {
                    Button {
                        copyToClipboard()
                    } label: {
                        HStack(spacing: 4) {
                            Image(systemName: showCopied ? "checkmark" : "doc.on.doc")
                                .font(.system(.caption))
                            Text(showCopied ? "Copied" : "Copy all")
                                .font(.system(.caption2, weight: .medium))
                        }
                        .foregroundStyle(
                            showCopied ? OxcerTheme.statusCompleted : OxcerTheme.textTertiary
                        )
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(
                            RoundedRectangle(cornerRadius: 6)
                                .fill(OxcerTheme.cardBackground)
                                .overlay(
                                    RoundedRectangle(cornerRadius: 6)
                                        .stroke(OxcerTheme.border, lineWidth: 1)
                                )
                        )
                    }
                    .buttonStyle(BouncyButtonStyle(scale: 0.93, dimmingOpacity: 0.15))

                    Spacer()
                }
                .transition(.opacity.animation(.easeInOut(duration: 0.15)))
            }
        }
        .padding(.vertical, 4)
        .onHover { hovering in
            withAnimation(.easeInOut(duration: 0.15)) {
                isHovering = hovering
            }
        }
    }

    private func copyToClipboard() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        withAnimation(OxcerTheme.snappy) { showCopied = true }
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            withAnimation(OxcerTheme.snappy) { showCopied = false }
        }
    }
}

/// Animated "Oxcer is thinking…" dots shown while the agent is running but hasn't produced text yet.
struct ThinkingIndicator: View {
    @State private var dotCount = 0

    var body: some View {
        HStack(spacing: 6) {
            Text("Oxcer is thinking")
                .font(.system(.body))
                .foregroundStyle(OxcerTheme.textTertiary)
            HStack(spacing: 3) {
                ForEach(0..<3, id: \.self) { i in
                    Circle()
                        .fill(OxcerTheme.textTertiary)
                        .frame(width: 4, height: 4)
                        .opacity(i < dotCount ? 1.0 : 0.25)
                        .animation(.easeInOut(duration: 0.2), value: dotCount)
                }
            }
        }
        .padding(.vertical, 4)
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 400_000_000)
                dotCount = (dotCount % 3) + 1
            }
        }
    }
}

/// Context-aware phase indicator shown while `streamingAnswer` is empty.
///
/// Primary line: generic "Thinking…" + animated dots, optionally preceded by a tool icon.
/// Secondary line (optional): a specific operation label (e.g. "Summarizing your document",
///   "Reading file") shown only when the intent is clearly known. Absent for plain chat.
struct AgentPhaseIndicator: View {
    let phase: AgentPhase

    @State private var dotCount = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            // Primary row: optional tool icon + "Thinking…" + animated dots.
            HStack(spacing: 6) {
                if let iconName = phase.toolIconName {
                    Image(systemName: iconName)
                        .font(.system(.caption, weight: .medium))
                        .foregroundStyle(OxcerTheme.accent)
                }

                Text(phase.displayLabel.isEmpty ? "Thinking…" : phase.displayLabel)
                    .font(.system(.body))
                    .foregroundStyle(OxcerTheme.textTertiary)

                HStack(spacing: 3) {
                    ForEach(0..<3, id: \.self) { i in
                        Circle()
                            .fill(OxcerTheme.textTertiary)
                            .frame(width: 4, height: 4)
                            .opacity(i < dotCount ? 1.0 : 0.25)
                            .animation(.easeInOut(duration: 0.2), value: dotCount)
                    }
                }
            }

            // Secondary row: shown only when the specific operation is identified.
            if let subtext = phase.subtextLabel {
                Text(subtext)
                    .font(.system(.caption))
                    .foregroundStyle(OxcerTheme.textTertiary.opacity(0.65))
            }
        }
        .padding(.vertical, 4)
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 400_000_000)
                dotCount = (dotCount % 3) + 1
            }
        }
    }
}

// MARK: - Approval Bubble

/// Card UI for an approval request. Keyboard shortcuts are handled by ApprovalOverlay.
struct ApprovalBubble: View {
    let request: ApprovalRequest
    let onRespond: (Bool) -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "lock.shield")
                .font(.system(.title3, weight: .medium))
                .foregroundStyle(OxcerTheme.accent)
                .frame(width: 28, height: 28)
                .padding(.top, 2)

            VStack(alignment: .leading, spacing: 10) {
                Text(request.summary)
                    .font(.system(.body))
                    .foregroundStyle(OxcerTheme.textPrimary)
                    .fixedSize(horizontal: false, vertical: true)

                Text("↩ Return to approve · ⎋ Escape to cancel")
                    .font(.system(.caption))
                    .foregroundStyle(OxcerTheme.textTertiary)

                HStack(spacing: 8) {
                    Button("Approve") { onRespond(true) }
                        .buttonStyle(ApprovalApproveButtonStyle())

                    Button("Cancel") { onRespond(false) }
                        .buttonStyle(ApprovalCancelButtonStyle())
                }
            }
        }
        .padding(16)
        .background(
            RoundedRectangle(cornerRadius: OxcerTheme.cardCornerRadius)
                .fill(OxcerTheme.accent.opacity(0.08))
                .overlay(
                    RoundedRectangle(cornerRadius: OxcerTheme.cardCornerRadius)
                        .stroke(OxcerTheme.accent.opacity(0.3), lineWidth: 1)
                )
        )
    }
}

// MARK: - Session Approval Layer

/// Renders the approval overlay if the observed session has a pending approval request.
/// Split into its own view so @ObservedObject can subscribe to session changes without
/// making ContentView itself re-render on every streaming update or session event.
private struct SessionApprovalLayer: View {
    @ObservedObject var session: ConversationSession
    let onRespond: (Bool) -> Void

    var body: some View {
        if let approval = session.pendingApproval {
            ApprovalOverlay(request: approval, onRespond: onRespond)
                .transition(.opacity.animation(OxcerTheme.snappy))
        }
    }
}

// MARK: - Approval Overlay

/// Full-screen overlay rendered from the root ContentView ZStack.
/// Directly observes `AppViewModel.pendingApproval` — no struct prop passing — so
/// no sub-tree re-render (streaming updates, isTaskRunning changes) can hide it.
/// Return approves; Escape cancels. Focus is claimed on appear.
private struct ApprovalOverlay: View {
    let request: ApprovalRequest
    let onRespond: (Bool) -> Void

    @FocusState private var isFocused: Bool

    var body: some View {
        ZStack(alignment: .bottom) {
            // Dimming backdrop — blocks tap-through (explicit button choice required).
            Color.black.opacity(0.35)
                .ignoresSafeArea()
                .contentShape(Rectangle())

            ApprovalBubble(request: request, onRespond: onRespond)
                .padding(.horizontal, 24)
                .padding(.bottom, 24)
        }
        .focusable()
        .focused($isFocused)
        .onKeyPress(.return) {
            onRespond(true)
            return .handled
        }
        .onKeyPress(.escape) {
            onRespond(false)
            return .handled
        }
        .onAppear { isFocused = true }
    }
}

private struct ApprovalApproveButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(.callout, weight: .medium))
            .foregroundStyle(OxcerTheme.onAccent)
            .padding(.horizontal, 16)
            .padding(.vertical, 7)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(OxcerTheme.accent.opacity(configuration.isPressed ? 0.75 : 1.0))
            )
            .scaleEffect(configuration.isPressed ? 0.97 : 1.0)
            .animation(OxcerTheme.snappy, value: configuration.isPressed)
    }
}

private struct ApprovalCancelButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(.callout, weight: .medium))
            .foregroundStyle(OxcerTheme.textSecondary)
            .padding(.horizontal, 16)
            .padding(.vertical, 7)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(OxcerTheme.cardBackground.opacity(configuration.isPressed ? 0.7 : 1.0))
                    .overlay(
                        RoundedRectangle(cornerRadius: 8)
                            .stroke(OxcerTheme.border, lineWidth: 1)
                    )
            )
            .scaleEffect(configuration.isPressed ? 0.97 : 1.0)
            .animation(OxcerTheme.snappy, value: configuration.isPressed)
    }
}

struct LogEventBubble: View {
    let event: LogEvent
    
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("\(event.component) · \(event.action)")
                .font(.system(.caption, weight: .medium))
                .foregroundStyle(OxcerTheme.textSecondary)
            Text(event.timestamp)
                .font(.system(.caption2, design: .monospaced))
                .foregroundStyle(OxcerTheme.textTertiary)
        }
        .padding(12)
        .background(
            RoundedRectangle(cornerRadius: 8)
                .fill(OxcerTheme.cardBackground.opacity(0.5))
        )
    }
}

// MARK: - Event detail (JSON details as monospaced string)

struct EventDetailView: View {
    /// Raw JSON string from LogEvent.details (UniFFI exposes details as String?).
    let details: String?

    var body: some View {
        if let text = details, !text.isEmpty, text != "null" {
            VStack(alignment: .leading, spacing: 4) {
                Text("Details")
                    .font(.caption)
                    .foregroundStyle(OxcerTheme.textSecondary)
                ScrollView {
                    Text(text)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(OxcerTheme.textSecondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(6)
                }
                .frame(maxHeight: 140)
            }
        }
    }
}

// MARK: - Metric chips

struct MetricChipsView: View {
    let metrics: LogMetrics

    var body: some View {
        HStack(spacing: 6) {
            if let t = metrics.tokensIn {
                Chip(text: "in: \(t)")
            }
            if let t = metrics.tokensOut {
                Chip(text: "out: \(t)")
            }
            if let m = metrics.latencyMs {
                Chip(text: "\(m) ms")
            }
            if let c = metrics.costUsd {
                Chip(text: String(format: "$%.4f", c))
            }
        }
    }
}

struct Chip: View {
    let text: String
    var body: some View {
        Text(text)
            .font(.system(.caption2, design: .monospaced))
            .foregroundStyle(OxcerTheme.textSecondary)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(OxcerTheme.cardBackground)
            .clipShape(Capsule())
    }
}

// MARK: - Timeline row (expandable)

struct TimelineEventRow: View {
    let event: LogEvent
    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(OxcerTheme.snappy) { isExpanded.toggle() }
            } label: {
                HStack(alignment: .top, spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(event.timestamp)
                            .font(.system(.caption2, design: .monospaced))
                            .foregroundStyle(OxcerTheme.textTertiary)
                        Text("\(event.component) · \(event.action)")
                            .font(.caption)
                            .foregroundStyle(OxcerTheme.textSecondary)
                        if let d = event.decision {
                            Text(d)
                                .font(.caption2)
                                .foregroundStyle(OxcerTheme.accent)
                        }
                        MetricChipsView(metrics: event.metrics)
                    }
                    Spacer()
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption)
                        .foregroundStyle(OxcerTheme.textTertiary)
                }
                .padding(8)
                .contentShape(Rectangle())
            }
            .buttonStyle(BouncyButtonStyle(scale: 0.97, dimmingOpacity: 0.15))

            if isExpanded {
                EventDetailView(details: event.details)
                    .padding(.leading, 8)
                    .transition(.opacity.combined(with: .move(edge: .top)))
                    .animation(OxcerTheme.snappy, value: isExpanded)
            }
        }
    }
}

// MARK: - Root View (stable container for one-time init)

/// Top-level container created exactly once per window. Owns AppViewModel and runs the
/// one-time model init .task here so it is not re-triggered when NavigationSplitView/detail
/// or chat input focus changes.
struct RootView: View {
    @StateObject private var viewModel = AppViewModel()

    var body: some View {
        ContentView(viewModel: viewModel)
            .task(id: "global_model_init") {
                await viewModel.checkAndPrepareModel()
            }
    }
}

// MARK: - Main ContentView

/// ContentView — ChatGPT-style NavigationSplitView layout. Receives viewModel from RootView.
/// Typing in ChatInputBar triggers ZERO updates here; only sendMessage does.
/// No .task(id: "global_model_init") here; it lives on RootView only.
struct ContentView: View {
    @ObservedObject var viewModel: AppViewModel
    @Environment(\.colorScheme) private var colorScheme

    /// Keeps sidebar + detail always visible. Bound to NavigationSplitView so we can observe
    /// user-initiated toggles, but chat key events must never write to this.
    @State private var columnVisibility = NavigationSplitViewVisibility.all

    var body: some View {
        ZStack {
            // Main app content
            NavigationSplitView(columnVisibility: $columnVisibility) {
                SidebarView(
                    sessions: viewModel.sessions,
                    selectedSessionId: viewModel.selectedSessionID,
                    createNewSession: { viewModel.createNewSession() },
                    selectSession: { viewModel.selectSession($0) },
                    renameSession: { viewModel.renameSession($0, to: $1) },
                    togglePin: { viewModel.togglePin(for: $0) },
                    deleteSession: { viewModel.deleteSession($0) }
                )
            } detail: {
                DetailView(
                    session: viewModel.selectedSession,
                    onSend: { text in viewModel.sendMessage(text) },
                    onStop: { viewModel.stopGeneration() }
                )
            }
            .disabled(!viewModel.isModelReady)
            .blur(radius: viewModel.isModelReady ? 0 : 5)
            .background(OxcerTheme.backgroundDark)
            .animation(.easeInOut(duration: 0.35), value: colorScheme)

            // Approval overlay — sits above the split view so no DetailView re-render can hide it.
            // Delegates to SessionApprovalLayer which observes the session directly, so only
            // approval-state changes (not streaming updates) cause this branch to re-evaluate.
            if let session = viewModel.selectedSession {
                SessionApprovalLayer(
                    session: session,
                    onRespond: { viewModel.respondToApproval($0) }
                )
                .zIndex(50)
            }

            // First Run model installation wizard overlay
            if !viewModel.isModelReady {
                ModelDownloadOverlay(
                    progress: viewModel.downloadProgress,
                    message: viewModel.downloadMessage,
                    loadError: viewModel.loadError,
                    onRetry: { Task { await viewModel.checkAndPrepareModel() } }
                )
                .transition(.opacity.animation(.easeInOut))
                .zIndex(100)
            }
        }
    }
}

#Preview {
    ContentView(viewModel: AppViewModel())
        .frame(width: 1200, height: 800)
}
