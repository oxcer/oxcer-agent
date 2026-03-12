# Deep Scan: Settings, Local LLM, and Data Flow

> **STALE (v0.1):** Written when local LLM (then Phi-3) was a stub with GGUF loading unwired. In v0.1, Llama-3 8B Instruct is fully wired via llama.cpp + Metal. SettingsView is implemented in SwiftUI. The data-flow diagram and FFI surface notes are superseded by `docs/ARCHITECTURE.md`.

Structural overview of where Settings and Local LLM (Phi-3) live, how config is persisted, what is exposed to Swift via FFI, and the SwiftUI -> ViewModel -> Rust data flow.

---

## 1. Settings & Persistence

### 1.1 Where is the config/settings struct defined?

| Location | Type | Scope |
|----------|------|--------|
| **`apps/desktop-tauri/src-tauri/src/settings.rs`** | **`AppSettings`** | Tauri app only (not in oxcer-core or oxcer_ffi). |
| **`oxcer-core/src/fs.rs`** | `AppFsContext`, `WorkspaceRoot` | Core FS context (workspace roots + app config path). No “app config” struct; only `BaseDirKind::AppConfig` as a path kind. |
| **`oxcer_ffi/src/lib.rs`** | (none) | FFI has **no** config/settings struct. Only reads `config.json` for **workspaces** via internal `ConfigFileDto` (not exported). |

So: the **canonical** settings type is **`AppSettings`** in the **Tauri crate** (`oxcer`), not in `oxcer-core` or `oxcer_ffi`.

**`AppSettings`** (from `settings.rs`):

- `workspace_directories: Vec<WorkspaceDirectory>` (id, name, path)
- `default_model_id: String`
- `advanced: AdvancedSettings` -> `allow_destructive_fs_without_hitl: bool`
- `observability: ObservabilityOptions` -> `max_session_cost_usd: f64`
- `llm: LlmSetup` -> `setup_complete: bool`, `profile: String` (e.g. `"local-only"`, `"hybrid"`)

### 1.2 How is it saved to disk?

- **Format:** JSON.
- **File:** `{app_config_dir}/config.json` (e.g. macOS `~/Library/Application Support/Oxcer/config.json`).
- **Mechanism:** `serde_json::to_string_pretty` + `std::fs::write`. No sled, no SQLite.
- **Functions:**  
  - **Load:** `settings::load(app_config_dir: &Path) -> AppSettings`  
  - **Save:** `settings::save(app_config_dir: &Path, settings: &AppSettings) -> Result<(), String>`

Legacy migration: if `config.json` is missing, `load()` tries `settings.json` and, on success, saves to `config.json` and removes the legacy file.

### 1.3 Is there any FFI (UniFFI) for Swift to read/write config?

**No.**

- **`oxcer_ffi`** (UniFFI) only exports:
  - `list_workspaces(app_config_dir)` -> reads `config.json` and returns **only** `Vec<WorkspaceInfo>` (id, name, root_path). No full config, no save.
  - `list_sessions`, `load_session_log`, `run_agent_task` — none of these read or write `AppSettings`.
- **Tauri** exposes settings to the **web frontend** via:
  - `cmd_settings_get` -> `AppSettings`
  - `cmd_settings_save(app, settings: AppSettings)` -> `()`
  - `get_config` -> raw `config.json` as `serde_json::Value`

Those Tauri commands are **not** available to the Swift app; Swift talks only to **oxcer_ffi** (the dylib).

**Conclusion:** Settings are **internal to the Tauri app**. To connect SwiftUI SettingsView to the **existing** Rust logic you must **expose** load/save (and an `AppSettings`-like struct) from **oxcer_ffi** (see §4).

---

## 2. Local LLM (Phi-3) Architecture

### 2.1 Where is the code that loads the phi-3-small model?

| Layer | File(s) | What it does |
|-------|---------|----------------|
| **Bootstrap** | `oxcer-core/src/llm/bootstrap.rs` | `create_engine_for_profile(profile_name, config_dir, models_dir, http_fallback_config)`. For profile `"local-only"` or engine `"local-phi3"` it calls `ensure_model_present("phi3-small", ...)` and `LocalPhi3Engine::new(&model_root)`. |
| **Model presence** | `oxcer-core/src/llm/model_downloader.rs` | `ensure_model_present("phi3-small", config_dir, models_dir, None)` ensures the directory `models_dir/phi3-small` exists and contains required files (see loader). Can download via `ModelDownloader` if missing. |
| **Loader** | `oxcer-core/src/llm/local_phi3/loader.rs` | `resolve_model_paths(model_root)` expects under `model_root`: `model.gguf`, `tokenizer.json`. Returns `Phi3ModelPaths`. `load_tokenizer(&paths.tokenizer_json)` loads the tokenizer (Hugging Face `tokenizers`). |
| **Engine** | `oxcer-core/src/llm/local_phi3/mod.rs` | `LocalPhi3Engine::new(model_root)` loads tokenizer + **runtime**. Runtime is currently a **stub** (see below). |

So “loading” = tokenizer from `tokenizer.json` + paths to `model.gguf`; actual **inference** is not yet implemented (stub).

### 2.2 Which inference engine is used?

- **Planned:** GGUF (and optionally ONNX). Comments in code: “TODO: Initialize llama.cpp or ONNX Runtime with paths.model_gguf.”
- **Current:** **Stub only.**  
  - `oxcer-core/src/llm/local_phi3/runtime.rs`: trait `PhiRuntime` with `generate(input_ids, params) -> Vec<u32>`.  
  - Only implementation: `StubPhiRuntime`, which returns `Ok(vec![])` (no real inference).  
- **Dependencies:** `oxcer-core/Cargo.toml` has **no** candle, burn, rust-bert, or llama-cpp bindings. It has `tokenizers` (Hugging Face) for encode/decode; inference backend is placeholder.

So: **model format** is **GGUF** (file `model.gguf`); **inference** is **stub** (no candle/burn/rust-bert/llama.cpp wired yet).

### 2.3 How does the Agent choose Local vs Cloud?

- **oxcer-core (orchestrator):** The agent does **not** choose. It emits **tool intents**, including `ToolCallIntent::LlmGenerate { strategy, task, ... }`. The **runner** (executor) is responsible for calling the LLM. So “local vs cloud” is decided in the **app** that implements the executor, not in the core orchestrator.
- **Tauri app:**  
  - **`cmd_llm_invoke`** (in `main.rs`) is the only LLM entrypoint used when executing agent steps. It dispatches by **model_id** (e.g. `gemini-2.5-flash`, `gpt-4o`) to **cloud** providers (Gemini, OpenAI, Anthropic, Grok) via HTTP.  
  - There is **no** branch in `cmd_llm_invoke` that calls `create_engine_for_profile` or `LocalPhi3Engine`. So in the current Tauri app, **all** LLM calls are **cloud**; local Phi-3 is **not** wired into the agent path.
- **oxcer_ffi:** `run_agent_task` uses `FfiStubExecutor`, which returns an error for every tool execution (and approval). So the Swift/FFI path never runs real tools or real LLM (local or cloud).

**Summary:**  
- **Core:** Agent emits `LlmGenerate`; executor (Tauri or FFI) decides how to run it.  
- **Tauri:** Uses only cloud APIs in `cmd_llm_invoke`; local Phi-3 exists in core but is **not** used by the agent.  
- **FFI:** No LLM at all (stub executor).

---

## 3. Data Flow: SwiftUI -> ViewModel -> Rust FFI -> Core

### 3.1 High-level

```
SwiftUI View (ContentView, future SettingsView)
    ↓ @StateObject / @ObservedObject
AppViewModel
    ↓ backend: OxcerBackend (protocol)
DefaultOxcerBackend
    ↓ Task.detached { try await listWorkspaces(...) }  (etc.)
UniFFI global functions (oxcer_ffi.swift)
    ↓ C FFI
oxcer_ffi (Rust dylib)
    ↓
oxcer_core (list_workspaces_impl, list_sessions, load_session_log, agent_request_impl)
```

- **Config dir:** ViewModel sets `appConfigDir` to `~/Library/Application Support/Oxcer` (same convention as Tauri). All FFI calls that need a directory receive this string.

### 3.2 Implemented flows

| UI action | ViewModel | Backend method | FFI function | Rust |
|-----------|-----------|----------------|---------------|------|
| Load workspaces | `loadWorkspaces()` | `listWorkspaces(appConfigDir:)` | `listWorkspaces(appConfigDir:)` | `list_workspaces` -> read `config.json` -> `Vec<WorkspaceInfo>` |
| Load sessions | `loadSessions()` | `listSessions(appConfigDir:)` | `listSessions(appConfigDir:)` | `list_sessions` -> read session dirs -> `Vec<SessionSummary>` |
| Load session log | `loadSessionLog(sessionId:)` | `loadSessionLog(sessionId:appConfigDir:)` | `loadSessionLog(sessionId:appConfigDir:)` | `load_session_log` -> `Vec<LogEvent>` |
| Run task | Submit task | `runAgentTask(payload:)` | `runAgentTask(payload:)` | `run_agent_task` -> `agent_request_impl` with **stub** executor -> `AgentResponse` (no real tools/LLM) |

### 3.3 What is **not** in the flow (yet)

- **Settings:** No `getConfig` / `saveConfig` in FFI; no struct for full app config in Swift.
- **Local LLM:** No FFI for `create_engine_for_profile` or Phi-3; Tauri doesn’t use local engine in `cmd_llm_invoke` either.

---

## 4. Connecting SwiftUI SettingsView to the Existing Rust Backend

### 4.1 Option A: Expose settings via UniFFI (recommended)

To use the **same** logic as Tauri (same file format, same schema), you need the **settings load/save** and **config type** in the **FFI** crate. The Tauri `AppSettings` and `settings::load` / `settings::save` live in the **Tauri app** crate, not in `oxcer-core` or `oxcer_ffi`. So you have two structural options:

1. **Move** (or re-export) settings types and load/save into a **shared** crate (e.g. `oxcer-core` or a new `oxcer_config`) that both Tauri and `oxcer_ffi` depend on, then in **oxcer_ffi** add UniFFI exports that call that logic; or  
2. **Duplicate** the config schema and load/save logic inside **oxcer_ffi** (e.g. copy the relevant structs and `config.json` read/write) and export that via UniFFI.

Either way, you need **in the FFI**:

- A **struct** equivalent to `AppSettings` (UniFFI record) with:
  - workspace list (id, name, path),
  - default_model_id,
  - advanced (e.g. allow_destructive_fs_without_hitl),
  - observability (e.g. max_session_cost_usd),
  - llm (setup_complete, profile).
- **Functions:**
  - **Get:** `get_config(app_config_dir: String) -> Result<AppSettings, OxcerError>`  
    - Implementation: same as `settings::load` (read `config.json`, parse into the FFI struct; fallback to default or legacy migration if you mirror that).
  - **Save:** `save_config(app_config_dir: String, settings: AppSettings) -> Result<(), OxcerError>`  
    - Implementation: same as `settings::save` (serialize to `config.json`).

Then in Swift:

- Call `getConfig(appConfigDir:)` when opening Settings and to refresh.
- Call `saveConfig(appConfigDir:settings:)` when the user saves.
- Use the same `appConfigDir` you already use (`~/Library/Application Support/Oxcer`), so the same `config.json` is used as Tauri and as `list_workspaces`.

### 4.2 Option B: Swift reads/writes config.json directly

- **Get:** Swift reads `appConfigDir/config.json` with `FileManager` and decodes JSON into a local Swift struct that mirrors `AppSettings`.  
- **Save:** Encode that struct to JSON and write to `config.json`.  
- **Pros:** No Rust/FFI changes.  
- **Cons:** Duplicated schema and migration logic; risk of drift from Tauri/Rust; no single source of truth in Rust.

### 4.3 Recommendation

- **Implement Option A:** Add `get_config` and `save_config` (and an `AppSettings`-like UniFFI record) in **oxcer_ffi**, implemented either by sharing the Tauri settings logic (via a shared crate) or by replicating the same `config.json` contract in the FFI crate. Then connect SwiftUI SettingsView to these two FFI functions and the same `appConfigDir` you already use.  
- For **local Phi-3:** The backend type and bootstrap exist in `oxcer-core`, but inference is stub and Tauri doesn’t use it in the agent. Wiring local model into the agent would require either (1) extending the Tauri runner to call `create_engine_for_profile` and use that engine for `LlmGenerate` when profile is local, or (2) exposing a similar path via FFI for the Swift app once a real runtime (e.g. llama.cpp/GGUF) is integrated.

---

## 5. Quick reference

| Need | In Rust | Exposed to Swift (UniFFI)? | Action for SettingsView |
|------|--------|----------------------------|--------------------------|
| AppSettings / config schema | Tauri `settings.rs` (`AppSettings`) | No | Expose equivalent struct + get/save in oxcer_ffi |
| Load config | `settings::load(app_config_dir)` | No | Add `get_config(app_config_dir)` in oxcer_ffi |
| Save config | `settings::save(app_config_dir, settings)` | No | Add `save_config(app_config_dir, settings)` in oxcer_ffi |
| List workspaces | `list_workspaces` (reads config.json) | Yes | Already use for workspace list |
| Phi-3 load / inference | `oxcer-core` (bootstrap + LocalPhi3Engine, stub runtime) | No | N/A for settings; for LLM, wire later when runtime exists |

**Rust functions you should have for SettingsView (to be added in oxcer_ffi):**

- `get_config(app_config_dir: String) -> Result<AppSettingsRecord, OxcerError>`
- `save_config(app_config_dir: String, settings: AppSettingsRecord) -> Result<(), OxcerError>`

with `AppSettingsRecord` (or equivalent name) being a UniFFI record that mirrors `AppSettings` (workspaces, default_model_id, advanced, observability, llm).
