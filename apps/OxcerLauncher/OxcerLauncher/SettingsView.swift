//  SettingsView.swift
//  OxcerLauncher
//
//  Local-only settings. Sensitive data in Keychain; non-sensitive in @AppStorage.

import SwiftUI

// MARK: - Storage Keys

private enum SettingsKey {
    static let appTheme = "appTheme"
    static let useCloudModel = "useCloudModel"
    static let userApiKey = "userApiKey"
}

// MARK: - SettingsView

struct SettingsView: View {
    @AppStorage(SettingsKey.appTheme) private var appTheme: String = "system"
    @AppStorage(SettingsKey.useCloudModel) private var useCloudModel: Bool = false
    @KeychainStorage(key: SettingsKey.userApiKey) private var userApiKey: String = ""

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
                settingsSection(title: "Intelligence") {
                    VStack(alignment: .leading, spacing: 16) {
                        Toggle("Use Cloud Model (Higher Performance)", isOn: $useCloudModel)
                            .toggleStyle(.switch)

                        if useCloudModel {
                            // Orange warning row
                            HStack(spacing: 12) {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .font(.body)
                                    .foregroundStyle(Color.orange)
                                Text("Data leaves device.")
                                    .font(.subheadline)
                                    .foregroundStyle(OxcerTheme.textSecondary)
                            }
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(
                                RoundedRectangle(cornerRadius: 12)
                                    .fill(Color.orange.opacity(0.12))
                            )

                            SecureField("API Key", text: $userApiKey)
                                .textFieldStyle(.plain)
                                .font(.subheadline)
                                .foregroundStyle(OxcerTheme.textPrimary)
                                .padding(12)
                                .background(
                                    RoundedRectangle(cornerRadius: 12)
                                        .fill(Color("OxcerBackground"))
                                )
                                .overlay(
                                    RoundedRectangle(cornerRadius: 12)
                                        .stroke(OxcerTheme.border, lineWidth: 1)
                                )
                        } else {
                            // Green shield row
                            HStack(spacing: 12) {
                                Image(systemName: "checkmark.shield.fill")
                                    .font(.body)
                                    .foregroundStyle(Color.green)
                                Text("Running on-device (Phi-3). Private & Fast.")
                                    .font(.subheadline)
                                    .foregroundStyle(OxcerTheme.textSecondary)
                            }
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(
                                RoundedRectangle(cornerRadius: 12)
                                    .fill(Color.green.opacity(0.12))
                            )
                        }
                    }
                }

                // Section 3: About
                settingsSection(title: "About") {
                    HStack {
                        Text("Version")
                            .font(.subheadline)
                            .foregroundStyle(OxcerTheme.textSecondary)
                        Spacer()
                        Text(versionInfo)
                            .font(.subheadline)
                            .foregroundStyle(OxcerTheme.textPrimary)
                    }
                }
            }
            .padding(24)
        }
        .background(Color("OxcerBackground"))
        .frame(width: 500, height: 400)
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
