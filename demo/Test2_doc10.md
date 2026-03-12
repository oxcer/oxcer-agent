# FsCreateDir and Idempotent Folder Creation

## Why a Dedicated Create-Dir Tool

Earlier versions of the move workflow required the user to create the destination folder manually before running the agent. This created friction for non-developer users who might not know how to create a folder in the right location or with the right name. `FsCreateDir` was added specifically so the agent can create the destination folder as part of the workflow — the user only needs to name it.

## Implementation

The Swift executor handles `FsCreateDir` with a single call:

```swift
try FileManager.default.createDirectory(
    at: dir, withIntermediateDirectories: true
)
```

The `withIntermediateDirectories: true` parameter means the call succeeds whether the folder exists or not, and creates any missing parent directories in the path. This makes the entire workflow idempotent: if the agent is restarted after a partial run, `FsCreateDir` will not fail because the folder already exists.

## Position in the Plan

`FsCreateDir` is always the first step inserted by `do_expand_plan` during a move workflow. It comes immediately after the `FsListDir` step and before any `FsMove` steps. This ordering guarantees the destination folder exists before the first file is moved.

## No FFI Regeneration Required

Because the FFI boundary uses a generic `FfiToolIntent { kind: String, intentJson: String }` type, adding `FsCreateDir` on the Rust side did not require regenerating UniFFI bindings. The new variant simply produces `kind = "fs_create_dir"` and a JSON blob with `workspace_root` and `rel_path`, which the Swift executor dispatches to `handleFsCreateDir` via a string switch. This is the main advantage of the opaque-intent FFI design.

## Approval Prompt

The approval prompt for `FsCreateDir` reads "Allow Oxcer to create folder: ~/Desktop/Test_folder". This is surfaced to the user before the folder is created, giving them the chance to cancel if they want to choose a different location or name.
