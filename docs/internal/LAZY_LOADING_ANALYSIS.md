# Backend 17GB Memory Spike: Analysis & Lazy Loading Proposal

## Executive Summary

The 17GB memory spike occurs when `ensure_local_model` runs (first FFI call during app startup). The root cause is **eager model/engine initialization**: `ensure_local_model_impl` both ensures files exist and immediately loads `LocalPhi3Engine` into `GLOBAL_ENGINE`. This analysis identifies the culprits and proposes a **Lazy Loading** pattern to defer engine creation until first inference.

---

## 1. Heavy Allocations

### 1.1 `Vec::with_capacity` / Large Buffers

| Location | Allocation | When |
|----------|------------|------|
| `oxcer-core/fs.rs:449` | `Vec::with_capacity(size)` + `read_to_end` | Not in FFI path |
| `oxcer-core/telemetry.rs:407` | `VecDeque::with_capacity(2001)` | Bounded, small |
| `oxcer-core/shell.rs:664` | `Vec::with_capacity(8192)` | Bounded |

**Conclusion:** No unbounded `Vec::with_capacity` in the FFI/ensure_local_model path.

### 1.2 File Loading in `ensure_local_model_impl`

```rust
// oxcer_ffi/src/lib.rs:182-291
async fn ensure_local_model_impl(...) {
    // 1. Path setup - no heavy I/O
    let model_gguf = model_root.join("phi-3-mini-4k-instruct-q4.gguf");
    let file_exists = model_gguf.is_file() && std::fs::metadata(&model_gguf)...

    // 2. DOWNLOAD (if missing): streams chunk-by-chunk to file
    download_file(PHI3_GGUF_URL, &model_gguf, adapter).await  // ~2.4GB file
    // downloader.rs: streams via resp.bytes_stream(), writes per-chunk - no full buffering

    // 3. FILE COPY: std::fs::copy when symlink fails (e.g. non-Unix)
    std::fs::copy(&model_gguf, &model_gguf_for_loader)
    // std::fs::copy: can buffer internally; 2.4GB copy = temporary memory pressure
```

**Risk:** `std::fs::copy` for a 2.4GB file may use significant temporary buffers (OS-dependent). On some systems, copy is implemented with a large buffer.

### 1.3 `LocalPhi3Engine::new` — The Heavy Load

```rust
// oxcer-core/src/llm/local_phi3/mod.rs:30-48
pub fn new(model_root: &Path) -> Result<Self, LlmError> {
    let paths = loader::resolve_model_paths(model_root)?;  // path checks only
    let tokenizer = loader::load_tokenizer(&paths.tokenizer_json)?;  // Tokenizer::from_file
    let runtime: Box<dyn PhiRuntime> = Box::new(runtime::StubPhiRuntime);
    Ok(Self { tokenizer, runtime })
}
```

- **Tokenizer:** `Tokenizer::from_file(tokenizer_path)` — loads `tokenizer.json` (~1–2 MB). The HuggingFace `tokenizers` crate can expand vocab in memory; Phi-3 vocab is ~32k tokens. Typical usage: tens of MB.
- **Runtime:** `StubPhiRuntime` — zero allocation, no GGUF load.
- **Planned (TODO in code):** When llama.cpp/ONNX is wired, `LocalPhi3Engine::new` will load the full GGUF (~2.3 GB) into RAM. The current stub avoids this, but the **design** assumes model load happens here.

**Conclusion:** Today’s stub + tokenizer is moderate (tens of MB). When GGUF is wired, this becomes ~2.3 GB+ in one call. The 17GB spike may stem from:
- (a) Tokenizers or a dependency allocating heavily on first use
- (b) `std::fs::copy` for the 2.4GB model file
- (c) Dylib + transitive deps (tokio, reqwest, tokenizers) reserving virtual memory on first FFI touch

---

## 2. Model Loading — Eager at Launch

**Current flow:**
```
App launch -> RootView .task -> checkAndPrepareModel() -> ensure_local_model()
    -> ensure_local_model_impl()
        -> if files exist: LocalPhi3Engine::new()  [IMMEDIATE]
        -> if not: download -> LocalPhi3Engine::new()  [IMMEDIATE after download]
```

The model/engine is loaded **as soon as files are present**, before any user inference. There is no lazy path.

---

## 3. Loops in Setup Phase

| Location | Loop | Risk |
|----------|------|------|
| `downloader.rs:61` | `while let Some(chunk) = stream.next().await` | Streaming download; each chunk written to file. No accumulation. |
| `ensure_local_model_impl` | None | No loops. |
| `LocalPhi3Engine::new` | None | No loops. |

**Conclusion:** No accumulation loops in the setup path.

---

## 4. Why 17GB?

Possible contributors:

1. **Tokenizers crate:** Known to have memory patterns with large inputs. First load of a complex tokenizer may trigger non-obvious allocations (regex engines, Unicode tables, vocab expansion).
2. **`std::fs::copy`:** Copying a 2.4GB file can cause large temporary buffers.
3. **Rust/tokio/reqwest initialisation:** First use of the FFI loads the dylib and initializes tokio, reqwest, and other deps. Virtual address space and allocator behaviour can appear as high memory in tools.
4. **macOS memory reporting:** Virtual vs resident; shared libraries; compression.

---

## 5. Lazy Loading Pattern

### 5.1 Goal

- **Ensure files exist** (including download) at startup.
- **Defer engine creation** until the first inference request.

### 5.2 Design

| Phase | Current | Proposed |
|-------|---------|----------|
| `ensure_local_model` | Ensure files + **load engine** | Ensure files **only** |
| `generate_text` / first LLM use | Use `GLOBAL_ENGINE` | Load engine **on first use** if not yet loaded |

### 5.3 Implementation Sketch

**1. Split `ensure_local_model_impl` into two phases**

```rust
// Phase A: Ensure files exist (download if needed). NO engine creation.
async fn ensure_model_files_present(
    app_config_dir: &Path,
    callback: Arc<dyn DownloadCallback>,
) -> Result<PathBuf, OxcerError> {
    if GLOBAL_ENGINE.get().is_some() {
        return Ok(/* existing model_root */);  // Already loaded, return path
    }
    let config_dir = app_config_dir.to_path_buf();
    let models_dir = config_dir.join("models");
    let model_root = models_dir.join("phi3");
    // ... download logic (unchanged), but STOP before LocalPhi3Engine::new
    Ok(model_root)
}

// Phase B: Load engine (called lazily from generate_text / run_agent_task)
fn ensure_engine_loaded(model_root: &Path) -> Result<(), OxcerError> {
    if GLOBAL_ENGINE.get().is_some() {
        return Ok(());
    }
    let _guard = INIT_LOCK.lock()?;
    if GLOBAL_ENGINE.get().is_some() {
        return Ok(());
    }
    let engine = LocalPhi3Engine::new(model_root)?;
    let _ = GLOBAL_ENGINE.get_or_init(|| Arc::new(Box::new(engine)));
    Ok(())
}
```

**2. `ensure_local_model` (FFI) — files only**

```rust
pub async fn ensure_local_model(...) -> Result<(), OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    ensure_model_files_present(&dir, Arc::from(callback)).await?;
    // Do NOT call ensure_engine_loaded here
    Ok(())
}
```

**3. `generate_text` — lazy load**

```rust
pub async fn generate_text(prompt: String) -> Result<String, OxcerError> {
    let engine_ref = get_global_engine();
    let engine = if let Some(e) = engine_ref {
        e
    } else {
        // Lazy load: ensure files first (must have been called at startup)
        let model_root = get_model_root_from_config()?;  // Store path when ensure_model_files runs
        ensure_engine_loaded(&model_root)?;
        get_global_engine().ok_or(...)?
    };
    // ... rest unchanged
}
```

**4. Store `model_root` for lazy load**

When `ensure_model_files_present` completes, store the resolved `model_root` in a `OnceLock<PathBuf>` (or similar) so `generate_text` can call `ensure_engine_loaded(&model_root)` when the engine is not yet loaded.

### 5.4 Benefits

| Benefit | Description |
|---------|-------------|
| **Faster startup** | No engine allocation at launch |
| **Lower memory at launch** | Engine (tokenizer + future GGUF) only on first inference |
| **Same semantics** | `ensure_local_model` still required before inference; it just no longer loads the engine |
| **Clear separation** | Files vs engine lifecycle |

### 5.5 Migration Notes

- `ensure_local_model` remains the “setup” call; it still ensures files (and download).
- First call to `generate_text` or any LLM path will trigger engine load.
- Error handling: if `ensure_local_model` was never called, lazy load will fail with a clear “model files not ready” error.

---

## 6. Recommended Next Steps

1. **Instrument** `LocalPhi3Engine::new` and `std::fs::copy` with memory measurements (e.g. `malloc_size` on macOS) to pinpoint the 17GB source.
2. **Implement lazy loading** as above to eliminate eager engine creation at startup.
3. **Optionally** replace `std::fs::copy` with a streaming copy or platform-specific copy (e.g. `copyfile` on macOS) to avoid large buffers when creating `model.gguf`.
