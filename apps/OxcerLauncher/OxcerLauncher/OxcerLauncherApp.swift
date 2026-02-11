//  OxcerLauncherApp.swift
//  OxcerLauncher
//
//  macOS-only SwiftUI app; Rust core via oxcer_ffi.dylib.

import SwiftUI

@main
struct OxcerLauncherApp: App {
    @AppStorage("appTheme") private var appTheme: String = "system"

    private var preferredScheme: ColorScheme? {
        switch appTheme {
        case "light": return .light
        case "dark": return .dark
        default: return nil
        }
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
                .frame(minWidth: 500, minHeight: 400)
                .preferredColorScheme(preferredScheme)
        }
        .windowStyle(.automatic)
        .commands {
            CommandGroup(replacing: .newItem) { }
        }

        Settings {
            SettingsView()
        }
    }
}
