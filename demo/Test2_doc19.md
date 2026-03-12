# The UniFFI Boundary

## Why UniFFI

The Rust orchestrator and the Swift UI need to talk to each other across a C ABI boundary. UniFFI automates the generation of Swift wrappers and C headers from Rust type annotations, eliminating the need to write FFI glue code by hand. Oxcer uses UniFFI 0.28 with attribute-based macros: `#[uniffi::export]` on functions, `#[derive(uniffi::Record)]` on structs, and `#[derive(uniffi::Enum)]` on enums.

## The Opaque Intent Design

The most important FFI type is `FfiToolIntent { kind: String, intentJson: String }`. The `kind` is a plain string (`"fs_list_dir"`, `"fs_read_file"`, etc.) that Swift uses to route to the correct handler. The `intentJson` is an opaque JSON blob containing the full intent parameters. Swift deserialises `intentJson` separately using `JSONDecoder` with `.convertFromSnakeCase`. This design means that adding a new `ToolCallIntent` variant to Rust — like `FsCreateDir` — requires no FFI regeneration: the new kind simply appears as a new string value in the existing `FfiToolIntent.kind` field.

## The FfiStepResult Type

`FfiStepResult { ok: Bool, payloadJson: String?, error: String? }` is the return type that Swift sends back to Rust after executing a tool. If `ok` is `true`, `payloadJson` contains the JSON-encoded result (e.g. `{ "text": "..." }` for a file read). If `ok` is `false`, `error` contains a human-readable description of what went wrong. The Rust orchestrator converts this into a `StepResult::Ok` or `StepResult::Err` for processing in `next_action`.

## Regeneration Discipline

After any change to a `#[uniffi::export]` function signature, both `oxcer_ffi.swift` and `oxcer_ffiFFI.h` must be regenerated with `scripts/regen-ffi.sh` (release build only — debug and release dylibs have different checksums). Both files must be committed together with the Rust source change. The CI workflow includes a `uniffi-binding-freshness` job that diffs the committed files against a fresh generation and fails if they diverge.

## Checksum Validation

UniFFI embeds checksum values in the generated Swift bindings that are validated at runtime against the loaded dylib. A mismatch (e.g. Swift bindings generated from a debug dylib but Xcode linking the release dylib) produces an `apiChecksumMismatch` crash at startup. The diagnostic is to run the checksum comparison script, which reads each `uniffi_oxcer_ffi_checksum_*` symbol from the dylib and compares it to the expected value in the Swift file.
