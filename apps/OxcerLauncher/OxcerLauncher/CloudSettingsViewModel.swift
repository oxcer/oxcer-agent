//  CloudSettingsViewModel.swift
//  OxcerLauncher
//
//  ViewModel for cloud provider settings.
//  Manages provider selection, per-provider API key storage, and connection testing.
//
//  Persistence:
//    - useCloudModel:     UserDefaults ("useCloudModel")
//    - selectedProvider:  UserDefaults ("selectedProvider")
//    - API keys:          Keychain, one slot per provider ("oxcer.apikey.<provider>")
//
//  Engine activation:
//    - `syncEngineState()` calls `activateCloudProvider` / `deactivateCloudProvider` so
//      that `generate_text` on the Rust side routes to the user-selected engine without
//      any changes to the FSM, tool layer, or approval gates.
//
//  Threading: @MainActor — all @Published mutations happen on the main actor.

import Foundation
import OSLog

private let cloudLogger = Logger(subsystem: "com.oxcer.launcher", category: "CloudSettings")

// MARK: - ProviderKind

/// Swift-side provider enum. Mirrors `FfiProviderKind` from the Rust FFI layer.
/// Provides display names, Keychain keys, and FfiProviderKind conversion.
enum ProviderKind: String, CaseIterable, Identifiable, Codable {
    case openAI    = "openAI"
    case anthropic = "anthropic"
    case gemini    = "gemini"
    case grok      = "grok"

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .openAI:    return "OpenAI (ChatGPT)"
        case .anthropic: return "Anthropic (Claude)"
        case .gemini:    return "Google (Gemini)"
        case .grok:      return "xAI (Grok)"
        }
    }

    /// Keychain item key for this provider's API key.
    var keychainKey: String { "oxcer.apikey.\(rawValue)" }

    /// Maps to the UniFFI-generated `FfiProviderKind` for Rust FFI calls.
    var ffiKind: FfiProviderKind {
        switch self {
        case .openAI:    return .openAi
        case .anthropic: return .anthropic
        case .gemini:    return .gemini
        case .grok:      return .grok
        }
    }
}

// MARK: - ConnectionStatus

enum ConnectionStatus: Equatable {
    case idle
    case success(String)
    case failure(String)
}

// MARK: - CloudSettingsViewModel

@MainActor
final class CloudSettingsViewModel: ObservableObject {

    // ── Persisted settings ────────────────────────────────────────────────────

    @Published var useCloudModel: Bool {
        didSet {
            UserDefaults.standard.set(useCloudModel, forKey: "useCloudModel")
            cloudLogger.info("useCloudModel set to \(self.useCloudModel, privacy: .public)")
            syncEngineState()
        }
    }

    @Published var selectedProvider: ProviderKind {
        didSet {
            // Save the departing provider's key before loading the new one.
            KeychainHelper.save(key: oldValue.keychainKey, value: apiKey)
            // Load the new provider's key (empty string if not yet set).
            apiKey = KeychainHelper.load(key: selectedProvider.keychainKey) ?? ""
            connectionStatus = .idle
            UserDefaults.standard.set(selectedProvider.rawValue, forKey: "selectedProvider")
            cloudLogger.info("selectedProvider changed to \(self.selectedProvider.rawValue, privacy: .public)")
            // syncEngineState is called by apiKey.didSet which fires from the line above.
        }
    }

    /// API key for the currently selected provider.
    /// Written to Keychain on every change so that switching providers does not lose in-progress input.
    @Published var apiKey: String = "" {
        didSet {
            KeychainHelper.save(key: selectedProvider.keychainKey, value: apiKey)
            // Reset connection status whenever the key changes.
            if connectionStatus != .idle { connectionStatus = .idle }
            syncEngineState()
        }
    }

    // ── Connection test state ─────────────────────────────────────────────────

    @Published var connectionStatus: ConnectionStatus = .idle
    @Published var isTesting: Bool = false

    // MARK: init

    init() {
        let rawProvider = UserDefaults.standard.string(forKey: "selectedProvider") ?? ""
        let provider = ProviderKind(rawValue: rawProvider) ?? .openAI
        self.useCloudModel = UserDefaults.standard.bool(forKey: "useCloudModel")
        self.selectedProvider = provider
        self.apiKey = KeychainHelper.load(key: provider.keychainKey) ?? ""
        // Restore engine state from persisted settings (e.g. after app relaunch).
        syncEngineState()
    }

    // MARK: Engine state sync

    /// Activate or deactivate the cloud engine based on current settings.
    ///
    /// Called from every `didSet` and `init` so the Rust engine slot always
    /// reflects `useCloudModel` + `selectedProvider` + saved API key.
    /// The FSM, tool layer, and approval gates are never aware of this call.
    private func syncEngineState() {
        let trimmedKey = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        if useCloudModel && !trimmedKey.isEmpty {
            do {
                try OxcerLauncher.activateCloudProvider(
                    provider: selectedProvider.ffiKind,
                    apiKey: trimmedKey
                )
                cloudLogger.info(
                    "activateCloudProvider ok provider=\(self.selectedProvider.rawValue, privacy: .public)"
                )
            } catch {
                cloudLogger.error(
                    "activateCloudProvider failed: \(error.localizedDescription, privacy: .public)"
                )
            }
        } else {
            OxcerLauncher.deactivateCloudProvider()
            cloudLogger.debug("deactivateCloudProvider called")
        }
    }

    // MARK: Actions

    /// Perform a one-token health-check against the selected provider.
    /// Updates `connectionStatus` with a user-readable result.
    func testConnection() async {
        let trimmedKey = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedKey.isEmpty else {
            connectionStatus = .failure("Enter an API key before testing the connection.")
            return
        }

        isTesting = true
        connectionStatus = .idle
        defer { isTesting = false }

        cloudLogger.info(
            "testConnection start provider=\(self.selectedProvider.rawValue, privacy: .public)"
        )

        let result = await OxcerLauncher.testCloudProvider(
            provider: selectedProvider.ffiKind,
            apiKey: trimmedKey
        )

        cloudLogger.info(
            "testConnection done ok=\(result.ok, privacy: .public) msg=\(result.message, privacy: .public)"
        )

        connectionStatus = result.ok
            ? .success(result.message)
            : .failure(result.message)
    }
}
