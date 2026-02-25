//  SettingsView.swift
//  OxcerLauncher
//
//  Local-only settings. Sensitive data in Keychain; non-sensitive in @AppStorage.

import SwiftUI

// MARK: - Storage Keys

private enum SettingsKey {
    static let appTheme = "appTheme"
    // useCloudModel and provider API keys are now managed by CloudSettingsViewModel.
}

// MARK: - SettingsView

struct SettingsView: View {
    @AppStorage(SettingsKey.appTheme) private var appTheme: String = "system"

    var body: some View {
        ScrollView {
            VStack(spacing: 24) {
                // Section 1: Appearance
                settingsSection(title: "Appearance") {
                    Picker("Theme", selection: $appTheme) {
                        Text("System").tag("system")
                        Text("Light").tag("light")
                        Text("Dark").tag("dark")
                    }
                    .pickerStyle(.segmented)
                }

                // Section 2: Intelligence
                // Provider selection, API key, and connection test are managed by
                // CloudModelSettingsView and its @StateObject CloudSettingsViewModel.
                settingsSection(title: "Intelligence") {
                    CloudModelSettingsView()
                }

                // Section 3: About
                settingsSection(title: "About") {
                    VStack(alignment: .leading, spacing: 14) {

                        // Version
                        HStack {
                            Text("Version")
                                .font(.subheadline)
                                .foregroundStyle(OxcerTheme.textSecondary)
                            Spacer()
                            Text(versionInfo)
                                .font(.subheadline)
                                .foregroundStyle(OxcerTheme.textPrimary)
                        }

                        Divider()

                        // "Built with Meta Llama 3" — required by §1.b.i of the
                        // Meta Llama 3 Community License. Must appear prominently
                        // in the product UI or documentation.
                        HStack(spacing: 8) {
                            Image(systemName: "cpu")
                                .font(.subheadline)
                                .foregroundStyle(OxcerTheme.accent)
                            Text("Built with Meta Llama 3")
                                .font(.subheadline.weight(.medium))
                                .foregroundStyle(OxcerTheme.textPrimary)
                            Spacer()
                        }

                        // Clickable license links
                        HStack(spacing: 20) {
                            Link(
                                "Model License",
                                destination: URL(string: "https://llama.meta.com/llama3/license/")!
                            )
                            .font(.caption)
                            .foregroundStyle(OxcerTheme.accent)

                            Link(
                                "Acceptable Use Policy",
                                destination: URL(string: "https://llama.meta.com/llama3/use-policy/")!
                            )
                            .font(.caption)
                            .foregroundStyle(OxcerTheme.accent)
                        }

                        // Verbatim attribution text required by the Meta Llama 3 license
                        Text(
                            "Meta Llama 3 is licensed under the Meta Llama 3 Community License, " +
                            "Copyright © Meta Platforms, Inc. All Rights Reserved."
                        )
                        .font(.caption2)
                        .foregroundStyle(OxcerTheme.textTertiary)
                        .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(24)
        }
        .background(Color("OxcerBackground"))
        .frame(width: 500, height: 600)
    }

    private var versionInfo: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.0"
        let build = Bundle.main.infoDictionary?["CFBundleVersion"] as? String ?? "1"
        return "\(version) (\(build))"
    }

    private func settingsSection<Content: View>(title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title)
                .font(.headline)
                .foregroundStyle(Color("OxcerTextPrimary"))

            content()
                .padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(
                    RoundedRectangle(cornerRadius: 16)
                        .fill(Color("OxcerSurface"))
                )
        }
    }
}

#Preview {
    SettingsView()
}
