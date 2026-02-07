# Oxcer
Personal AI in your computer.

## Architecture

- **Oxcer (Tauri)** = backend only, no HTML/WebView UI. Local agent backend exposing commands (FS, Shell, Security, Agent) over Tauri IPC / sidecar mechanisms.
- **Swift app** = primary UI/launcher for users. Native macOS app that talks to Oxcer's backend.

The `src/` directory contains only a minimal placeholder `index.html` required by Tauri. The former WebView UI is archived in `reference/legacy_ui/`.

**Development:** Core logic lives in `oxcer-core` (no Tauri); test with `cargo test -p oxcer-core`. The Tauri app in `src-tauri` is a thin backend launcher; run with `pnpm tauri dev`. See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for workflow.
