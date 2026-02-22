# Oxcer Project Memory

## Architecture
- Swift/macOS desktop app (OxcerLauncher) + Rust backend (oxcer_ffi, oxcer-core)
- FFI bridge via **UniFFI 0.28** attribute-based macros (no .udl file)
- `uniffi::setup_scaffolding!("oxcer_ffi")` in `oxcer_ffi/src/lib.rs`

## FFI File Locations
- **Rust**: `oxcer_ffi/src/lib.rs` — all `#[uniffi::export]` functions and `#[uniffi::Record]` types
- **Swift bindings (live)**: `apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift` — single committed source
- **Swift bindings (stale!)**: `generated_swift/oxcer_ffi.swift` — old, diverges; recommend deleting
- **Service layer**: `apps/OxcerLauncher/OxcerLauncher/OxcerBackend.swift`
- **Integration tests (Rust)**: `oxcer_ffi/tests/ffi_validation.rs`
- **Unit tests (Rust)**: `oxcer_ffi/src/lib.rs` `#[cfg(test)]` module
- **XCTests (Swift)**: `apps/OxcerLauncher/OxcerLauncherTests/OxcerFFITests.swift`

## Critical Bug History: 88 GB Virtual Memory
- Caused by stale Swift bindings after Rust `list_workspaces` return type changed
- `generated_swift/` had `throws -> [WorkspaceInfo]` (old); app-local had `-> Int32` (updated manually)
- UniFFI sequence decoder reads `len: Int32` from a `RustBuffer` that was never written
- Garbage `len` -> `seq.reserveCapacity(garbage)` -> ~88 GB VM reservation
- Root fix: CI checks binding freshness (`scripts/check-ffi-freshness.sh` + ci.yml job)

## FFI Migration Pattern (Primitive-First Reconstruction)
When changing a complex return type, advance through stages:
1. `-> Result<String, OxcerError>` — probe sentinel ("PROBE_OK:…")
2. `-> Result<MinimalStruct, OxcerError>` — single struct
3. `-> Result<Vec<Struct>, OxcerError>` — hardcoded small vec
4. `-> Result<Vec<Struct>, OxcerError>` — live implementation
Run `./scripts/regen-ffi.sh` between each stage.

## Current Migration State: list_workspaces
- **Stage 1** (current): `-> Result<String, OxcerError>` returning `"PROBE_OK:list_workspaces:stage1"`
- `list_workspaces_impl()` is implemented (serde DTOs: ConfigWorkspaceDto, ConfigFileDto) but not wired
- `OxcerBackend.swift` protocol returns `String`; MockOxcerBackend returns probe string
- Stage 4 code (commented): both `ffi_validation.rs` and `OxcerFFITests.swift`

## CI Safeguards
- `cargo test -p oxcer_ffi` — validates Rust-side FFI including `list_workspaces_impl` unit tests
- `uniffi-binding-freshness` job in ci.yml — builds dylib, runs bindgen, diffs against committed file
- `scripts/regen-ffi.sh` — developer regeneration helper
- `scripts/check-ffi-freshness.sh` — local equivalent of CI check

## Regeneration Workflow
Always run after any Rust FFI signature change:
```
./scripts/regen-ffi.sh
# then: ⌘B in Xcode, ⌘U to run OxcerFFITests
# then: git add oxcer_ffi/src/lib.rs apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift
```

## Key UniFFI ABI Facts
- `RustBuffer`: 24 bytes on 64-bit (capacity: i64, len: i64, data: *mut u8)
- Sequences: `Int32` length prefix + items (garbage len -> huge reserveCapacity)
- Checksum verified at app startup via `uniffiEnsureInitialized()`; mismatch -> `apiChecksumMismatch`
- `rustCallWithError` wraps `Result<T, E>`; plain `rustCall` wraps infallible returns
- Changing `-> i32` to `-> Result<Vec<T>>` changes C ABI from register return to RustBuffer return

## User Preferences
- Production-grade, safety-first approach
- Explicit over clever; favor verbosity in FFI/safety code
- All FFI changes must: (1) fix Rust, (2) regen bindings, (3) update service layer, (4) test
