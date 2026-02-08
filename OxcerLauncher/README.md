# OxcerLauncher (macOS SwiftUI)

Minimal macOS SwiftUI launcher for the Oxcer Rust core. All business logic runs in Rust; the app calls into the core via the `oxcer_ffi` C API (UTF-8 JSON in/out). No webviews or HTML/JS — SwiftUI only.

## Requirements

- **macOS 14+**, **Xcode 15+**
- **Rust** toolchain (for building `liboxcer_ffi.dylib`)

## 1. Building `oxcer_ffi` for macOS

From the **repository root** (parent of `OxcerLauncher/`):

```bash
cargo build --release -p oxcer_ffi
```

This produces **`target/release/liboxcer_ffi.dylib`**. The Xcode project is set up to run this command automatically before linking (see below), so you can also build from Xcode without running `cargo` yourself.

## 2. Opening, building, and running OxcerLauncher in Xcode

1. **Open the project**  
   Open `OxcerLauncher.xcodeproj` in Xcode (e.g. double‑click the file or **File → Open**).

2. **Select scheme and destination**  
   - Scheme: **OxcerLauncher**  
   - Destination: **My Mac**

3. **Build and run**  
   - **⌘B** to build, **⌘R** to run.

On **Build**, Xcode runs a **Run Script** phase first: it runs `cargo build -p oxcer_ffi --release` from the repo root, then compiles Swift and links/embeds `liboxcer_ffi.dylib` into the app bundle. The app’s runpath is set so it loads the dylib from `Contents/Frameworks` at runtime.

## 3. Integration checklist

After building and running:

- **Launch** — The app window opens (Task tab + Recent Sessions tab).
- **Workspaces** — Uses the same config as the Rust core: **`~/Library/Application Support/Oxcer/config.json`**. Add a `workspaces` array there (see [CONFIG_SCHEMA](../../docs/CONFIG_SCHEMA.md)) so the workspace dropdown lists real folders.
- **Run Task** — Enter a task, choose a workspace, press **Run Task**. The FFI calls `oxcer_agent_request`. With the current stub executor, tasks that need tools will return an error; simple flows still demonstrate the round‑trip.
- **Recent Sessions** — The **Recent Sessions** tab lists sessions from **`~/Library/Application Support/Oxcer/logs/*.jsonl`** (same JSONL logs as the Tauri app). Select a row to load and show the event timeline.

## 4. FFI functions and JSON contracts

All functions take and return **UTF-8 JSON strings**. The caller must call **`oxcer_string_free(ptr)`** on every non‑null pointer returned (the Swift wrapper in `OxcerFFI.swift` does this).

| Function | Input JSON | Output JSON |
|----------|------------|-------------|
| **`oxcer_list_workspaces`** | `{}` or `{ "app_config_dir": "/path" }` | `{ "workspaces": [ { "id", "name", "root_path" }, ... ] }` |
| **`oxcer_list_sessions`** | `{}` or `{ "app_config_dir": "/path" }` | Array of session summaries: `{ "session_id", "start_timestamp", "end_timestamp", "total_cost_usd", "success", "tool_calls_count", "approvals_count", "denies_count" }` |
| **`oxcer_load_session_log`** | `{ "session_id": "..." }` and optional `"app_config_dir"` | Array of `LogEvent` (timestamp, session_id, caller, component, action, decision, metrics, details). |
| **`oxcer_agent_request`** | `{ "task_description": "...", "workspace_id"?, "workspace_root"?, "context"? }` and optional `"app_config_dir"` | `{ "ok": true, "answer": "...", "error": null }` or `{ "ok": false, "error": "..." }`. |
| **`oxcer_string_free`** | `ptr` (pointer previously returned by any of the above) | — (void). Frees the string; safe to call with `NULL`. |

Config and logs path: if `app_config_dir` is omitted, the Rust side uses the default (e.g. on macOS `~/Library/Application Support/Oxcer`).

## Optional: use a copy of the dylib

If you prefer not to run the Run Script phase:

1. Create `OxcerLauncher/Libs/` if needed.
2. Copy `target/release/liboxcer_ffi.dylib` into `OxcerLauncher/Libs/`.
3. In Xcode, remove or disable the **“Build Rust dylib”** Run Script phase, and in **Frameworks** and **Embed Libraries** use `Libs/liboxcer_ffi.dylib` instead of the **Rust release** reference.

## Bundle identifier

Placeholder: **`com.oxcer.launcher`**. Change it in the target’s **Signing & Capabilities** (or in the project file) if needed.
