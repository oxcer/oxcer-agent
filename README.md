# Oxcer
AI Assistant in your computer.

Oxcer is built for privacy. It works entirely on your device, keeping your data safe and never sending anything to the cloud.


Special Thanks to

- OpenClaw: for Core Idea
- Nvidia, vLLM: for Guardrails and Semantic Router Structure
- Cursor, Google, Perplexity: for AI tools 

for all Computer Scientist, Researchers 


## Repo layout

```
core/
  oxcer-core/      # shared Rust core
  oxcer_ffi/       # Rust FFI layer for native UIs
  plugins/         # YAML plugins
  config/          # policies, defaults, etc.

apps/
  desktop-tauri/       # cross-platform desktop shell around oxcer-core (no real web UI)
  OxcerLauncher/       # macOS native SwiftUI launcher
  windows-launcher/    # planned WinUI 3 launcher (Windows native, stub only)
```

```mermaid
flowchart TB
  subgraph core["Core"]
    oc[oxcer-core]
    ffi[oxcer_ffi]
  end
  ffi --> oc

  subgraph shells["Desktop shells / launchers"]
    dt[apps/desktop-tauri (Tauri shell, no real web UI)]
    ol[apps/OxcerLauncher (SwiftUI)]
    wl[apps/windows-launcher (WinUI 3, planned)]
  end

  dt -->|Rust commands (src-tauri)| oc
  ol -->|FFI dylib| ffi
  wl -.->|planned: FFI or CLI| oc
```

> See each app’s README for app-level diagrams and details.

## Architecture

- **Oxcer (Tauri)** = backend only, no HTML/WebView UI. Local agent backend exposing commands (FS, Shell, Security, Agent) over Tauri IPC / sidecar mechanisms.
- **Swift app (OxcerLauncher)** = primary UI/launcher for users. Native macOS SwiftUI app that talks to the Rust core via the **oxcer_ffi** C API (JSON in/out). No webviews; SwiftUI only.

The Tauri app’s frontend is a minimal placeholder (see `apps/desktop-tauri/dist` after build). The former WebView UI is archived in `reference/legacy_ui/`.

**Development:** Core logic lives in `oxcer-core` (no Tauri); test with `cargo test -p oxcer-core`. The Tauri app lives in `apps/desktop-tauri/`; run with `pnpm tauri dev` from repo root. See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for workflow.

### OxcerLauncher (macOS SwiftUI)

- **Build Rust FFI:** `cargo build --release -p oxcer_ffi` (from repo root).
- **Open and run:** Open [apps/OxcerLauncher/OxcerLauncher.xcodeproj](apps/OxcerLauncher/OxcerLauncher.xcodeproj) in Xcode; the first build phase runs the same `cargo` command, then links/embeds the dylib. Press **⌘R** to run.
- **Docs and FFI contracts:** [apps/OxcerLauncher/README.md](apps/OxcerLauncher/README.md).
