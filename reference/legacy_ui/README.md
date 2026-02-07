# Legacy WebView UI

This directory contains the former Tauri WebView (HTML/TypeScript) frontend for Oxcer.

**Status:** Archived. Oxcer's UI is now a **native macOS Swift app**. The Tauri side is a **backend-only** process exposing commands (FS, Shell, Security, Agent) via IPC.

**Preserved for reference:** The Settings screen, approval modals, and test flows implemented here may inform the Swift UI implementation. The Swift app will use native macOS APIs for dialogs, windows, and configuration.

## Contents

- `index.html` — Original HTML shell (main view, settings view, loading)
- `app.ts` — Approval modal, Settings (Basic/Advanced tabs), workspace management, model dropdown
