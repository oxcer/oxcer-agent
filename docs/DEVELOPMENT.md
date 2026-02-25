# Development Guide

> **v0.1.0 — macOS only.** This guide covers the macOS development workflow. Development and testing are currently performed on Apple Silicon machines (M1 and later). Intel macOS builds are expected to work but are not part of the regular test cycle. Windows and Linux support are planned but not yet available. If you are interested in contributing toward cross-platform support, see [CONTRIBUTING.md](../CONTRIBUTING.md).

This guide covers building, testing, and contributing to Oxcer.

---

## Prerequisites

| Tool | Version | Install |
|---|---|---|
| macOS | 14+ | — |
| Xcode | 15+ | App Store or developer.apple.com |
| Rust (stable) | see `rust-toolchain.toml` | `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh` |
| CMake | 3.15+ | `brew install cmake` |
| SwiftFormat (optional, for linting) | latest | `brew install swiftformat` |

> **CMake is required.** The `llama-cpp-sys` build dependency compiles llama.cpp via CMake. Use the Homebrew version (`/opt/homebrew/bin/cmake`); Anaconda's cmake is not on Xcode's PATH.

---

## Building

### 1. Build the Rust core

From the repository root:

```bash
cargo build --release -p oxcer_ffi
```

Output: `target/release/liboxcer_ffi.dylib`

The first build compiles llama.cpp and can take 5–10 minutes. Subsequent incremental builds are fast.

### 2. Build and run the macOS app

```bash
open apps/OxcerLauncher/OxcerLauncher.xcodeproj
```

In Xcode:
- Scheme: **OxcerLauncher**
- Destination: **My Mac**
- Press **⌘R** to build and run.

Xcode automatically runs `cargo build --release -p oxcer_ffi` as a build phase before linking. You do not need to run it manually unless you want faster iteration on Rust-only changes.

### Clean builds

After significant Rust changes or after switching branches, do a clean Xcode build:

```
Product → Clean Build Folder   (Shift + ⌘ + K)
```

Then rebuild normally.

---

## Testing

### Rust tests

```bash
# Core library: unit + integration tests
cargo test -p oxcer-core

# FFI contract tests
cargo test -p oxcer_ffi

# Full workspace sanity check
cargo check --workspace
```

Run `cargo test -p oxcer-core` first; it is the fastest feedback loop for core logic changes.

### Structured log output (Rust)

Set `OXCER_LOG` to control verbosity. The subscriber emits JSON lines to stdout:

```bash
OXCER_LOG=debug cargo test -p oxcer_ffi 2>&1 | jq '.event, .session_id'
OXCER_LOG=info  ./target/release/oxcer-launcher 2>&1 | jq .
```

### macOS app build check (no Xcode GUI)

```bash
cargo build --release -p oxcer_ffi    # must succeed first

xcodebuild \
  -project apps/OxcerLauncher/OxcerLauncher.xcodeproj \
  -scheme OxcerLauncher \
  -destination 'platform=macOS' \
  build
```

### Swift logging

Run the app from Xcode and filter Console.app by subsystem `com.oxcer.launcher`, or use:

```bash
log stream --predicate 'subsystem == "com.oxcer.launcher"' --level debug
```

---

## FFI Workflow

Whenever you add, remove, or change a `#[uniffi::export]` item, a `#[uniffi::Record]`, or a `#[uniffi::Error]` in `oxcer_ffi/src/lib.rs`, you must regenerate the Swift bindings.

```bash
./scripts/regen-ffi.sh
```

This script:
1. Builds the release dylib (`cargo build --release -p oxcer_ffi`).
2. Runs `uniffi-bindgen generate --library … --language swift`.
3. Diffs and copies `oxcer_ffi.swift` and `oxcer_ffiFFI.h` into the Xcode project.
4. Verifies that runtime API checksums in the dylib match the generated Swift bindings.

Commit the Rust change and both generated files together:

```bash
git add oxcer_ffi/src/lib.rs \
        apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift \
        apps/OxcerLauncher/OxcerLauncher/oxcer_ffiFFI.h
git commit -m 'ffi: <describe the contract change>'
```

**Never** run bindgen against a debug dylib; Xcode links the release dylib and a checksum mismatch produces `apiChecksumMismatch` at runtime.

CI enforces binding freshness: the `uniffi-binding-freshness` job fails if the committed Swift files do not match what the current Rust source would produce.

### Diagnosing apiChecksumMismatch at runtime

```python
import ctypes, re
lib = ctypes.CDLL('target/release/liboxcer_ffi.dylib')
swift = open('apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift').read()
for m in re.finditer(r'uniffi_oxcer_ffi_checksum_(\w+)\(\) != (\d+)', swift):
    sym = f'uniffi_oxcer_ffi_checksum_{m.group(1)}'
    fn = getattr(lib, sym); fn.restype = ctypes.c_uint16
    got = fn(); expected = int(m.group(2))
    print('OK' if got == expected else 'MISMATCH', m.group(1), got, 'vs', expected)
```

---

## Configuration

Runtime config is stored in `~/Library/Application Support/Oxcer/config.json`:

```json
{
  "workspaces": [
    { "id": "uuid", "name": "My Project", "root_path": "/path/to/dir" }
  ],
  "model": { "default_id": "local-llama3" },
  "security": {
    "destructive_fs": { "enabled": false }
  }
}
```

A missing file or missing `workspaces` key results in an empty workspace list (not an error). A malformed JSON file is reported as an error in the UI.

---

## Project Conventions

- **Core logic belongs in `oxcer-core`**, not in `oxcer_ffi` or the Swift app. Test it there independently.
- **FFI functions are thin wrappers**: deserialise input, call into `oxcer-core`, serialise output. No business logic in `oxcer_ffi`.
- **Tool errors never throw** out of `SwiftAgentExecutor.execute(intent:)`. They become `FfiStepResult(ok: false, ...)` so the Rust orchestrator can reason about them.
- **Never bypass `scrub_for_llm_call`**. All content destined for the LLM must pass through the data sensitivity pipeline.
- **Use `tracing::` macros** (not `println!`) in Rust. Use `os.Logger` (not `print()`) in Swift.

---

## Troubleshooting

| Symptom | Action |
|---|---|
| `cargo build` fails with "cmake not found" | `brew install cmake` |
| Xcode "Cannot find symbol uniffi_oxcer_ffi_fn_..." | Run `./scripts/regen-ffi.sh`, then Clean Build Folder |
| `apiChecksumMismatch` crash at launch | Run `./scripts/regen-ffi.sh`; ensure you built the release dylib |
| App crashes on launch | Run from Xcode; check Console for Rust panic. Run `RUST_BACKTRACE=1` |
| "Insufficient Space" in LLM generation | Prompt + system context exceeds context window; check `safe_max_tokens` logic |
| No workspaces shown | Verify `~/Library/Application Support/Oxcer/config.json` exists with a valid `workspaces` array |
