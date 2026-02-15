//  ModelDownloadOverlay.swift
//  OxcerLauncher
//
//  Full-screen overlay for First Run model installation wizard.

import SwiftUI

/// Full-screen overlay that blocks interaction until the model is ready.
/// Shows download progress or an error state with Retry.
struct ModelDownloadOverlay: View {
    let progress: Double
    let message: String
    let loadError: String?
    let onRetry: () -> Void

    @Environment(\.scenePhase) private var scenePhase
    @State private var isPulsing = false
    @State private var pulseTask: Task<Void, Never>?

    var body: some View {
        ZStack {
            Rectangle()
                .fill(.ultraThickMaterial)
                .overlay(Color.black.opacity(0.3))
                .ignoresSafeArea()

            VStack(spacing: 32) {
                if let error = loadError {
                    errorView(error: error, onRetry: onRetry)
                } else {
                    loadingView
                }
            }
            .padding(48)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func errorView(error: String, onRetry: @escaping () -> Void) -> some View {
        VStack(spacing: 20) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 50))
                .foregroundStyle(.orange)

            Text("Setup Failed")
                .font(.headline)
                .foregroundStyle(OxcerTheme.textPrimary)

            Text(error)
                .font(.caption)
                .foregroundStyle(OxcerTheme.textSecondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal)

            Button(action: onRetry) {
                Label("Retry Setup", systemImage: "arrow.clockwise")
                    .padding()
                    .background(OxcerTheme.accent)
                    .foregroundStyle(.white)
                    .clipShape(RoundedRectangle(cornerRadius: 10))
            }
        }
    }

    private var loadingView: some View {
        VStack(spacing: 32) {
            Image(systemName: "brain")
                .font(.system(size: 72))
                .foregroundStyle(OxcerTheme.accent)
                .scaleEffect(isPulsing ? 1.08 : 1.0)
                .opacity(isPulsing ? 0.9 : 1.0)
                .animation(.easeInOut(duration: 0.6), value: isPulsing)
                .onAppear { startPulseIfActive() }
                .onDisappear {
                    pulseTask?.cancel()
                    pulseTask = nil
                }
                .onChange(of: scenePhase) { newPhase in
                    if newPhase != .active {
                        pulseTask?.cancel()
                        pulseTask = nil
                    } else {
                        startPulseIfActive()
                    }
                }

            Text("Setting up Oxcer Intelligence")
                .font(.system(.title, design: .rounded, weight: .semibold))
                .foregroundStyle(OxcerTheme.textPrimary)

            VStack(spacing: 12) {
                ProgressView(value: progress, total: 1.0)
                    .progressViewStyle(.linear)
                    .tint(OxcerTheme.accent)
                    .frame(maxWidth: 280)

                Text(message)
                    .font(.system(.subheadline))
                    .foregroundStyle(OxcerTheme.textSecondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: 320)
            }
            .padding(.horizontal, 24)
        }
    }

    private func startPulseIfActive() {
        guard scenePhase == .active else { return }
        pulseTask = Task { @MainActor in
            while !Task.isCancelled {
                isPulsing = true
                try? await Task.sleep(nanoseconds: 600_000_000)
                guard !Task.isCancelled else { break }
                isPulsing = false
                try? await Task.sleep(nanoseconds: 600_000_000)
            }
        }
    }
}

#Preview("Loading") {
    ModelDownloadOverlay(
        progress: 0.45,
        message: "Downloading Phi-3 Mini... 45%",
        loadError: nil,
        onRetry: {}
    )
    .frame(width: 400, height: 300)
}

#Preview("Error") {
    ModelDownloadOverlay(
        progress: 0,
        message: "",
        loadError: "Network connection failed. Please check your internet.",
        onRetry: {}
    )
    .frame(width: 400, height: 300)
}
