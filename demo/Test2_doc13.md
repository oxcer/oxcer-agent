# The confirmed_root Invariant

## Why confirmed_root Exists

When the orchestrator plans a workflow, it resolves directory paths from the task string using keyword matching (e.g. "Downloads" maps to `~/Downloads`). This resolution happens at planning time, before any filesystem operation has run. There is a theoretical risk that the resolved path differs from the path actually used by the first tool call. `confirmed_root` is the guarantee that all subsequent operations use the path that the filesystem actually accepted, not the path that was originally guessed.

## How It Is Set

The first time a `StepResult::Ok` arrives from a step whose plan entry is `FsListDir` or `FsReadFile`, the orchestrator reads the `workspace_root` field from that intent (the intent that was actually executed) and stores it in `session.confirmed_root`. Because the Swift executor always constructs the absolute path from the intent's `workspace_root`, the confirmed root is the real on-disk path.

## Usage in do_expand_plan

When `do_expand_plan` runs to insert `FsReadFile` or `FsMove` steps, it reads `session.confirmed_root` to populate the `workspace_root` of every new intent. This means that even if the original planning-time path was slightly different (for example, if the home directory had a symlink component that got resolved), all dynamically inserted steps use the same path that the first tool call actually used.

## The Workspace ID Companion

Alongside `confirmed_root`, the `workspace_id` for dynamically inserted steps is recovered from the `FsListDir` intent that just completed. The orchestrator looks up `session.plan[step_index - 1]` and extracts the `workspace_id` from the `FsListDir` variant. This keeps the workspace ID consistent across all steps in the same workflow.

## Edge Cases

If `confirmed_root` is `None` when `do_expand_plan` runs — which can only happen if the `FsListDir` payload was missing the `workspace_root` field — the orchestrator defaults to an empty string, which will cause all subsequent tool calls to fail with a path error. The error will surface in the chat rather than producing silent incorrect behaviour.
