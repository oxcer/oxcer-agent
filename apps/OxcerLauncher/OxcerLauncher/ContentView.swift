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
struct ShimmerModifier: ViewModifier {
    @State private var phase: CGFloat = -0.6
    
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
                withAnimation(.linear(duration: 1.1).repeatForever(autoreverses: false)) {
                    phase = 1.4
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

// MARK: - Identifiable for Table selection

extension SessionSummary: Identifiable {
    public var id: String { sessionId }
}

// MARK: - Helpers

private func shortSessionId(_ id: String) -> String {
    if id.count <= 12 { return id }
    return String(id.prefix(6)) + "…" + String(id.suffix(4))
}

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

// MARK: - App View Model (single source of truth)

/// AppViewModel
/// Implemented:
/// - Process/window-scoped composition root: created once per window scene and held for the life of the SwiftUI hierarchy.
/// - Owns the single source of truth for workspaces, current task, results, recent sessions, and logs.
/// - Owns the OxcerBackend service and exposes feature-specific view models (sidebar, session, task input) via computed properties.
/// TODO:
/// - Extract the raw state into a separate AppStore struct if the app grows significantly.
@MainActor
final class AppViewModel: ObservableObject {
    // Workspace + task state
    @Published var workspaces: [WorkspaceInfo] = []
    @Published var selectedWorkspaceId: String?
    @Published var taskDescription: String = ""
    @Published var resultText: String = ""
    @Published var errorMessage: String?
    @Published var isTaskRunning: Bool = false

    // Sessions + logs
    @Published var sessions: [SessionSummary] = []
    @Published var selectedSessionId: String?
    @Published var sessionEvents: [LogEvent] = []
    @Published var isSessionsLoading: Bool = false

    // App config directory for FFI (set by root view onAppear)
    @Published var appConfigDir: String?

    // Backend service abstraction over OxcerFFI
    private let backend: OxcerBackend

    /// Ensures we only run the initial workspace + session load once (avoids OOM from repeated onAppear/.task).
    private var hasPerformedInitialLoad = false

    /// Max number of sessions to keep in memory; avoids OOM with large telemetry dirs.
    private let maxSessionsCount = 500
    /// Max number of log events per session to keep in memory.
    private let maxSessionEventsCount = 2000

    init(backend: OxcerBackend = DefaultOxcerBackend()) {
        self.backend = backend
        // Set default once at init so the view's .task never writes @Published (avoids re-render → .task re-run loop).
        self.appConfigDir = Self.defaultAppConfigDir()
    }

    private static func defaultAppConfigDir() -> String? {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent("Oxcer").path
    }

    /// Call once when the root view appears. Loads workspaces and sessions only the first time.
    func loadInitialDataIfNeeded() async {
        guard appConfigDir != nil else { return }
        guard !hasPerformedInitialLoad else { return }
        hasPerformedInitialLoad = true
        await loadWorkspaces()
        await loadSessions()
    }

    // MARK: - Feature view models (unidirectional data flow)

    /// Sidebar-specific view model constructed from the single source of truth.
    var sidebarViewModel: SidebarViewModel {
        SidebarViewModel(
            workspaces: workspaces,
            selectedWorkspaceId: selectedWorkspaceId,
            sessions: sessions,
            selectedSessionId: selectedSessionId,
            isSessionsLoading: isSessionsLoading,
            selectWorkspace: { [weak self] id in
                // Intent: user selected a workspace in the sidebar.
                self?.selectedWorkspaceId = id
            },
            refreshSessions: { [weak self] in
                guard let self else { return }
                Task { await self.loadSessions() }
            },
            selectSession: { [weak self] id in
                guard let self else { return }
                Task { await self.loadSessionLog(sessionId: id) }
            }
        )
    }

    /// Session (main panel) view model derived from root state.
    var sessionViewModel: SessionViewModel {
        let currentName: String
        if let id = selectedWorkspaceId,
           let workspace = workspaces.first(where: { $0.id == id }) {
            currentName = workspace.name
        } else {
            currentName = "No workspace selected"
        }
        let status: TaskStatus
        if isTaskRunning {
            status = .running
        } else if !resultText.isEmpty || errorMessage != nil {
            status = .completed
        } else {
            status = .idle
        }
        return SessionViewModel(
            currentWorkspaceName: currentName,
            status: status,
            taskDescription: taskDescription,
            resultText: resultText,
            errorMessage: errorMessage,
            sessionEvents: sessionEvents,
            runTask: { [weak self] in
                guard let self else { return }
                Task { await self.runAgentRequest() }
            }
        )
    }

    /// Task-input (bottom bar) view model, exposing a binding to the single taskDescription source.
    var taskInputViewModel: TaskInputViewModel {
        TaskInputViewModel(
            taskDescription: Binding(
                get: { self.taskDescription },
                set: { self.taskDescription = $0 }
            ),
            isRunning: isTaskRunning,
            runTask: { [weak self] in
                guard let self else { return }
                Task { await self.runAgentRequest() }
            }
        )
    }

    // MARK: - Data loading (FFI via backend service)

    /// Implemented: loads workspaces from Rust via OxcerBackend.listWorkspaces, preserves original behavior.
    func loadWorkspaces() async {
        guard let dir = appConfigDir else { return }
        do {
            let list = try await backend.listWorkspaces(appConfigDir: dir)
            workspaces = list
            if selectedWorkspaceId == nil, let first = list.first {
                selectedWorkspaceId = first.id
            }
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// Implemented: loads recent sessions list from Rust via OxcerBackend.listSessions.
    /// Capped to maxSessionsCount to avoid OOM with large telemetry dirs.
    func loadSessions() async {
        isSessionsLoading = true
        guard let dir = appConfigDir else { isSessionsLoading = false; return }
        do {
            let list = try await backend.listSessions(appConfigDir: dir)
            sessions = Array(list.prefix(maxSessionsCount))
            isSessionsLoading = false
        } catch {
            errorMessage = error.localizedDescription
            isSessionsLoading = false
        }
    }

    /// Implemented: loads one session's log events via OxcerBackend.loadSessionLog.
    /// Capped to maxSessionEventsCount to avoid OOM for very long logs.
    func loadSessionLog(sessionId: String) async {
        guard let dir = appConfigDir else { return }
        selectedSessionId = sessionId
        do {
            let events = try await backend.loadSessionLog(sessionId: sessionId, appConfigDir: dir)
            sessionEvents = Array(events.prefix(maxSessionEventsCount))
        } catch {
            sessionEvents = []
            errorMessage = error.localizedDescription
        }
    }

    /// Implemented: runs the agent request via OxcerBackend.runAgentTask, same payload as before.
    /// The `taskDescription` property here is the single source bound to TaskInputViewModel.
    func runAgentRequest() async {
        let task = taskDescription.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !task.isEmpty else { return }
        isTaskRunning = true
        errorMessage = nil
        resultText = ""
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
            resultText = response.answer ?? ""
            if let err = response.error, !err.isEmpty {
                errorMessage = err
            }
            isTaskRunning = false
            await loadSessions()
        } catch {
            errorMessage = error.localizedDescription
            resultText = ""
            isTaskRunning = false
        }
    }
}

// MARK: - Feature view models (value types for SwiftUI)

/// SidebarViewModel
/// Implemented:
/// - Feature-scoped, created on-demand from AppViewModel, does not outlive the window scene that owns AppViewModel.
/// - Exposes read-only workspace/session lists and selection state, plus intent closures for user actions.
/// - Does NOT talk to the backend directly; instead, it calls into AppViewModel via its intent closures.
/// TODO:
/// - Extend with additional sidebar sections (e.g., favorites, pinned sessions) without touching root logic.
struct SidebarViewModel {
    // Read-only data for the sidebar UI.
    let workspaces: [WorkspaceInfo]
    let selectedWorkspaceId: String?
    let sessions: [SessionSummary]
    let selectedSessionId: String?
    let isSessionsLoading: Bool

    // Intents from the sidebar into the app.
    let selectWorkspace: (String?) -> Void
    let refreshSessions: () -> Void
    let selectSession: (String) -> Void
}

/// SessionViewModel
/// Implemented:
/// - Feature-scoped, derived entirely from AppViewModel state (read-only from the UI perspective).
/// - Does not own or mutate any state directly; exposes a single runTask() intent.
/// - Contains all data needed to render the main panel (current workspace, status, result, logs).
/// TODO:
/// - Add richer timeline structures (grouped events, filters) without coupling to FFI.
struct SessionViewModel {
    let currentWorkspaceName: String
    let status: TaskStatus
    let taskDescription: String
    let resultText: String
    let errorMessage: String?
    let sessionEvents: [LogEvent]

    let runTask: () -> Void
}

/// TaskInputViewModel
/// Implemented:
/// - Feature-scoped, owns a Binding into AppViewModel.taskDescription plus a runTask() intent.
/// - Acts as the only writer for the task input from the UI side; the FFI payload reads the same string from AppViewModel.
/// TODO:
/// - Add support for additional metadata (e.g., risk hints, file selections) without breaking the input bar API.
struct TaskInputViewModel {
    @Binding var taskDescription: String
    let isRunning: Bool
    let runTask: () -> Void
}

// MARK: - Sidebar View

struct SidebarView: View {
    let viewModel: SidebarViewModel
    @Namespace private var selectionNamespace
    
    var body: some View {
        VStack(spacing: 0) {
            // App logo + name
            HStack(spacing: 10) {
                Image(systemName: "sparkles")
                    .font(.title2)
                    .foregroundStyle(OxcerTheme.accent)
                Text("Oxcer")
                    .font(.system(.title2, design: .rounded, weight: .semibold))
                    .foregroundStyle(OxcerTheme.textPrimary)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 16)
            .frame(maxWidth: .infinity, alignment: .leading)
            
            Divider()
                .background(OxcerTheme.divider)
            
            ScrollView {
                VStack(spacing: 0) {
                    // Workspaces section
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Workspaces")
                            .font(.system(.subheadline, weight: .medium))
                            .foregroundStyle(OxcerTheme.textSecondary)
                            .padding(.horizontal, 20)
                            .padding(.top, 20)
                            .padding(.bottom, 8)
                        
                        VStack(spacing: 4) {
                            if viewModel.workspaces.isEmpty {
                                Text("No workspaces")
                                    .font(.caption)
                                    .foregroundStyle(OxcerTheme.textTertiary)
                                    .padding(.horizontal, 20)
                                    .padding(.vertical, 8)
                            } else {
                                ForEach(viewModel.workspaces, id: \.id) { workspace in
                                    WorkspaceRow(
                                        workspace: workspace,
                                        isSelected: viewModel.selectedWorkspaceId == workspace.id,
                                        namespace: selectionNamespace
                                    ) {
                                        withAnimation(OxcerTheme.snappy) {
                                            viewModel.selectWorkspace(workspace.id)
                                        }
                                    }
                                }
                            }
                        }
                    }
                    
                    // Recent Sessions section
                    VStack(alignment: .leading, spacing: 8) {
                        HStack {
                            Text("Recent Sessions")
                                .font(.system(.subheadline, weight: .medium))
                                .foregroundStyle(OxcerTheme.textSecondary)
                            Spacer()
                            Button {
                                viewModel.refreshSessions()
                            } label: {
                                Image(systemName: "arrow.clockwise")
                                    .font(.caption)
                                    .foregroundStyle(OxcerTheme.textSecondary)
                            }
                            .buttonStyle(BouncyButtonStyle(scale: 0.9, dimmingOpacity: 0.25))
                            .disabled(viewModel.isSessionsLoading)
                        }
                        .padding(.horizontal, 20)
                        .padding(.top, 20)
                        .padding(.bottom, 8)
                        
                        if viewModel.isSessionsLoading {
                            // Skeleton loading state with shimmer effect while sessions load.
                            VStack(spacing: 6) {
                                ForEach(0..<3, id: \.self) { _ in
                                    RoundedRectangle(cornerRadius: 8)
                                        .fill(OxcerTheme.cardBackground.opacity(0.6))
                                        .frame(height: 28)
                                        .shimmering()
                                        .padding(.horizontal, 20)
                                }
                            }
                            .padding(.vertical, 4)
                        } else {
                            if viewModel.sessions.isEmpty {
                                Text("No sessions")
                                    .font(.caption)
                                    .foregroundStyle(OxcerTheme.textTertiary)
                                    .padding(.horizontal, 20)
                                    .padding(.vertical, 8)
                            } else {
                                ForEach(viewModel.sessions.prefix(5), id: \.id) { session in
                                    SessionRow(
                                        session: session,
                                        isSelected: viewModel.selectedSessionId == session.sessionId
                                    ) {
                                        viewModel.selectSession(session.sessionId)
                                    }
                                }
                            }
                        }
                    }
                    
                    Spacer(minLength: 20)
                }
            }
        }
        .frame(width: 240)
        .background(OxcerTheme.sidebarBackground)
        .safeAreaInset(edge: .bottom) {
            Button {
                NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
            } label: {
                HStack(spacing: 10) {
                    Image(systemName: "gearshape")
                        .font(.system(.body))
                    Text("Settings")
                        .font(.system(.body))
                }
                .foregroundStyle(OxcerTheme.textSecondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.vertical, 10)
                .contentShape(Rectangle())
            }
            .buttonStyle(BouncyButtonStyle())
            .background(OxcerTheme.sidebarBackground)
        }
    }
}

struct WorkspaceRow: View {
    let workspace: WorkspaceInfo
    let isSelected: Bool
    let namespace: Namespace.ID
    let action: () -> Void
    @State private var isHovered = false
    
    var body: some View {
        Button(action: action) {
            ZStack(alignment: .leading) {
                if isSelected {
                    RoundedRectangle(cornerRadius: 10)
                        .fill(OxcerTheme.accent.opacity(0.18))
                        .matchedGeometryEffect(id: "workspaceSelection", in: namespace)
                }
                
                HStack(spacing: 10) {
                    Circle()
                        .fill(isSelected ? OxcerTheme.accent : OxcerTheme.textTertiary)
                        .frame(width: 6, height: 6)
                    Text(workspace.name)
                        .font(.system(.body))
                        .foregroundStyle(isSelected ? OxcerTheme.textPrimary : OxcerTheme.textSecondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 10)
                .background(
                    RoundedRectangle(cornerRadius: 10)
                        .fill(isHovered && !isSelected ? OxcerTheme.hoverOverlay : Color.clear)
                )
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(BouncyButtonStyle())
        .onHover { hovering in
            withAnimation(OxcerTheme.snappy) {
                isHovered = hovering
            }
        }
    }
}

struct SessionRow: View {
    let session: SessionSummary
    let isSelected: Bool
    let action: () -> Void
    @State private var isHovered = false
    
    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 4) {
                Text(shortSessionId(session.sessionId))
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(isSelected ? OxcerTheme.textPrimary : OxcerTheme.textSecondary)
                Text(formatTimestamp(session.startTimestamp))
                    .font(.system(.caption2))
                    .foregroundStyle(OxcerTheme.textTertiary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 20)
            .padding(.vertical, 8)
            .background(
                RoundedRectangle(cornerRadius: 8)
                    .fill(isSelected ? OxcerTheme.accent.opacity(0.15) : (isHovered ? OxcerTheme.hoverOverlay : Color.clear))
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(BouncyButtonStyle())
        .onHover { hovering in
            withAnimation(OxcerTheme.snappy) {
                isHovered = hovering
            }
        }
    }
}

// MARK: - Session View (Main Panel)

struct SessionView: View {
    let viewModel: SessionViewModel
    
    var body: some View {
        VStack(spacing: 0) {
            // Header
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text(viewModel.currentWorkspaceName)
                        .font(.system(.headline, weight: .semibold))
                        .foregroundStyle(OxcerTheme.textPrimary)
                    TaskStatusBadge(status: viewModel.status)
                }
                
                Spacer()
                
                Button {
                    viewModel.runTask()
                } label: {
                    HStack(spacing: 6) {
                        if viewModel.status == .running {
                            ProgressView()
                                .scaleEffect(0.7)
                                .tint(OxcerTheme.onAccent)
                        } else {
                            Image(systemName: "play.fill")
                                .font(.system(.caption, weight: .semibold))
                        }
                        Text("Run Task")
                            .font(.system(.subheadline, weight: .medium))
                    }
                    .foregroundStyle(OxcerTheme.onAccent)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .background(
                        Capsule()
                            .fill(OxcerTheme.accent)
                    )
                }
                .buttonStyle(BouncyButtonStyle())
                .disabled(viewModel.taskDescription.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || viewModel.status == .running)
                .keyboardShortcut(.return, modifiers: .command)
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 16)
            .background(
                Rectangle()
                    .fill(OxcerTheme.backgroundPanel)
                    .overlay(
                        Rectangle()
                            .fill(OxcerTheme.divider)
                            .frame(height: 1),
                        alignment: .bottom
                    )
            )
            
            // Timeline / Log area
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    // Current task description bubble
                    if !viewModel.taskDescription.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                        TaskBubble(text: viewModel.taskDescription)
                            .transition(.opacity.combined(with: .move(edge: .bottom)))
                            .animation(OxcerTheme.snappy, value: viewModel.taskDescription)
                    }
                    
                    // Result or error
                    if !viewModel.resultText.isEmpty {
                        ResultBubble(text: viewModel.resultText, isError: false)
                            .transition(.opacity.combined(with: .move(edge: .bottom)))
                            .animation(OxcerTheme.snappy, value: viewModel.resultText)
                    }
                    
                    if let error = viewModel.errorMessage {
                        ResultBubble(text: error, isError: true)
                            .transition(.opacity.combined(with: .move(edge: .bottom)))
                            .animation(OxcerTheme.snappy, value: error)
                    }
                    
                    // TODO: Log entries will appear here as they stream in
                    // For now, show placeholder if we have session events
                    if !viewModel.sessionEvents.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("Session Log")
                                .font(.system(.caption, weight: .medium))
                                .foregroundStyle(OxcerTheme.textSecondary)
                                .padding(.horizontal, 16)
                            
                            ForEach(viewModel.sessionEvents.prefix(3), id: \.timestamp) { event in
                                LogEventBubble(event: event)
                            }
                        }
                    }
                    
                    Spacer(minLength: 20)
                }
                .padding(.horizontal, 24)
                .padding(.vertical, 20)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(OxcerTheme.backgroundPanel)
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

// MARK: - Task Input Bar

struct TaskInputBar: View {
    let viewModel: TaskInputViewModel
    @FocusState private var isFocused: Bool
    
    var body: some View {
        HStack(spacing: 12) {
            // Multi-line text field
            ZStack(alignment: .topLeading) {
                if viewModel.taskDescription.isEmpty {
                    Text("Describe what you want Oxcer to do…")
                        .font(.system(.body))
                        .foregroundStyle(OxcerTheme.textTertiary)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 12)
                        .allowsHitTesting(false)
                }
                
                // Single source of truth: this TextEditor writes into AppViewModel.taskDescription
                // via TaskInputViewModel, which AppViewModel.runAgentRequest() then reads to
                // construct the FFI payload.
                TextEditor(text: viewModel.$taskDescription)
                    .font(.system(.body))
                    .foregroundStyle(OxcerTheme.textPrimary)
                    .scrollContentBackground(.hidden)
                    .background(Color.clear)
                    .frame(minHeight: 44, maxHeight: 120)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .focused($isFocused)
                    .onSubmit {
                        if !viewModel.taskDescription
                            .trimmingCharacters(in: .whitespacesAndNewlines)
                            .isEmpty && !viewModel.isRunning {
                            viewModel.runTask()
                        }
                    }
            }
            
            // Run button
            Button {
                viewModel.runTask()
            } label: {
                if viewModel.isRunning {
                    ProgressView()
                        .scaleEffect(0.8)
                        .tint(OxcerTheme.onAccent)
                } else {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(.title3))
                }
            }
            .buttonStyle(BouncyButtonStyle())
            .foregroundStyle(OxcerTheme.onAccent)
            .frame(width: 36, height: 36)
            .background(
                Circle()
                    .fill(OxcerTheme.accent)
            )
            .disabled(
                viewModel.taskDescription
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                    .isEmpty || viewModel.isRunning
            )
            .keyboardShortcut(.return, modifiers: .command)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
        .background(
            RoundedRectangle(cornerRadius: OxcerTheme.inputCornerRadius)
                .fill(OxcerTheme.cardBackground)
                .overlay(
                    RoundedRectangle(cornerRadius: OxcerTheme.inputCornerRadius)
                        .stroke(
                            isFocused
                            ? OxcerTheme.accent.opacity(0.5)
                            : OxcerTheme.border,
                            lineWidth: isFocused ? 2 : 1
                        )
                )
        )
        .scaleEffect(isFocused ? 1.01 : 1.0)
        .shadow(
            color: isFocused ? OxcerTheme.accent.opacity(0.25) : Color.clear,
            radius: 8,
            x: 0,
            y: 4
        )
        .animation(OxcerTheme.snappy, value: isFocused)
        .padding(.horizontal, 24)
        .padding(.vertical, 16)
        .background(OxcerTheme.backgroundPanel)
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

// MARK: - Main ContentView

/// ContentView (root)
/// Implemented:
/// - Owns a single @StateObject AppViewModel used as the source of truth for all subviews.
/// - Sets AppViewModel.appConfigDir on appear so FFI calls know where to read/write.
/// - Constructs SidebarViewModel, SessionViewModel, and TaskInputViewModel from AppViewModel state
///   and passes them down so that views depend only on their feature-specific view model.
///   - Workspace selection in SidebarView calls SidebarViewModel.selectWorkspace(_:) which mutates
///     AppViewModel.selectedWorkspaceId; SessionViewModel then reflects that change via its derived
///     currentWorkspaceName property.
///   - TaskInputBar binds to TaskInputViewModel.taskDescription, which writes into
///     AppViewModel.taskDescription; AppViewModel.runAgentRequest() reads the same value
///     to build the AgentRequestPayload for the Rust FFI.
/// TODO:
/// - Introduce additional windows/scenes (e.g., settings, inspector) that also observe AppViewModel.
struct ContentView: View {
    @StateObject private var viewModel = AppViewModel()
    @Environment(\.colorScheme) private var colorScheme
    /// View-level guard so initial load runs at most once even if .task is re-invoked (avoids infinite loop).
    @State private var initialLoadStarted = false

    var body: some View {
        HStack(spacing: 0) {
            // Left Sidebar
            SidebarView(viewModel: viewModel.sidebarViewModel)
            
            Divider()
                .background(OxcerTheme.divider)
            
            // Main content area
            VStack(spacing: 0) {
                SessionView(viewModel: viewModel.sessionViewModel)
                
                Divider()
                    .background(OxcerTheme.divider)
                
                TaskInputBar(viewModel: viewModel.taskInputViewModel)
            }
        }
        .background(OxcerTheme.backgroundDark)
        .animation(.easeInOut(duration: 0.35), value: colorScheme)
        .task(id: "initialLoad") {
            guard !initialLoadStarted else { return }
            initialLoadStarted = true
            await viewModel.loadInitialDataIfNeeded()
        }
    }
}

#Preview {
    ContentView()
        .frame(width: 1200, height: 800)
}
