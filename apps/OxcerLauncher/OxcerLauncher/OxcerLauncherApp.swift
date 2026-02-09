//  OxcerLauncherApp.swift
//  OxcerLauncher
//
//  macOS-only SwiftUI app; Rust core via oxcer_ffi.dylib.

import SwiftUI

@main
struct OxcerLauncherApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
                .frame(minWidth: 500, minHeight: 400)
        }
        .windowStyle(.automatic)
        .commands {
            CommandGroup(replacing: .newItem) { }
        }
    }
}
