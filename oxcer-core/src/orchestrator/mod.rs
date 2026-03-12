//! Agent Orchestrator: cost-aware execution over Command Router.
//!
//! ## Core types and flow
//!
//! - **SessionState**: Per-session task machine (router_output, plan, step_index, tool_traces).
//! - **ToolCallIntent**: One tool call (FsListDir, FsDelete, LlmGenerate, etc.); runner executes via Command Router.
//! - **StepResult**: Outcome of one tool execution (Ok, Err, ApprovalPending).
//! - **Flow**: `start_session` -> build plan from router; `agent_step` / `next_action` advance state; executor runs tools and feeds results back.
//! - **agent_request**: Sync loop over agent_step + executor (blocks until complete). FFI uses stub executor; Tauri uses real commands.
//!
//! ## Agent as untrusted client
//!
//! - All tool calls go through the same Command Router with **caller = PolicyCaller::AgentOrchestrator**.
//! - The Security Policy Engine applies a more conservative rule set for the agent (path blocklist, command blacklist, write/exec require approval).
//! - **No batching:** we never batch tool calls into an opaque "macro". Each tool intent is emitted and executed separately; every call is evaluated by the policy engine and may trigger approval. High-risk sequences are never merged into a single request.
//!
//! ## Sensitive data protection and scrubbing pipeline
//!
//! - Every LLM-bound task string is sanitized via `prompt_sanitizer::sanitize_task_for_llm` when building an `LlmGenerate` intent.
//! - **Central scrubbing pipeline (every LLM call):** The runner must build a combined raw payload (task + file snippets + shell outputs + tool outputs + metadata), then call `prompt_sanitizer::scrub_for_llm_call(&raw_payload, &options)` (or `build_and_scrub_for_llm(&parts, &options)`). Use the returned scrubbed string for the request; never send the raw payload. If the pipeline returns `Err(ScrubbingError::TooMuchSensitiveData)` (≥50% redacted), the runner must **not** call the LLM and must return `StepResult::Err { message }` with that error message so the Orchestrator can surface it and optionally fall back to tools-only.
//!
//! Responsibilities:
//! - Call Semantic Router (`route_task`) to get strategy (cheap / expensive / tools_only).
//! - Execute per strategy: ToolsOnly (deterministic planner), CheapModel (single LLM), ExpensiveModel (planning + tool steps).
//! - All tool execution goes through Command Router -> Security Policy Engine -> Approval.
//!
//! Entrypoints:
//! - `agent_request`: run to completion with an executor; returns `AgentTaskResult`.
//! - `agent_step`: sync step for frontend-driven execution; returns `AgentStepOutcome`.

// ── Structured logging ────────────────────────────────────────────────────────

/// Structured agent lifecycle event. Always includes `session_id` and `event` fields.
///
/// Usage: `agent_event!(DEBUG, session_id, "plan_built", field = value, ...)`
///
/// The macro wraps `tracing::event!` and uses fully qualified paths so it works
/// in any submodule without additional `use` statements.
macro_rules! agent_event {
    ($level:ident, $sid:expr, $event:expr) => {
        ::tracing::event!(::tracing::Level::$level, session_id = %$sid, event = $event)
    };
    ($level:ident, $sid:expr, $event:expr, $($rest:tt)*) => {
        ::tracing::event!(::tracing::Level::$level, session_id = %$sid, event = $event, $($rest)*)
    };
}

// ── Submodules (implementation detail — not part of the external API) ─────────

mod types;
mod planning;
mod execution;

// ── Public API re-exports ──────────────────────────────────────────────────────

pub use types::{
    AgentConfig, AgentSessionState, AgentStepOutcome, AgentTaskInput, AgentTaskResult,
    ExpansionKind, OrchestratorAction, PolicyDecisionKind, SessionKind, SessionState,
    StepResult, TaskState, ToolCallIntent, ToolOutcome, ToolTrace,
};

pub use execution::{
    agent_request, agent_step, run_first_step, AgentToolExecutor,
};

pub use planning::start_session;

// next_action is used by oxcer_ffi and tests — keep it public.
pub use execution::next_action;

// ── Test-only re-exports: items accessed via `crate::orchestrator::*` in tests ─

#[cfg(test)]
pub(crate) use planning::{
    do_expand_plan,
    FS_RESULT_PLACEHOLDER,
    FILE_CONTENTS_PLACEHOLDER,
    MOST_RECENT_FILE_PLACEHOLDER,
    SUMMARIZER_SYSTEM_HINT,
};
