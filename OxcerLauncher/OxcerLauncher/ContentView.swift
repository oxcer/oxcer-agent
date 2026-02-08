//  ContentView.swift
//  OxcerLauncher
//
//  v0 screens: (1) Workspace + Task, (2) Recent Sessions + timeline.

import SwiftUI

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

// MARK: - Workspace + Task screen (3.1)

struct WorkspaceTaskView: View {
    @Binding var workspaces: [WorkspaceInfo]
    @Binding var selectedWorkspaceId: String?
    @Binding var taskDescription: String
    @Binding var resultText: String
    @Binding var errorMessage: String?
    var isRunning: Bool
    var appConfigDir: String?
    let onRunTask: () -> Void
    let onLoadWorkspaces: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            // Left/top: workspace selector
            HStack(alignment: .center, spacing: 8) {
                Text("Workspace")
                    .font(.headline)
                Picker("", selection: $selectedWorkspaceId) {
                    Text("— Select —").tag(nil as String?)
                    ForEach(workspaces, id: \.id) { w in
                        Text(w.name).tag(w.id as String?)
                    }
                }
                .pickerStyle(.menu)
                .labelsHidden()
                .frame(maxWidth: 280)
                if workspaces.isEmpty {
                    Text("No workspaces (config at Application Support/Oxcer/config.json)")
                        .foregroundStyle(.secondary)
                        .font(.caption)
                }
            }

            // Center: multiline task + Run Task
            VStack(alignment: .leading, spacing: 8) {
                Text("Task description")
                    .font(.headline)
                TextEditor(text: $taskDescription)
                    .font(.body)
                    .border(Color.secondary.opacity(0.3), width: 1)
                    .frame(minHeight: 100, maxHeight: 200)
                HStack(spacing: 8) {
                    Button("Run Task") {
                        onRunTask()
                    }
                    .disabled(taskDescription.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || isRunning)
                    .keyboardShortcut(.return, modifiers: .command)
                    if isRunning {
                        ProgressView()
                            .scaleEffect(0.7)
                        Text("Running…")
                            .foregroundStyle(.secondary)
                            .font(.caption)
                    }
                }
            }

            // Result: scrollable answer or error
            if !resultText.isEmpty || (errorMessage != nil) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Result")
                        .font(.headline)
                    ScrollView {
                        Group {
                            if let err = errorMessage {
                                Text(err)
                                    .foregroundStyle(.red)
                                    .textSelection(.enabled)
                            }
                            if !resultText.isEmpty {
                                Text(resultText)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .textSelection(.enabled)
                            }
                        }
                        .padding(8)
                    }
                    .frame(minHeight: 80, maxHeight: 220)
                    .border(Color.secondary.opacity(0.3), width: 1)
                }
            }

            Spacer(minLength: 0)
        }
        .padding()
        .onAppear { onLoadWorkspaces() }
    }
}

// MARK: - Event detail (JSON details as monospaced / key-value)

struct EventDetailView: View {
    let details: AnyCodableValue?

    var body: some View {
        if let d = details {
            let text = d.jsonString
            if !text.isEmpty && text != "null" {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Details")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    ScrollView {
                        Text(text)
                            .font(.system(.caption, design: .monospaced))
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(6)
                    }
                    .frame(maxHeight: 140)
                }
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
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Color.secondary.opacity(0.2))
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
                withAnimation(.easeInOut(duration: 0.2)) { isExpanded.toggle() }
            } label: {
                HStack(alignment: .top, spacing: 8) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(event.timestamp)
                            .font(.system(.caption2, design: .monospaced))
                            .foregroundStyle(.secondary)
                        Text("\(event.component) · \(event.action)")
                            .font(.caption)
                        if let d = event.decision {
                            Text(d)
                                .font(.caption2)
                                .foregroundStyle(.blue)
                        }
                        MetricChipsView(metrics: event.metrics)
                    }
                    Spacer()
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .padding(8)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if isExpanded {
                EventDetailView(details: event.details)
                    .padding(.leading, 8)
            }
        }
    }
}

// MARK: - Recent Sessions + timeline (3.2)

struct RecentSessionsView: View {
    @Binding var sessions: [SessionSummary]
    @Binding var selectedSessionId: String?
    @Binding var sessionEvents: [LogEvent]
    var isLoading: Bool
    var appConfigDir: String?
    let onLoadSessions: () -> Void
    let onSelectSession: (String) -> Void

    var body: some View {
        HSplitView {
            // Table of sessions
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text("Recent Sessions")
                        .font(.headline)
                    Button("Refresh") {
                        onLoadSessions()
                    }
                    .disabled(isLoading)
                }
                Table(sessions, selection: $selectedSessionId) {
                    TableColumn("Session") { s in
                        Text(shortSessionId(s.sessionId))
                            .font(.system(.caption, design: .monospaced))
                    }
                    TableColumn("Start") { s in
                        Text(formatTimestamp(s.startTimestamp))
                            .font(.caption)
                    }
                    TableColumn("End") { s in
                        Text(formatTimestamp(s.endTimestamp))
                            .font(.caption)
                    }
                    TableColumn("Cost") { s in
                        Text(String(format: "%.4f", s.totalCostUsd))
                            .font(.caption)
                    }
                    TableColumn("Tools") { s in
                        Text("\(s.toolCallsCount)")
                            .font(.caption)
                    }
                    TableColumn("Status") { s in
                        Text(s.success ? "✓" : "✗")
                            .foregroundStyle(s.success ? .green : .red)
                    }
                }
                .onChange(of: selectedSessionId) { _, newId in
                    if let id = newId { onSelectSession(id) }
                }
            }
            .frame(minWidth: 420)

            // Timeline
            VStack(alignment: .leading, spacing: 8) {
                Text("Timeline")
                    .font(.headline)
                if sessionEvents.isEmpty {
                    Text(selectedSessionId == nil ? "Select a session" : "No events")
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    List(sessionEvents, id: \.timestamp) { e in
                        TimelineEventRow(event: e)
                    }
                    .listStyle(.inset)
                }
            }
            .frame(minWidth: 320)
        }
        .onAppear { onLoadSessions() }
    }
}

// MARK: - Main ContentView (tabs)

struct ContentView: View {
    @State private var workspaces: [WorkspaceInfo] = []
    @State private var selectedWorkspaceId: String?
    @State private var taskDescription: String = ""
    @State private var resultText: String = ""
    @State private var errorMessage: String?
    @State private var isTaskRunning = false
    @State private var sessions: [SessionSummary] = []
    @State private var selectedSessionId: String?
    @State private var sessionEvents: [LogEvent] = []
    @State private var isSessionsLoading = false
    @State private var appConfigDir: String?
    @State private var selectedTab: Int = 0

    var body: some View {
        TabView(selection: $selectedTab) {
            WorkspaceTaskView(
                workspaces: $workspaces,
                selectedWorkspaceId: $selectedWorkspaceId,
                taskDescription: $taskDescription,
                resultText: $resultText,
                errorMessage: $errorMessage,
                isRunning: isTaskRunning,
                appConfigDir: appConfigDir,
                onRunTask: runAgentRequest,
                onLoadWorkspaces: loadWorkspaces
            )
            .tabItem { Label("Task", systemImage: "terminal") }
            .tag(0)

            RecentSessionsView(
                sessions: $sessions,
                selectedSessionId: $selectedSessionId,
                sessionEvents: $sessionEvents,
                isLoading: isSessionsLoading,
                appConfigDir: appConfigDir,
                onLoadSessions: loadSessions,
                onSelectSession: loadSessionLog
            )
            .tabItem { Label("Recent Sessions", systemImage: "clock.arrow.circlepath") }
            .tag(1)
        }
        .onAppear {
            appConfigDir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
                .appendingPathComponent("Oxcer").path
        }
    }

    private func loadWorkspaces() {
        guard let dir = appConfigDir else { return }
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let list = try OxcerFFI.listWorkspaces(appConfigDir: dir)
                DispatchQueue.main.async {
                    workspaces = list
                    if selectedWorkspaceId == nil, let first = list.first {
                        selectedWorkspaceId = first.id
                    }
                }
            } catch {
                DispatchQueue.main.async {
                    errorMessage = error.localizedDescription
                }
            }
        }
    }

    private func loadSessions() {
        isSessionsLoading = true
        guard let dir = appConfigDir else { isSessionsLoading = false; return }
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let list = try OxcerFFI.listSessions(appConfigDir: dir)
                DispatchQueue.main.async {
                    sessions = list
                    isSessionsLoading = false
                }
            } catch {
                DispatchQueue.main.async {
                    errorMessage = error.localizedDescription
                    isSessionsLoading = false
                }
            }
        }
    }

    private func loadSessionLog(sessionId: String) {
        guard let dir = appConfigDir else { return }
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let events = try OxcerFFI.loadSessionLog(sessionId: sessionId, appConfigDir: dir)
                DispatchQueue.main.async {
                    sessionEvents = events
                }
            } catch {
                DispatchQueue.main.async {
                    sessionEvents = []
                    errorMessage = error.localizedDescription
                }
            }
        }
    }

    private func runAgentRequest() {
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
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let response = try OxcerFFI.agentRequest(payload)
                DispatchQueue.main.async {
                    resultText = response.answer ?? ""
                    if let err = response.error, !err.isEmpty {
                        errorMessage = err
                    }
                    isTaskRunning = false
                    loadSessions()
                }
            } catch {
                DispatchQueue.main.async {
                    errorMessage = error.localizedDescription
                    resultText = ""
                    isTaskRunning = false
                }
            }
        }
    }
}

#Preview {
    ContentView()
        .frame(width: 900, height: 560)
}
