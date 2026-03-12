# Session State and the Step Index

## What SessionState Holds

`SessionState` is the single source of truth for everything the orchestrator knows about an in-progress task. It records the original task description, the router's strategy decision, the full plan vector, the current `step_index`, an `accumulated_response` string (the last tool's text output), a `content_accumulator` for multi-file reads, tool traces for logging, and the pending expansion kind if dynamic plan growth is expected.

## The Step Index

`step_index` is a `usize` that points to the next plan entry to execute. When a tool result arrives, the orchestrator first records the result into session state, then increments `step_index`, then looks up `plan[step_index]` to find the next intent to emit. If `step_index` equals `plan.len()`, the task is complete.

## Immutability Through the FFI Boundary

Because `SessionState` is serialised to JSON and passed to Swift as an opaque blob, it is effectively immutable from Swift's perspective. Swift cannot accidentally mutate it; it can only store it and pass it back. This removes an entire class of concurrency bug: there is no shared mutable state crossing the FFI boundary. The only writer is the Rust orchestrator, which receives a fresh copy on each step.

## Tool Traces

Every completed step appends a `ToolTrace` to `session.tool_traces`. Each trace records the tool name, a sanitised summary of its input (workspace root and relative path, or command preview for shell tools), whether it succeeded, and a timestamp. Traces are surfaced in the debug panel and written to the session telemetry log on disk.

## State Machine Transitions

`SessionState` contains a `TaskState` enum: `Initial`, `Executing`, or `Complete`. The orchestrator starts in `Initial`, moves to `Executing` on the first step, and transitions to `Complete` when the plan is exhausted or a tool returns an error. The FFI layer checks `TaskState::Complete` to decide whether to return a `Complete` outcome to Swift rather than a `NeedTool` outcome.
