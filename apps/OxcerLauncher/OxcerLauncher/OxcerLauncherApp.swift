//  OxcerLauncherApp.swift
//  OxcerLauncher
//
//  macOS-only SwiftUI app; Rust core via oxcer_ffi.dylib.

import AppKit
import SwiftUI

/// Wrapper view that observes @AppStorage("appTheme") and applies color scheme.
/// Placed in a View (not App) so SwiftUI properly re-renders when theme changes from Settings.
///
/// Architecture note — do NOT add `.id(appTheme)` to the `RootView()` call below:
///
/// `.id()` is a **SwiftUI identity modifier**, not a styling hint. When its value changes,
/// SwiftUI considers the old and new views to be completely unrelated nodes, destroys the
/// old subtree, and builds a fresh one from scratch. Any `@StateObject` owned by that
/// subtree — including `AppViewModel` — is discarded and recreated with default state,
/// which means `isModelReady = false` and the onboarding overlay reappears.
///
/// This mistake is easy to reintroduce because `.id(x)` looks like a harmless "refresh
/// key". It is not. Reserve `.id()` for cases where you explicitly want the view to reset
/// (e.g. resetting a form). For theme switching, `@AppStorage` already causes this view's
/// body to re-evaluate when `appTheme` changes, and `.preferredColorScheme()` propagates
/// the updated scheme through the SwiftUI environment — no subtree recreation needed.
///
/// Architecture note — why we also call NSApp.appearance directly:
///
/// SwiftUI's `.preferredColorScheme` sets the window-level appearance (`NSWindow.appearance`).
/// On some macOS versions, passing `nil` does not immediately reset `NSApp.appearance` to nil
/// after a forced Light/Dark override — the window's AppKit rendering pipeline keeps the old
/// appearance until the next layout cycle (often triggered by closing the Settings window).
///
/// The fix is to also drive `NSApp.appearance` explicitly. Setting `NSApp.appearance = nil`
/// is a synchronous AppKit call that immediately makes ALL windows (main window + the separate
/// Settings scene window) follow the current macOS system appearance. Switching back to
/// "System" therefore takes effect the moment the segmented control is tapped, with no
/// window-close or layout trick required.
private struct RootWindowContent: View {
    @AppStorage("appTheme") private var appTheme: String = "system"

    /// Persisted across launches. False until the user actively taps "Accept & Continue"
    /// in LlamaConsentView. The model is not loaded or downloaded until this is true.
    @AppStorage("hasAcceptedLlamaLicense") private var hasAcceptedLlamaLicense: Bool = false

    private var selectedScheme: ColorScheme? {
        if appTheme == "light" { return .light }
        if appTheme == "dark" { return .dark }
        return nil // System: let SwiftUI environment fall through to the macOS default
    }

    var body: some View {
        RootView()
            .frame(minWidth: 500, minHeight: 400)
            .preferredColorScheme(selectedScheme)
            // Apply the AppKit appearance layer immediately on launch and on every change.
            // This ensures both the main window and the separate Settings scene window
            // update without requiring the Settings window to be closed.
            .onAppear { applyNSAppAppearance(appTheme) }
            .onChange(of: appTheme) { _, newTheme in applyNSAppAppearance(newTheme) }
            // Restore cloud engine from persisted settings on every cold launch.
            // CloudSettingsViewModel does the same in its init(), but that view model
            // is only instantiated when the Settings window is opened. This task ensures
            // the engine is wired up even if the user never opens Settings.
            .task { restoreCloudEngineIfNeeded() }
            // ── Llama 3 consent gate ─────────────────────────────────────────
            // Sheet is non-dismissable (interactiveDismissDisabled) so the user
            // must make an explicit choice before the model is loaded or downloaded.
            // When hasAcceptedLlamaLicense flips to true inside LlamaConsentView,
            // the binding's get closure returns false and SwiftUI dismisses the sheet.
            .sheet(
                isPresented: Binding(
                    get: { !hasAcceptedLlamaLicense },
                    set: { _ in } // only the buttons inside control dismissal
                )
            ) {
                LlamaConsentView(
                    onAccept: { /* hasAcceptedLlamaLicense set inside the view */ },
                    onDecline: { NSApp.terminate(nil) }
                )
                .interactiveDismissDisabled(true)
            }
    }

    /// Activate the cloud LLM provider if the user had it enabled in a previous session.
    ///
    /// Reads `useCloudModel`, `selectedProvider`, and the saved Keychain API key —
    /// the same values `CloudSettingsViewModel` uses — and calls `activateCloudProvider`
    /// so that `generate_text` is already routed to the cloud engine before the user
    /// sends their first message.
    private func restoreCloudEngineIfNeeded() {
        guard UserDefaults.standard.bool(forKey: "useCloudModel") else { return }
        let rawProvider = UserDefaults.standard.string(forKey: "selectedProvider") ?? ""
        guard let provider = ProviderKind(rawValue: rawProvider) else { return }
        let key = KeychainHelper.load(key: provider.keychainKey) ?? ""
        guard !key.isEmpty else { return }
        do {
            try OxcerLauncher.activateCloudProvider(provider: provider.ffiKind, apiKey: key)
        } catch {
            // Non-fatal: the engine falls back to local Llama.
        }
    }

    /// Sets NSApp.appearance so that all windows update immediately.
    ///
    /// Light/Dark: forces the named appearance app-wide.
    /// System (any other value): sets nil so every window follows the macOS system preference.
    private func applyNSAppAppearance(_ theme: String) {
        switch theme {
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        case "dark":  NSApp.appearance = NSAppearance(named: .darkAqua)
        default:      NSApp.appearance = nil
        }
    }
}

@main
struct OxcerLauncherApp: App {
    var body: some Scene {
        WindowGroup {
            RootWindowContent()
        }
        .windowStyle(.hiddenTitleBar)
        .commands {
            CommandGroup(replacing: .newItem) { }
        }

        Settings {
            SettingsView()
        }
    }
}
