# Oxcer architecture

## Overview

Oxcer is split into two parts:

1. **Oxcer (Tauri)** — Local backend providing commands over Tauri IPC / sidecar mechanisms.
2. **Swift app** — Native macOS UI that talks to Oxcer's backend.

## Backend (Tauri)

- **Role:** Exposes commands (FS, Shell, Security, Agent, Settings) via Tauri's invoke/event system.
- **No HTML/WebView UI:** The `src/` directory contains only a minimal placeholder `index.html` required by Tauri's build. The window is hidden (`visible: false`).
- **Launch:** Run as a sidecar process from the Swift app, or standalone with `pnpm tauri dev` for development. The backend currently uses a hidden window; it can be evolved into a tray app or pure daemon if needed.
- **Commands:** `cmd_fs_*`, `cmd_shell_run`, `cmd_approve_and_execute`, `cmd_settings_get`, `cmd_settings_save`, `cmd_workspace_add`, `cmd_workspace_remove`, `cmd_models_list`.

## Swift app (future)

- **Role:** Primary UI/launcher for users. Native macOS windows, dialogs, and controls.
- **Communication:** Invokes Oxcer backend commands via Tauri IPC or sidecar mechanisms.
- **Responsibilities:** Settings screen, approval modals (HITL), workspace picker, model selection — all implemented natively in Swift.

## Legacy WebView UI

The former HTML/TypeScript WebView (Settings, approval modals, test flows) is archived in `reference/legacy_ui/` for reference. The Swift app will implement equivalent functionality using native APIs.
