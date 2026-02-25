//  CloudModelSettingsView.swift
//  OxcerLauncher
//
//  Intelligence section of the Settings window.
//  Manages the cloud vs. local toggle, provider picker, API key entry, and
//  the "Test Connection" button with inline success/failure feedback.
//
//  Usage: embed as the content of the Intelligence settings section in SettingsView.
//  The view owns its own @StateObject so it manages lifecycle automatically.

import SwiftUI
import OSLog

// MARK: - CloudModelSettingsView

struct CloudModelSettingsView: View {
    @StateObject private var vm = CloudSettingsViewModel()

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {

            // ── Toggle ────────────────────────────────────────────────────────
            Toggle("Use Cloud Model (Higher Performance)", isOn: $vm.useCloudModel)
                .toggleStyle(.switch)

            if vm.useCloudModel {
                cloudSection
            } else {
                localSection
            }
        }
    }

    // MARK: Cloud section

    @ViewBuilder
    private var cloudSection: some View {
        VStack(alignment: .leading, spacing: 14) {

            // Warning banner: data leaves device
            HStack(spacing: 12) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.body)
                    .foregroundStyle(Color.orange)
                Text("Data leaves your device when using a cloud provider.")
                    .font(.subheadline)
                    .foregroundStyle(OxcerTheme.textSecondary)
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                RoundedRectangle(cornerRadius: 12)
                    .fill(Color.orange.opacity(0.12))
            )

            // Provider picker
            HStack {
                Text("Provider")
                    .font(.subheadline)
                    .foregroundStyle(OxcerTheme.textSecondary)
                    .frame(width: 72, alignment: .leading)

                Picker("", selection: $vm.selectedProvider) {
                    ForEach(ProviderKind.allCases) { provider in
                        Text(provider.displayName).tag(provider)
                    }
                }
                .pickerStyle(.menu)
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            // API key field
            HStack {
                Text("API Key")
                    .font(.subheadline)
                    .foregroundStyle(OxcerTheme.textSecondary)
                    .frame(width: 72, alignment: .leading)

                SecureField("Paste your API key here", text: $vm.apiKey)
                    .textFieldStyle(.plain)
                    .font(.subheadline)
                    .foregroundStyle(OxcerTheme.textPrimary)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(
                        RoundedRectangle(cornerRadius: 10)
                            .fill(Color("OxcerBackground"))
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 10)
                            .stroke(OxcerTheme.border, lineWidth: 1)
                    )
            }

            // Test Connection button + inline status
            HStack(spacing: 12) {
                Button {
                    Task { await vm.testConnection() }
                } label: {
                    HStack(spacing: 6) {
                        if vm.isTesting {
                            ProgressView()
                                .scaleEffect(0.7)
                                .frame(width: 14, height: 14)
                        }
                        Text(vm.isTesting ? "Testing…" : "Test Connection")
                            .font(.subheadline.weight(.medium))
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 7)
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(vm.isTesting)
                .keyboardShortcut(.return, modifiers: [])

                connectionStatusView
            }
        }
    }

    // MARK: Local section

    @ViewBuilder
    private var localSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 12) {
                Image(systemName: "checkmark.shield.fill")
                    .font(.body)
                    .foregroundStyle(Color.green)
                Text("Running on-device (Meta Llama 3 8B Instruct). Private and fast.")
                    .font(.subheadline)
                    .foregroundStyle(OxcerTheme.textSecondary)
            }
            Text("Recommended minimum: 8 GB unified memory for comfortable on-device inference.")
                .font(.caption)
                .foregroundStyle(OxcerTheme.textSecondary.opacity(0.75))
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.green.opacity(0.12))
        )
    }

    // MARK: Connection status indicator

    @ViewBuilder
    private var connectionStatusView: some View {
        switch vm.connectionStatus {
        case .idle:
            EmptyView()
        case .success(let message):
            Label(message, systemImage: "checkmark.circle.fill")
                .font(.caption)
                .foregroundStyle(Color.green)
                .lineLimit(2)
                .transition(.opacity)
        case .failure(let message):
            Label(message, systemImage: "xmark.circle.fill")
                .font(.caption)
                .foregroundStyle(Color.red)
                .lineLimit(2)
                .transition(.opacity)
        }
    }
}

// MARK: - Preview

#Preview {
    CloudModelSettingsView()
        .padding(24)
        .frame(width: 460)
        .background(Color("OxcerBackground"))
}
