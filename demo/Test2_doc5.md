# Filesystem Tools Overview

## Design Principles

All six filesystem tools in Oxcer follow the same interface contract: they receive a JSON-encoded intent (workspace root + relative path + any extra fields), perform a single, scoped operation, and return a JSON-encoded payload. They never recurse, never follow symlinks across workspace boundaries, and always operate on absolute paths constructed by joining `workspaceRoot` and `relPath` inside the Swift executor.

## Path Construction Safety

The Swift executor uses `URL(fileURLWithPath: workspaceRoot).appendingPathComponent(relPath).standardized` to construct every target path. The `.standardized` call resolves any `..` components, preventing a crafted `relPath` like `../../etc/passwd` from escaping the workspace root. This is a defence-in-depth measure alongside the approval gate.

## Atomic Writes

`FsWriteFile` receives file content as a base64-encoded string and writes it using `Data.write(to:options:.atomic)`. The atomic write option causes Foundation to first write to a temporary file in the same directory, then rename it into place. This means a write that is interrupted mid-way (power loss, process kill) leaves either the old file or the new file intact — never a partial or zero-byte file.

## Idempotent Directory Creation

`FsCreateDir` calls `FileManager.createDirectory(withIntermediateDirectories: true)`. If the directory already exists, this call succeeds silently rather than throwing an error. This makes the create-and-move workflow idempotent: running it twice produces the same result as running it once, which is important when the user retries a workflow after a partial failure.

## Return Payloads

Each tool returns a structured JSON payload. `FsListDir` returns `{ entries, sortedByModified, text }` where `sortedByModified` is sorted newest-first by modification date. `FsReadFile` returns `{ text }`. `FsWriteFile`, `FsDelete`, `FsCreateDir`, and `FsMove` all return `{ ok: true }`. `ShellRun` returns `{ stdout, stderr, exitCode }`. These shapes are stable contracts between Swift and Rust.
