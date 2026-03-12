# The Rust Orchestrator Core

## Role in the System

The orchestrator is the brain of Oxcer. It lives in `oxcer-core/src/orchestrator.rs` and is responsible for translating a user's natural-language request into a concrete, ordered sequence of tool calls, then advancing that sequence one step at a time as results come back from the Swift executor.

## Plan-First Execution

When a task arrives, the orchestrator runs a semantic router to classify the intent, then builds a complete plan upfront — a `Vec<ToolCallIntent>` stored in `SessionState`. The plan is never rebuilt mid-run; instead, it is expanded in place when dynamic information (such as a directory listing) arrives and the number of required steps becomes known.

## The Step Loop

The Swift shell calls `ffi_agent_step` in a loop, passing the current session blob and the result of the last tool execution. The orchestrator applies the result to its state, advances the `step_index`, and returns the next intent. When the plan is exhausted, it returns a `Complete` action with the final answer string. The step loop has a configurable maximum (default 20) to prevent runaway execution.

## SessionState Serialisation

`SessionState` is serialised to JSON after every step and passed back to Swift as an opaque string. Swift stores it and returns it unchanged on the next call. This design means the Rust orchestrator is entirely stateless from Swift's point of view — there are no shared globals, no background threads, and no Rust-side locks that could deadlock with Swift's cooperative thread pool.

## Error Handling

If a tool returns an error, the orchestrator immediately transitions to `TaskState::Complete` and returns the error message as the final answer. It never silently skips a failed step or moves on to the next tool call as if the error did not happen. This is especially important for filesystem operations: a missing file should produce "Error: No such file" in the chat, not a fabricated summary of imaginary contents.
