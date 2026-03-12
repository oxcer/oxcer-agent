# FsListDir and Directory Listing

## When FsListDir Is Used

`FsListDir` is the first step in every workflow where the exact set of files is not known at planning time. This includes the multi-file summarise workflow (20 reports whose names are only discovered at runtime), the most-recent-file workflow (newest file in a directory), and the move workflow (files matching a prefix that need to be relocated). It is also used when the user asks a general question like "what's in my Downloads folder?"

## The EntriesPayload

The Swift executor returns an `EntriesPayload` with three fields. `entries` is an alphabetically sorted list of all filenames in the directory. `sortedByModified` is the same list sorted by file modification date, newest first — this is the field the orchestrator reads to resolve the `{{MOST_RECENT_FILE}}` placeholder. `text` is the alphabetical listing joined with newlines, used as the direct substitution target for `{{FS_RESULT}}` in a two-step listing plan.

## The confirmed_root Invariant

The first time a successful `FsListDir` or `FsReadFile` result arrives, the orchestrator records `workspace_root` into `session.confirmed_root`. All subsequent dynamic expansion uses `confirmed_root` rather than the originally planned root. This prevents a subtle class of bug where a path is resolved differently at planning time versus execution time.

## Filtering the Listing

When `do_expand_plan` runs after `FsListDir`, it filters the returned entries with two predicates: `is_readable_file_type` (accepting `.md`, `.txt`, `.csv`, `.json`, `.yaml`, `.log`, `.rst` and rejecting dotfiles and binary formats) and an optional `file_filter` string (a prefix like `"Test2_doc"` that limits the expansion to matching files). Files that pass both predicates are converted into `FsReadFile` intents and spliced into the plan.

## Large Directories

If a directory contains many files, `do_expand_plan` may insert a large number of `FsReadFile` steps. The step limit (default 20) provides a backstop, but for the v0.1.0 demo workflows the directories are assumed to be reasonably sized. Future versions may add a configurable cap on the number of files processed in a single multi-file run.
