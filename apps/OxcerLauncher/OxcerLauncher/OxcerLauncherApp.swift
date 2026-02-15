//  OxcerLauncherApp.swift
//  OxcerLauncher
//
//  macOS-only SwiftUI app; Rust core via oxcer_ffi.dylib.

import SwiftUI

/// Wrapper view that observes @AppStorage("appTheme") and applies color scheme.
/// Placed in a View (not App) so SwiftUI properly re-renders when theme changes from Settings.
private struct RootWindowContent: View {
    @AppStorage("appTheme") private var appTheme: String = "system"

    private var selectedScheme: ColorScheme? {
        if appTheme == "light" { return .light }
        if appTheme == "dark" { return .dark }
        return nil // System
    }

    var body: some View {
        RootView()
            .frame(minWidth: 500, minHeight: 400)
            .preferredColorScheme(selectedScheme)
            .id(appTheme)
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
