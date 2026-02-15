//  ContentView.swift
//  OxcerLauncher
//
//  Refactored UI: ChatGPT-style layout with Toss-inspired dark theme

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
/// App-lifetime state only. Heavy per-session data lives in SessionDetailViewModel.
/// Only one SessionDetailViewModel is alive at a time; switching sessions releases the previous one.
@MainActor
final class AppViewModel: ObservableObject {
    // Workspace + task state (app lifetime)
    @Published var workspaces: [WorkspaceInfo] = []
    @Published var selectedWorkspaceId: String?
    @Published var isTaskRunning: Bool = false

    /// Lightweight session list for sidebar (app lifetime).
    /// Capped at maxSessionsCount (100); loadSessions() not called per agent response.
    @Published var sessions: [SidebarSessionItem] = []
    @Published var isSessionsLoading: Bool = false

    /// Active session detail. Only ONE alive at a time. nil = welcome/empty state.
    @Published var activeSessionDetail: SessionDetailViewModel?

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

    /// Ensures we only run the initial workspace + session load once (avoids OOM from repeated onAppear/.task).
    private var hasPerformedInitialLoad = false

    /// 🔒 GLOBAL LOCK: Persists even if AppViewModel is recreated (e.g. by parent view updates).
    private static var hasAttemptedInit = false
    private static let initLock = NSLock()
    private var isCheckingModel = false

    /// Max number of sessions to keep in memory (lightweight SidebarSessionItem only).
    /// Aligns with Rust list_sessions_from_dir truncation (100); avoids holding more than backend returns.
    private let maxSessionsCount = 100

    /// Cancellable task for sessions list load.
    private var loadSessionsTask: Task<Void, Never>?

    /// Debug: counts loadWorkspaces/loadSessions calls to detect runaway loops.
    private var callCount = 0

    /// Idempotency guard: limits initial workspace/session load to exactly once.
    private var isLoadingInitialData = false

    init(backend: OxcerBackend = DefaultOxcerBackend()) {
        self.backend = backend
        // Warm-up: first FFI call triggers dylib load and static runtime init (pays 17GB VMS cost upfront).
        // _ = backend.ping()
        // Set default once at init so the view's .task never writes @Published (avoids re-render → .task re-run loop).
        self.appConfigDir = Self.defaultAppConfigDir()
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

    /// Call once when the root view appears. Loads workspaces and sessions only the first time.
    /// Prefer checkAndPrepareModel() from .task to benefit from the strict guard against re-entry.
    func loadInitialDataIfNeeded() async {
        guard appConfigDir != nil else { return }
        guard !hasPerformedInitialLoad else { return }
        hasPerformedInitialLoad = true
        await loadWorkspaces()
        await loadSessions()
    }

    /// Start new chat: release previous SessionDetailViewModel (frees messages + sessionEvents).
    func startNewChat() {
        activeSessionDetail?.cancelLoad()
        activeSessionDetail = nil
    }

    /// Send message: ensure we have an active session, append user message, run agent.
    func sendMessage(_ text: String) {
        print("[UI] SEND triggered at \(Date()) with prompt length: \(text.count)")
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        if activeSessionDetail == nil {
            activeSessionDetail = SessionDetailViewModel(sessionId: nil)
        }
        guard let detail = activeSessionDetail else { return }
        detail.appendMessage(ChatMessage(id: UUID(), role: "user", content: trimmed))
        Task { await runAgentRequest(taskText: trimmed) }
    }

    /// Explicit user-initiated refresh. Cancels any in-flight load and reloads the session list.
    /// Call from Refresh button; NOT called automatically after agent responses.
    func refreshSessions() {
        loadSessionsTask?.cancel()
        loadSessionsTask = Task { await loadSessions() }
    }

    /// Select session: release previous, create new SessionDetailViewModel, load its log.
    func selectSession(_ sessionId: String) {
        activeSessionDetail?.cancelLoad()
        activeSessionDetail = nil
        let detail = SessionDetailViewModel(sessionId: sessionId)
        activeSessionDetail = detail
        detail.loadSessionLog(sessionId: sessionId, appConfigDir: appConfigDir, backend: backend)
    }

    // MARK: - Data loading (FFI via backend service)

    /// Loads workspaces from backend. Hidden Singleton: auto-selects first workspace when present,
    /// then loads sessions. Rest of app implicitly uses selectedWorkspaceId.
    /// Idempotency: runs at most once per app launch (isLoadingInitialData guard).
    /// ISOLATION: smoke test body — FFI returns Int32 (42).
    func loadWorkspaces() async {
        do {
            let result = try await backend.listWorkspaces(appConfigDir: "")
            print("FFI SMOKE TEST SUCCESS: \(result)")
        } catch {
            // Non-chat error; could log or show in UI
        }
    }

    /// Loads recent sessions from backend; maps to lightweight SidebarSessionItem.
    /// Invoked: (1) from loadWorkspaces during initial load, (2) explicit Refresh, (3) after first response in new chat.
    /// During initial load, called only from loadWorkspaces (when isLoadingInitialData is true).
    func loadSessions() async {
        callCount += 1
        print("[FFI CALL DEBUG] \(#function) called \(callCount) times")
        isSessionsLoading = true
        defer { isSessionsLoading = false }
        guard let dir = appConfigDir else { return }
        do {
            let list = try await backend.listSessions(appConfigDir: dir)
            guard !Task.isCancelled else { return }
            sessions = Array(list.prefix(maxSessionsCount).map { SidebarSessionItem.from($0) })
        } catch {
            // Ignore cancellation; defer will reset isSessionsLoading
        }
    }

    /// Runs the agent request. Appends response to activeSessionDetail.
    /// IMPORTANT: Does NOT call ensureLocalModel — model must already be ready (gated by checkAndPrepareModel + isModelReady).
    ///
    /// **Session list refresh:** Does NOT reload the full session list on every response.
    /// - Refreshes only when this was a new chat (first response); backend creates the session on first request.
    /// - For subsequent messages in the same chat: no refresh. User can use explicit Refresh to update.
    func runAgentRequest(taskText: String) async {
        let task = taskText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !task.isEmpty else { return }
        isTaskRunning = true

        let wasNewChat = activeSessionDetail?.sessionId == nil

        let wsId = selectedWorkspaceId
        let wsRoot = workspaces.first(where: { $0.id == wsId })?.rootPath
        let context = TaskContext(workspaceId: wsId, selectedPaths: nil, riskHints: nil)
        let payload = AgentRequestPayload(
            taskDescription: task,
            workspaceId: wsId,
            workspaceRoot: wsRoot,
            context: context,
            appConfigDir: appConfigDir
        )

        do {
            let response = try await backend.runAgentTask(payload: payload)
            let answer = response.answer ?? ""
            let errorText = response.error
            if let detail = activeSessionDetail {
                if let err = errorText, !err.isEmpty {
                    detail.appendMessage(ChatMessage(id: UUID(), role: "assistant", content: "Error: \(err)"))
                } else if !answer.isEmpty {
                    detail.appendMessage(ChatMessage(id: UUID(), role: "assistant", content: answer))
                }
            }
            isTaskRunning = false

            // Refresh session list only after first response of a new chat (backend creates session then).
            // Subsequent responses in the same chat do NOT trigger full telemetry scan.
            if wasNewChat {
                await loadSessions()
            }
        } catch {
            activeSessionDetail?.appendMessage(ChatMessage(id: UUID(), role: "assistant", content: "Error: \(error.localizedDescription)"))
            isTaskRunning = false
        }
    }
}

// MARK: - Sidebar View (ChatGPT-style)
// Receives data and closures directly to avoid computed view model re-creation.

struct SidebarView: View {
    let sessions: [SidebarSessionItem]
    let selectedSessionId: String?
    let isSessionsLoading: Bool
    let startNewChat: () -> Void
    let refreshSessions: () -> Void
    let selectSession: (String) -> Void

    var body: some View {
        VStack(spacing: 0) {
            // New Chat button (top)
            Button {
                startNewChat()
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
            if isSessionsLoading {
                VStack(spacing: 6) {
                    ForEach(0..<5, id: \.self) { _ in
                        RoundedRectangle(cornerRadius: 8)
                            .fill(OxcerTheme.cardBackground.opacity(0.6))
                            .frame(height: 36)
                            .shimmering()
                            .padding(.horizontal, 12)
                    }
                }
                .padding(.vertical, 8)
            } else {
                List(sessions, selection: Binding(
                    get: { selectedSessionId.flatMap { Set([$0]) } ?? [] },
                    set: { ids in
                        if let id = ids.first {
                            selectSession(id)
                        } else {
                            startNewChat()
                        }
                    }
                )) { session in
                    SessionListRow(session: session)
                        .tag(session.id)
                }
                .listStyle(.sidebar)
                .scrollContentBackground(.hidden)
                .background(Color.clear)
            }

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

                    Button {
                        refreshSessions()
                    } label: {
                        Image(systemName: "arrow.clockwise")
                            .font(.system(.body))
                            .foregroundStyle(OxcerTheme.textSecondary)
                    }
                    .buttonStyle(BouncyButtonStyle(scale: 0.92, dimmingOpacity: 0.2))
                    .disabled(isSessionsLoading)

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

/// List row for session in sidebar (used with List selection).
private struct SessionListRow: View {
    let session: SidebarSessionItem

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(session.title)
                .font(.system(.caption, design: .monospaced))
                .foregroundStyle(OxcerTheme.textPrimary)
                .lineLimit(1)
            Text(formatTimestamp(session.createdAt))
                .font(.system(.caption2))
                .foregroundStyle(OxcerTheme.textTertiary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 6)
        .padding(.horizontal, 4)
    }
}

// MARK: - Detail View (Chat History / Empty State)

/// When sessionDetail is nil: welcome/empty state. When non-nil: chat + session log from that view model.
struct DetailView: View {
    let sessionDetail: SessionDetailViewModel?
    let isTaskRunning: Bool
    let onSend: (String) -> Void

    var body: some View {
        ZStack(alignment: .bottom) {
            if let detail = sessionDetail {
                ChatDetailContent(sessionDetail: detail)
            } else {
                emptyStateView
            }

            ChatInputBar(isRunning: isTaskRunning, onSend: onSend)
                .padding(.horizontal, 24)
                .padding(.bottom, 24)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color("OxcerBackground"))
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
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

}

/// Observes SessionDetailViewModel so it re-renders when messages/sessionEvents change.
private struct ChatDetailContent: View {
    @ObservedObject var sessionDetail: SessionDetailViewModel

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                ForEach(sessionDetail.messages) { msg in
                    if msg.role == "user" {
                        TaskBubble(text: msg.content)
                    } else {
                        ResultBubble(text: msg.content, isError: msg.content.hasPrefix("Error:"))
                    }
                }
                if !sessionDetail.sessionEvents.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Session Log")
                            .font(.system(.caption, weight: .medium))
                            .foregroundStyle(OxcerTheme.textSecondary)
                        ForEach(sessionDetail.sessionEvents, id: \.self) { event in
                            LogEventBubble(event: event)
                        }
                    }
                }
                Spacer(minLength: 100)
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 24)
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

struct ResultBubble: View {
    let text: String
    let isError: Bool
    
    var body: some View {
        HStack {
            Text(text)
                .font(.system(.body))
                .foregroundStyle(isError ? OxcerTheme.textError : OxcerTheme.textPrimary)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(16)
        .background(
            RoundedRectangle(cornerRadius: OxcerTheme.cardCornerRadius)
                .fill(isError ? OxcerTheme.textError.opacity(0.12) : OxcerTheme.cardBackground)
                .overlay(
                    RoundedRectangle(cornerRadius: OxcerTheme.cardCornerRadius)
                        .stroke(isError ? OxcerTheme.textError.opacity(0.3) : OxcerTheme.border, lineWidth: 1)
                )
        )
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

    var body: some View {
        ZStack {
            // Main app content
            NavigationSplitView {
                SidebarView(
                    sessions: viewModel.sessions,
                    selectedSessionId: viewModel.activeSessionDetail?.sessionId,
                    isSessionsLoading: viewModel.isSessionsLoading,
                    startNewChat: { viewModel.startNewChat() },
                    refreshSessions: { viewModel.refreshSessions() },
                    selectSession: { viewModel.selectSession($0) }
                )
            } detail: {
                DetailView(
                    sessionDetail: viewModel.activeSessionDetail,
                    isTaskRunning: viewModel.isTaskRunning,
                    onSend: { text in viewModel.sendMessage(text) }
                )
            }
            .disabled(!viewModel.isModelReady)
            .blur(radius: viewModel.isModelReady ? 0 : 5)
            .background(OxcerTheme.backgroundDark)
            .animation(.easeInOut(duration: 0.35), value: colorScheme)

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
