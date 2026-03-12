# The MOST_RECENT_FILE Placeholder

## The Problem

When a user says "summarise the file I just saved in Downloads," the filename is not known at planning time. Building a plan that reads a specific file is impossible without first listing the directory. The `{{MOST_RECENT_FILE}}` placeholder is the solution: the orchestrator plans `[FsListDir, FsReadFile({{MOST_RECENT_FILE}}), LlmGenerate]` and resolves the placeholder after the listing returns.

## The sortedByModified Field

The Swift executor returns an `EntriesPayload` that includes a `sortedByModified` array alongside the alphabetical `entries` array. Filenames in `sortedByModified` are ordered newest-first by the file's modification date (`NSFileModificationDate`). The orchestrator captures this array in `session.last_dir_listing_sorted` as soon as the `FsListDir` result arrives.

## Placeholder Resolution

In the "More steps?" section of `next_action`, before emitting a `FsReadFile` intent, the orchestrator checks whether its `rel_path` contains `{{MOST_RECENT_FILE}}`. If it does, it takes `session.last_dir_listing_sorted.first()` — the most recently modified file — and substitutes it into the `rel_path` in-place. If the listing was empty, the placeholder is replaced with `"(no files found)"`, which will cause the read to fail with a clear error.

## Why Modification Date Rather Than Creation Date

macOS does not reliably provide file creation dates for files copied from external sources or downloaded from the internet. Modification date is more consistently meaningful: a file that was "just saved" will have a modification date that is very recent. For the v0.1.0 demo workflow, the test files are freshly created, so modification date correctly identifies the most recently touched file.

## The Three-Step Plan

The full plan for this workflow is: `FsListDir` (populate `last_dir_listing_sorted`), `FsReadFile({{MOST_RECENT_FILE}})` (read the newest file, resolved at emit time), `LlmGenerate({{FS_RESULT}})` (summarise the file content). This three-step structure was the first multi-step dynamic workflow implemented in Oxcer and established the placeholder substitution pattern used by all subsequent workflows.
