# FsReadFile and the Content Pipeline

## The Read-Then-Summarise Pattern

The simplest and most common Oxcer workflow is: read a file, then ask the local model to summarise it. `FsReadFile` returns the full text content of a file as a UTF-8 string. That string is stored in `session.accumulated_response` and later substituted into the `LlmGenerate` prompt via the `{{FS_RESULT}}` placeholder. This pattern ensures the model is given real file content, not asked to recall or invent it.

## Multi-File Accumulation

When multiple `FsReadFile` steps appear in a plan (the Workflow 2 case), each result is also appended to `session.content_accumulator` — a `Vec<String>` that grows with each successful read. After all reads are complete, the `LlmGenerate` step receives all accumulated content joined with `\n\n---\n\n` separators via the `{{FILE_CONTENTS}}` placeholder. The separator is intentionally plain and readable so the model can identify file boundaries.

## UTF-8 Assumption

`FsReadFile` reads files as UTF-8. Binary files (images, PDFs, compiled binaries) will either fail to read or produce garbled text. The `is_readable_file_type` filter in `do_expand_plan` prevents binary formats from being included in multi-file read plans. If the user explicitly names a binary file, the executor will attempt the read and the error will surface in the chat rather than producing nonsense output.

## File Size and Context Limits

The local Llama 3 8B model has a context window of 8 192 tokens. The orchestrator does not currently truncate file content before passing it to the model. For v0.1.0 demo workflows, files are assumed to be short enough to fit comfortably. A `safe_max_tokens` calculation in the runtime clamps generation length to avoid KV-cache overflow, but very large files could still exhaust the prompt budget.

## Confirmed Root After Read

A successful `FsReadFile` also sets `session.confirmed_root` if it is not already set, using the `workspace_root` field from the intent. This means that even in a direct-read workflow (no prior `FsListDir`), the confirmed root is captured and available for any subsequent dynamic steps.
