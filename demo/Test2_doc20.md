# Roadmap and Future Work

## v0.1.0 Scope

The v0.1.0 release locks in three fully deterministic, automatic demo workflows: single-file summary (Workflow 1), multi-file overview (Workflow 2), and folder-to-folder move with folder creation (Workflow 3). All three work end-to-end with no follow-up questions, no hallucinated content, and per-step user approval for every filesystem operation. The local Llama 3 8B model handles all LLM steps; no internet connection is required.

## Batch Approval

Currently every tool call in a multi-step workflow requires a separate approval. For a 20-file move this means 20 approval taps. A planned improvement is a "batch approve" option that lets the user approve all remaining steps of the same kind in a single tap — for example, "Approve all FsMove steps for this session." Individual-step approval remains the default; batch approval is opt-in per session.

## Smarter File Filtering

The current `file_filter` mechanism matches on a simple substring of the filename. A more powerful approach would support glob patterns (`Test2_doc*.md`) or file-type-only filtering (e.g. "all Markdown files"). The `is_readable_file_type` function already implements extension-based filtering; extending it to accept user-specified patterns is a natural next step.

## Undo and Session History

Oxcer currently has no undo for destructive operations. A planned feature is a lightweight undo stack that records moved and deleted file paths during a session, allowing the user to issue a "reverse last operation" command. The telemetry log already captures all tool traces; building undo on top of those traces is straightforward.

## Plugin System

A plugin architecture is in early development. Plugins are YAML files that define a name, a description, a trigger phrase, and a shell command template. When the router matches a plugin's trigger, the orchestrator builds a `ShellRun` plan using the command template. Dangerous plugins (those whose commands match a blocklist) require the same approval UI as any other shell operation. The plugin system lets power users extend Oxcer's capabilities without touching the Rust or Swift source.

## Multi-Turn Conversations

The current orchestrator model is single-turn: one user message produces one agent run, which either completes successfully or returns an error. A future version will support multi-turn conversations where the agent can ask clarifying questions, receive follow-up instructions, and maintain context across multiple messages within a session. This requires changes to `SessionState` to track the conversation history alongside the tool execution history.
