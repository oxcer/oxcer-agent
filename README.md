# Oxcer
AI Assistant in your computer.

Oxcer is built for privacy. It works entirely on your device, keeping your data safe and never sending anything to the cloud.


Special Thanks to

- OpenClaw: for Core Idea
- Nvidia, vLLM: for Guardrails and Semantic Router Structure
- Cursor, Google, Perplexity: for AI tools 

for all Computer Scientist, Researchers 


## Architecture

- **Oxcer (Tauri)** = backend only, no HTML/WebView UI. Local agent backend exposing commands (FS, Shell, Security, Agent) over Tauri IPC / sidecar mechanisms.
- **Swift app (OxcerLauncher)** = primary UI/launcher for users. Native macOS SwiftUI app that talks to the Rust core via the **oxcer_ffi** C API (JSON in/out). No webviews; SwiftUI only.

The `src/` directory contains only a minimal placeholder `index.html` required by Tauri. The former WebView UI is archived in `reference/legacy_ui/`.

**Development:** Core logic lives in `oxcer-core` (no Tauri); test with `cargo test -p oxcer-core`. The Tauri app in `src-tauri` is a thin backend launcher; run with `pnpm tauri dev`. See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for workflow.

### OxcerLauncher (macOS SwiftUI)

- **Build Rust FFI:** `cargo build --release -p oxcer_ffi` (from repo root).
- **Open and run:** Open [OxcerLauncher/OxcerLauncher.xcodeproj](OxcerLauncher/OxcerLauncher.xcodeproj) in Xcode; the first build phase runs the same `cargo` command, then links/embeds the dylib. Press **⌘R** to run.
- **Docs and FFI contracts:** [OxcerLauncher/README.md](OxcerLauncher/README.md).
