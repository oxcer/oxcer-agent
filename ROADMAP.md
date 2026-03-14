# Roadmap

Oxcer is a local-first document intelligence tool for macOS. The goal: let non-developers summarize, organize, and query documents by describing what they want in plain English — entirely on-device, with no data leaving the machine.

This is a directional plan, not a commitment. Priorities may shift based on contributor interest and user feedback. See [docs/whitepaper.pdf](docs/whitepaper.pdf) for the full architecture and design rationale behind each milestone.

---

## v0.1 — Current (stable)

Everything listed here is shipped, tested end-to-end on Apple Silicon, and enabled in the production UI unless otherwise noted.

- **Single-file summarization.** Type `Summarize Test1_doc.md in Downloads` and Oxcer reads the file, runs local inference via Llama 3 8B Q4\_K\_M on Metal, and returns a summary. No internet required.
- **Supported file types.** `.md`, `.txt`, `.csv`, `.json`, `.yaml`, `.yml`, `.log`, `.rst` — any UTF-8 encoded plain-text file.
- **Human-in-the-loop approval.** Write operations (`fs_write_file`, `fs_delete`, `fs_rename`, `fs_move`, `fs_create_dir`, `shell_run`) require explicit user approval before execution. Read-only operations (`fs_list_dir`, `fs_read_file`) do not.
- **DLP scrubbing.** All prompt text passes through `prompt_sanitizer::sanitize_task_for_llm` before reaching the model. Credentials, API keys, JWTs, and PEM keys are redacted.
- **Cloud provider opt-in.** OpenAI, Anthropic, Gemini, and Grok are wired as optional backends via `CLOUD_ENGINE_SLOT`. Configured in Settings; local inference is the default.
- **Narration sanitizer.** LLM output that describes tool calls instead of summarizing content (`"I'll use fs_list_dir…"`) is detected and rejected before it reaches the UI.
- **FFI binding freshness enforcement.** CI fails on stale UniFFI bindings (diff between committed `.swift`/`.h` files and what `uniffi-bindgen` would produce from the current dylib). Pre-push hook: `scripts/check-ffi-freshness.sh`.
- **Implemented, awaiting v1.0 wiring:**
  - `memory.rs` — Markdown-backed persistent fact store with keyword search, compaction, and disk persistence. Fully implemented, not yet called from `ffi_agent_step`.
  - `db.rs` — SQLite episodic memory store (WAL mode, `Arc<Mutex<Connection>>`). Fully implemented, connected only to the experimental `orchestrate_query` FSM path.
  - `agent_session_log.rs` — Per-session structured log builder with DLP scrubbing on all stored text. Fully implemented, no callers in the production agent loop.

---

## v1.0 — Target

### Streaming output

Stream model output token-by-token to the UI. Long summaries appear incrementally. A stop button cancels mid-stream. Requires adding a streaming callback interface to the `generate_text` FFI export; the `LlamaCppPhiRuntime` generation loop already produces tokens one at a time.

### Multi-file summarization (Workflow 2)

Summarize a set of documents in one request, producing a single coherent overview.

The `ReadAndSummarize` plan expansion path already exists in `oxcer-core/src/orchestrator/planning.rs` (`do_expand_plan`, `ExpansionKind::ReadAndSummarize`). It is disabled in `start_session` with a `#[allow(dead_code)]` comment and `pending_expansion = None`. Enabling it requires end-to-end context-budget validation: the `content_accumulator` must not overflow the 8 192-token context window when accumulating content from multiple files.

### Workflow Memory wired (`memory.rs` → `ffi_agent_step`)

Wire the `Memory` fact store into the `ffi_agent_step` loop so the agent can record and recall observations across steps within a session, and across sessions over time. Facts are stored in a Markdown file at `~/.config/oxcer/memory.md` and injected into `LlmGenerate` prompts as a `[MEMORY_CONTEXT]` block.

### Document Relationship Graph

Build a lightweight graph of document co-occurrence using the `StateDb` episodic store (`db.rs`). When the agent reads multiple files in a session it records their relationship (co-summarized, co-moved, related by filename pattern). Enables queries like "what documents are related to the report I summarized last Tuesday?"

### Personalized Command Set

Detect recurring patterns in `agent_session_log.rs` records (same workflow triggered repeatedly with similar phrasing) and surface them as user-definable shortcuts. Users can name a shortcut (`"monthly"`) and invoke it instead of the full natural-language description.

### PDF support

Native PDF text extraction. The planner's `is_readable_file_type` and `FILE_EXTS` lists already include `.pdf`; the gap is a text-extraction step before content is passed to `FsReadFile`.

### App notarization

Developer ID signing and Gatekeeper notarization for distributable builds. Required before any binary distribution outside of build-from-source.

### Context window expansion

Investigate sliding-window chunking for files that exceed `FS_RESULT_MAX_CHARS = 4 000` characters. Current behaviour truncates with a notice. v1.0 target: chunk-and-summarize so the full file is covered even when it exceeds the single-pass limit.

---

## Beyond v1.0

These are on the roadmap with no committed timeline.

### Folder-level operations (Workflow 3)

Move a set of files from one directory to another by describing the operation in plain English (`"Move the 20 Test2_doc files from Downloads into a new folder called Test_folder on Desktop"`).

The `MoveToDir` plan expansion path exists in `planning.rs` (`ExpansionKind::MoveToDir`, `do_expand_plan`, `build_plan_list_then_move`). Disabled for the same reason as Workflow 2 — needs end-to-end validation, particularly the `FsCreateDir` + `FsMove×N` fan-out on real directory sizes.

### Cross-session document graph queries

Query the document relationship graph across sessions: "show me everything I've read about Q3 financials." Requires the episodic store from v1.0 to accumulate enough data and a query interface surfaced in the UI.

### Non-English directory name support

The current planner recognizes `Downloads`, `Desktop`, and `Documents` by exact English substring match. Non-English macOS locale directory names (e.g. `Bureau`, `Téléchargements`, `Schreibtisch`) are not recognized and fall through to the chat fallback. Fix: resolve well-known directories via `dirs_next` platform APIs rather than string matching.

### Windows and Linux native launchers

The Rust core (`oxcer-core`) is fully platform-agnostic. The `desktop-tauri` Tauri shell exists as a backend-only stub and is not the intended Windows distribution target. The target is a native WinUI 3 launcher on Windows. Linux: a CLI or minimal GTK/Qt wrapper for headless and server use cases.

### GGUF → ONNX Runtime migration

Replace `LlamaCppPhiRuntime` with an `OnnxPhiRuntime` implementing the same `PhiRuntime` trait inside `LocalPhi3Engine`. The `LlmEngine` trait interface is unchanged; everything above it (planner, tools, security, FFI) is unaffected.

ONNX Runtime execution providers:

| Provider | Hardware |
|---|---|
| CUDA EP | NVIDIA GPUs |
| ROCm EP | AMD GPUs |
| DirectML EP | Windows, D3D12 (NVIDIA + AMD + Intel) |
| QNN EP / Hexagon NPU | Qualcomm Snapdragon |
| OpenVINO EP | Intel CPU / GPU / VPU |
| CoreML EP | Apple Silicon (alternative to llama.cpp Metal path) |

**Goal:** a single Oxcer codebase running local inference on any consumer device — Apple Silicon, Snapdragon X Elite, NVIDIA, AMD — without cloud dependency.

---

## How to get involved

**Use it and report what breaks.** The most useful contributions right now are bug reports on Workflow 1 edge cases: unusual file encodings, long files, non-ASCII filenames, large Downloads/Documents folders.

**Good first issues** are labeled [`good first issue`](../../issues?q=label%3A%22good+first+issue%22) in the issue tracker.

**Areas where contributors can help most:**

- Streaming output: token callback interface in `generate_text` FFI + Swift `AsyncStream` wiring in `SwiftAgentExecutor`.
- Eval fixtures for Workflow 1: reproducible `(task_description, file, expected_plan)` triples for `cargo test`.
- PDF text extraction: integrate a pure-Rust PDF parser (e.g. `pdf-extract`) in `FsReadFile` dispatch.
- Windows WinUI 3 launcher stub.
- ONNX Runtime proof-of-concept for `OnnxPhiRuntime`.
- Non-English directory name resolution via `dirs_next`.

Before opening a pull request, read [CONTRIBUTING.md](CONTRIBUTING.md).

---

*Last updated: v0.1.0 release.*
