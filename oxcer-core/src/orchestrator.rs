//! Agent Orchestrator: cost-aware execution over Command Router.
//!
//! ## Core types and flow
//!
//! - **SessionState**: Per-session task machine (router_output, plan, step_index, tool_traces).
//! - **ToolCallIntent**: One tool call (FsListDir, FsDelete, LlmGenerate, etc.); runner executes via Command Router.
//! - **StepResult**: Outcome of one tool execution (Ok, Err, ApprovalPending).
//! - **Flow**: `start_session` → build plan from router; `agent_step` / `next_action` advance state; executor runs tools and feeds results back.
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
//! - All tool execution goes through Command Router → Security Policy Engine → Approval.
//!
//! Entrypoints:
//! - `agent_request`: run to completion with an executor; returns `AgentTaskResult`.
//! - `agent_step`: sync step for frontend-driven execution; returns `AgentStepOutcome`.

use serde::{Deserialize, Serialize};

use crate::prompt_sanitizer::sanitize_task_for_llm;
use crate::semantic_router::{route, RouterConfig, RouterDecision, RouterInput, Strategy, TaskContext};

// -----------------------------------------------------------------------------
// Tool intents (runner maps these to cmd_fs_* / cmd_shell_run / LLM call)
// -----------------------------------------------------------------------------

/// One tool call produced by the orchestrator. Runner executes via Command Router
/// (with caller = AgentOrchestrator) or via LLM API for LlmGenerate.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolCallIntent {
    FsListDir {
        workspace_id: String,
        workspace_root: String,
        rel_path: String,
    },
    FsReadFile {
        workspace_id: String,
        workspace_root: String,
        rel_path: String,
    },
    FsWriteFile {
        workspace_id: String,
        workspace_root: String,
        rel_path: String,
        contents_base64: String,
    },
    FsDelete {
        workspace_id: String,
        workspace_root: String,
        rel_path: String,
    },
    FsRename {
        workspace_id: String,
        workspace_root: String,
        rel_path: String,
        new_rel_path: String,
    },
    FsMove {
        workspace_id: String,
        workspace_root: String,
        rel_path: String,
        dest_workspace_root: String,
        dest_rel_path: String,
    },
    ShellRun {
        workspace_root: String,
        command_id: String,
        params: serde_json::Value,
    },
    /// Runner calls the appropriate remote API (OpenAI/Gemini/Anthropic/Grok).
    LlmGenerate {
        strategy: Strategy,
        task: String,
        system_hint: Option<String>,
    },
}

// -----------------------------------------------------------------------------
// Step result (from runner back to orchestrator)
// -----------------------------------------------------------------------------

/// Result of executing one tool intent. Runner fills this after Command Router or LLM call.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepResult {
    Ok {
        payload: serde_json::Value,
    },
    Err {
        message: String,
    },
    /// Tool required approval; request_id for frontend to resolve via cmd_approve_and_execute.
    ApprovalPending {
        request_id: String,
    },
}

// -----------------------------------------------------------------------------
// Agent API: task input, result, tool trace, config
// -----------------------------------------------------------------------------

/// Input for a single agent task (task description + context).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentTaskInput {
    pub task_description: String,
    pub context: TaskContext,
}

/// Policy decision for a tool call (for logging / UI).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecisionKind {
    Allow,
    Deny,
    RequireApproval,
}

/// One tool invocation trace: for logging and UI.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolTrace {
    pub tool_name: String,
    pub input: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_decision: Option<PolicyDecisionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
}

/// Final result of an agent task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentTaskResult {
    /// Human-readable answer (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_answer: Option<String>,
    /// Tool invocations for logging / UI.
    pub tool_traces: Vec<ToolTrace>,
}

/// Config for agent execution (router, default workspace, models).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub router_config: RouterConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_workspace_root: Option<String>,
}

// -----------------------------------------------------------------------------
// Session state (per-session task machine)
// -----------------------------------------------------------------------------

/// State of the task in this session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Router not yet run.
    Initial,
    /// Plan created; executing steps.
    Executing,
    /// Done (success or failure).
    Complete,
}

/// Per-session orchestrator state: steps executed, approvals requested, observations.
/// Serializable for persistence and in-memory log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub task_description: String,
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub router_output: Option<RouterDecision>,
    /// Plan: sequence of intents to execute.
    pub plan: Vec<ToolCallIntent>,
    /// Current step index (0..plan.len()).
    pub step_index: usize,
    /// Accumulated response (e.g. last LLM reply or summary of tool outputs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accumulated_response: Option<String>,
    /// Tool invocations so far (for logging / UI).
    #[serde(default)]
    pub tool_traces: Vec<ToolTrace>,
    /// Request IDs for which approval was requested (not yet resolved).
    #[serde(default)]
    pub approvals_requested: Vec<String>,
    /// Intermediate observations (e.g. "Planning call completed", "Step 2: delete approved").
    #[serde(default)]
    pub intermediate_observations: Vec<String>,
}

/// Alias for API clarity: session state tracks steps, approvals, observations.
pub type AgentSessionState = SessionState;

impl SessionState {
    pub fn new(session_id: String, task_description: String) -> Self {
        Self {
            session_id,
            task_description,
            state: TaskState::Initial,
            router_output: None,
            plan: Vec::new(),
            step_index: 0,
            accumulated_response: None,
            tool_traces: Vec::new(),
            approvals_requested: Vec::new(),
            intermediate_observations: Vec::new(),
        }
    }
}

// -----------------------------------------------------------------------------
// Orchestrator action (what the runner should do next)
// -----------------------------------------------------------------------------

/// Outcome of one orchestrator step: either done, or one tool call, or waiting for approval.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum OrchestratorAction {
    Complete {
        response: String,
        session: SessionState,
    },
    ToolCall {
        intent: ToolCallIntent,
        session: SessionState,
    },
    AwaitingApproval {
        request_id: String,
        session: SessionState,
    },
}

/// Outcome of one agent step (API-oriented): complete with result, or need tool, or awaiting approval.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AgentStepOutcome {
    Complete(AgentTaskResult),
    NeedTool {
        intent: ToolCallIntent,
        session: AgentSessionState,
    },
    AwaitingApproval {
        request_id: String,
        session: AgentSessionState,
    },
}

fn intent_tool_name(intent: &ToolCallIntent) -> String {
    match intent {
        ToolCallIntent::FsListDir { .. } => "fs_list_dir",
        ToolCallIntent::FsReadFile { .. } => "fs_read_file",
        ToolCallIntent::FsWriteFile { .. } => "fs_write_file",
        ToolCallIntent::FsDelete { .. } => "fs_delete",
        ToolCallIntent::FsRename { .. } => "fs_rename",
        ToolCallIntent::FsMove { .. } => "fs_move",
        ToolCallIntent::ShellRun { .. } => "shell_run",
        ToolCallIntent::LlmGenerate { .. } => "llm_generate",
    }
    .to_string()
}

fn intent_input_json(intent: &ToolCallIntent) -> serde_json::Value {
    match intent {
        ToolCallIntent::FsListDir { workspace_root, rel_path, .. } => {
            serde_json::json!({ "workspace_root": workspace_root, "rel_path": rel_path })
        }
        ToolCallIntent::FsReadFile { workspace_root, rel_path, .. } => {
            serde_json::json!({ "workspace_root": workspace_root, "rel_path": rel_path })
        }
        ToolCallIntent::FsWriteFile { workspace_root, rel_path, .. } => {
            serde_json::json!({ "workspace_root": workspace_root, "rel_path": rel_path })
        }
        ToolCallIntent::FsDelete { workspace_root, rel_path, .. } => {
            serde_json::json!({ "workspace_root": workspace_root, "rel_path": rel_path })
        }
        ToolCallIntent::FsRename { workspace_root, rel_path, new_rel_path, .. } => serde_json::json!({
            "workspace_root": workspace_root,
            "rel_path": rel_path,
            "new_rel_path": new_rel_path
        }),
        ToolCallIntent::FsMove { workspace_root, rel_path, dest_workspace_root, dest_rel_path, .. } => {
            serde_json::json!({
                "workspace_root": workspace_root,
                "rel_path": rel_path,
                "dest_workspace_root": dest_workspace_root,
                "dest_rel_path": dest_rel_path
            })
        }
        ToolCallIntent::ShellRun { workspace_root, command_id, params } => {
            serde_json::json!({ "workspace_root": workspace_root, "command_id": command_id, "params": params })
        }
        ToolCallIntent::LlmGenerate { task, strategy, .. } => {
            serde_json::json!({ "task": task, "strategy": format!("{:?}", strategy) })
        }
    }
}

fn build_tool_trace(
    intent: &ToolCallIntent,
    result: &StepResult,
) -> ToolTrace {
    let tool_name = intent_tool_name(intent);
    let input = intent_input_json(intent);
    let (policy_decision, approved, result_summary) = match result {
        StepResult::Ok { payload } => (
            Some(PolicyDecisionKind::Allow),
            Some(true),
            payload.get("text").and_then(|v| v.as_str()).map(String::from).or_else(|| {
                serde_json::to_string(payload).ok().map(|s| if s.len() > 200 { format!("{}...", &s[..200]) } else { s })
            }),
        ),
        StepResult::Err { message } => (None, Some(false), Some(message.clone())),
        StepResult::ApprovalPending { .. } => (Some(PolicyDecisionKind::RequireApproval), None, None),
    };
    ToolTrace {
        tool_name,
        input,
        policy_decision,
        approved,
        result_summary,
    }
}

// -----------------------------------------------------------------------------
// Plan building (heuristic for Sprint 6; LLM planner can be added later)
// -----------------------------------------------------------------------------

/// Deterministic planner for ToolsOnly: no LLM; single FS/Shell commands.
/// All commands go through Security Policy Engine and approval flow (delete/rename/move).
fn build_plan_tools_only(
    task: &str,
    context: &TaskContext,
    default_workspace_id: Option<&str>,
    default_workspace_root: Option<&str>,
) -> Vec<ToolCallIntent> {
    let task_lower = task.to_lowercase();
    let ws_id = context
        .workspace_id
        .as_deref()
        .or(default_workspace_id)
        .unwrap_or("")
        .to_string();
    let ws_root = default_workspace_root.unwrap_or("").to_string();
    if ws_root.is_empty() {
        return Vec::new();
    }

    let mut intents = Vec::new();

    // "list files (in workspace)" / "list dir" / "ls" → single FsListDir
    if task_lower.contains("list files")
        || task_lower.contains("list dir")
        || task_lower.contains("list the files")
        || (task_lower.contains("list") && (task_lower.contains("file") || task_lower.contains("dir")))
        || task_lower.trim() == "ls"
    {
        intents.push(ToolCallIntent::FsListDir {
            workspace_id: ws_id.clone(),
            workspace_root: ws_root.clone(),
            rel_path: ".".to_string(),
        });
        return intents;
    }

    // "delete X" / "remove X" → single FsDelete (always goes through policy + approval)
    let delete_prefixes = ["delete ", "remove ", "rm "];
    for prefix in delete_prefixes {
        if task_lower.starts_with(prefix) || task_lower.contains(&format!(" {} ", prefix.trim())) {
            let rest = task_lower
                .strip_prefix(prefix)
                .or_else(|| task_lower.split(prefix).nth(1))
                .unwrap_or("")
                .trim();
            let path = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| c == '"' || c == '\'');
            if !path.is_empty() {
                intents.push(ToolCallIntent::FsDelete {
                    workspace_id: ws_id,
                    workspace_root: ws_root,
                    rel_path: path.to_string(),
                });
                return intents;
            }
        }
    }

    intents
}

fn build_plan_with_llm(task: &str, strategy: Strategy) -> Vec<ToolCallIntent> {
    let task_sanitized = sanitize_task_for_llm(task);
    vec![ToolCallIntent::LlmGenerate {
        strategy,
        task: task_sanitized,
        system_hint: None,
    }]
}

// -----------------------------------------------------------------------------
// Next action (state machine step)
// -----------------------------------------------------------------------------

/// Builds initial session: run router and build plan. Call once at task start.
pub fn start_session(
    session_id: String,
    input: RouterInput,
    default_workspace_id: Option<String>,
    default_workspace_root: Option<String>,
) -> (SessionState, Option<ToolCallIntent>) {
    let router_output = route(&input);
    let task = input.task_description.clone();
    let context = input.context.clone();

    let plan: Vec<ToolCallIntent> = match router_output.strategy {
        Strategy::ToolsOnly => build_plan_tools_only(
            &task,
            &context,
            default_workspace_id.as_deref(),
            default_workspace_root.as_deref(),
        ),
        Strategy::CheapModel | Strategy::ExpensiveModel => {
            build_plan_with_llm(&task, router_output.strategy)
        }
    };

    let session = SessionState {
        session_id: session_id.clone(),
        task_description: task,
        state: TaskState::Executing,
        router_output: Some(router_output),
        plan: plan.clone(),
        step_index: 0,
        accumulated_response: None,
        tool_traces: Vec::new(),
        approvals_requested: Vec::new(),
        intermediate_observations: Vec::new(),
    };

    let first_intent = if session.plan.is_empty() {
        None
    } else {
        session.plan.get(0).cloned()
    };

    (session, first_intent)
}

/// Advances the session: apply last tool result (if any) and return next action.
/// Runner calls this after executing a ToolCallIntent or after user approval.
/// Records tool traces and approvals_requested for logging / UI.
pub fn next_action(
    mut session: SessionState,
    last_result: Option<StepResult>,
) -> Result<OrchestratorAction, String> {
    // Apply last result if present: record trace (except for ApprovalPending), then update state
    if let Some(res) = &last_result {
        match res {
            StepResult::ApprovalPending { request_id } => {
                session.approvals_requested.push(request_id.clone());
                return Ok(OrchestratorAction::AwaitingApproval {
                    request_id: request_id.clone(),
                    session,
                });
            }
            _ => {
                if session.step_index < session.plan.len() {
                    let intent = &session.plan[session.step_index];
                    let trace = build_tool_trace(intent, res);
                    session.tool_traces.push(trace);
                }
            }
        }
        match res {
            StepResult::ApprovalPending { .. } => unreachable!(),
            StepResult::Err { message } => {
                session.state = TaskState::Complete;
                session.accumulated_response = Some(format!("Error: {}", message));
                session
                    .intermediate_observations
                    .push(format!("Step failed: {}", message));
                return Ok(OrchestratorAction::Complete {
                    response: session.accumulated_response.clone().unwrap_or_default(),
                    session,
                });
            }
            StepResult::Ok { payload } => {
                if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
                    session.accumulated_response = Some(text.to_string());
                } else if let Some(s) = serde_json::to_string(payload).ok() {
                    session.accumulated_response = Some(s);
                }
                session.step_index += 1;
            }
        }
    }

    // Initial step (no last_result): emit first intent
    if last_result.is_none() && !session.plan.is_empty() {
        let intent = session.plan[0].clone();
        return Ok(OrchestratorAction::ToolCall {
            intent,
            session,
        });
    }

    // More steps?
    if session.step_index < session.plan.len() {
        let intent = session.plan[session.step_index].clone();
        return Ok(OrchestratorAction::ToolCall {
            intent,
            session,
        });
    }

    // No more steps
    session.state = TaskState::Complete;
    let response = session
        .accumulated_response
        .clone()
        .unwrap_or_else(|| "Done.".to_string());
    Ok(OrchestratorAction::Complete {
        response,
        session,
    })
}

fn build_agent_task_result(session: &SessionState) -> AgentTaskResult {
    AgentTaskResult {
        final_answer: session.accumulated_response.clone(),
        tool_traces: session.tool_traces.clone(),
    }
}

/// Agent step (API): one step of the orchestrator. Update session in place; return outcome.
/// First call: pass `last_result: None`; orchestrator runs `route_task`, builds plan, returns `NeedTool` or `Complete`.
/// Subsequent calls: pass the previous step's result; returns next `NeedTool`, `AwaitingApproval`, or `Complete`.
pub fn agent_step(
    input: AgentTaskInput,
    session: &mut AgentSessionState,
    config: &AgentConfig,
    last_result: Option<StepResult>,
) -> Result<AgentStepOutcome, String> {
    if session.state == TaskState::Initial && last_result.is_none() {
        let router_input = RouterInput {
            task_description: input.task_description,
            context: input.context,
            config: config.router_config.clone(),
            capabilities: None,
        };
        let (new_session, first_intent) = start_session(
            session.session_id.clone(),
            router_input,
            config.default_workspace_id.clone(),
            config.default_workspace_root.clone(),
        );
        *session = new_session;
        return match first_intent {
            Some(intent) => Ok(AgentStepOutcome::NeedTool {
                intent,
                session: session.clone(),
            }),
            None => Ok(AgentStepOutcome::Complete(build_agent_task_result(session))),
        };
    }
    if let Some(result) = last_result {
        let action = next_action(session.clone(), Some(result))?;
        match action {
            OrchestratorAction::Complete { session: s, .. } => {
                *session = s;
                Ok(AgentStepOutcome::Complete(build_agent_task_result(session)))
            }
            OrchestratorAction::ToolCall { intent, session: s } => {
                *session = s;
                Ok(AgentStepOutcome::NeedTool {
                    intent,
                    session: session.clone(),
                })
            }
            OrchestratorAction::AwaitingApproval { request_id, session: s } => {
                *session = s;
                Ok(AgentStepOutcome::AwaitingApproval {
                    request_id,
                    session: session.clone(),
                })
            }
        }
    } else {
        Err("last_result required when session is already executing".to_string())
    }
}

/// Executor for tool runs: used by `agent_request` to run tools and resolve approvals.
/// Implement this to drive the agent to completion (e.g. Tauri commands with caller "agent_orchestrator").
pub trait AgentToolExecutor {
    /// Execute one tool intent. Returns outcome or error.
    fn execute_tool(&self, intent: ToolCallIntent) -> Result<ToolOutcome, String>;
    /// Block until approval is resolved and return the execution result (or error if denied).
    fn resolve_approval(&self, request_id: &str, approved: bool) -> Result<serde_json::Value, String>;
}

/// Result of executing a tool (before or after approval).
#[derive(Clone, Debug)]
pub enum ToolOutcome {
    Ok(serde_json::Value),
    ApprovalPending(String),
    Err(String),
}

impl ToolOutcome {
    pub fn into_step_result(self) -> StepResult {
        match self {
            ToolOutcome::Ok(payload) => StepResult::Ok { payload },
            ToolOutcome::ApprovalPending(request_id) => StepResult::ApprovalPending { request_id },
            ToolOutcome::Err(message) => StepResult::Err { message },
        }
    }
}

/// Run the agent to completion using the given executor. Sync: blocks on tool execution and approval.
/// For frontend-driven execution (no blocking executor), use `agent_step` in a loop instead.
pub fn agent_request(
    input: AgentTaskInput,
    session: &mut AgentSessionState,
    config: &AgentConfig,
    executor: &impl AgentToolExecutor,
) -> Result<AgentTaskResult, String> {
    let mut last_result: Option<StepResult> = None;
    loop {
        let outcome = agent_step(input.clone(), session, config, last_result.take())?;
        match outcome {
            AgentStepOutcome::Complete(result) => return Ok(result),
            AgentStepOutcome::NeedTool { intent, .. } => {
                let outcome = executor.execute_tool(intent)?;
                last_result = Some(outcome.into_step_result());
            }
            AgentStepOutcome::AwaitingApproval { request_id, .. } => {
                let resolved = executor.resolve_approval(&request_id, true)?;
                last_result = Some(StepResult::Ok {
                    payload: resolved,
                });
            }
        }
    }
}

/// Run from a fresh session: route, build plan, return first action (or complete if no steps).
pub fn run_first_step(
    session_id: String,
    input: RouterInput,
    default_workspace_id: Option<String>,
    default_workspace_root: Option<String>,
) -> Result<OrchestratorAction, String> {
    let (session, first_intent) = start_session(
        session_id,
        input,
        default_workspace_id,
        default_workspace_root,
    );

    if let Some(intent) = first_intent {
        return Ok(OrchestratorAction::ToolCall {
            intent,
            session,
        });
    }

    // Empty plan (e.g. tools_only with no heuristic steps)
    next_action(session, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic_router::{RouterConfig, TaskContext};

    #[test]
    fn start_session_creates_plan_with_llm_step() {
        let input = RouterInput {
            task_description: "What is Rust?".to_string(),
            context: TaskContext::default(),
            config: Default::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            None,
            None,
        );
        assert_eq!(session.state, TaskState::Executing);
        assert_eq!(session.plan.len(), 1);
        assert!(matches!(&session.plan[0], ToolCallIntent::LlmGenerate { .. }));
        assert!(first.is_some());
    }

    #[test]
    fn start_session_tools_only_list_files() {
        let input = RouterInput {
            task_description: "list files in workspace".to_string(),
            context: TaskContext::default(),
            config: RouterConfig {
                prefer_tools_only: true,
                ..Default::default()
            },
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(session.state, TaskState::Executing);
        assert!(
            session.plan.iter().any(|step| {
                matches!(step, ToolCallIntent::FsListDir { rel_path, .. } if rel_path == ".")
            }),
            "tools-only list files plan should include FsListDir(\".\")"
        );
        assert!(first.is_some());
    }

    #[test]
    fn next_action_after_ok_completes() {
        let input = RouterInput {
            task_description: "Hello".to_string(),
            context: TaskContext::default(),
            config: Default::default(),
            capabilities: None,
        };
        let (mut session, _) = start_session("s1".to_string(), input, None, None);
        session.step_index = 1; // one step done
        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": "Rust is a systems language." }),
            }),
        )
        .unwrap();
        match action {
            OrchestratorAction::Complete { response, .. } => {
                assert!(response.contains("Rust"));
            }
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn agent_step_first_call_returns_need_tool() {
        let mut session = SessionState::new("s1".to_string(), "What is Rust?".to_string());
        let config = AgentConfig::default();
        let input = AgentTaskInput {
            task_description: "What is Rust?".to_string(),
            context: TaskContext::default(),
        };
        let outcome = agent_step(input, &mut session, &config, None).unwrap();
        match outcome {
            AgentStepOutcome::NeedTool { intent, .. } => {
                assert!(matches!(intent, ToolCallIntent::LlmGenerate { .. }));
            }
            _ => panic!("expected NeedTool"),
        }
    }

    #[test]
    fn agent_step_complete_builds_task_result() {
        let mut session = SessionState::new("s1".to_string(), "Hello".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![ToolCallIntent::LlmGenerate {
            strategy: Strategy::CheapModel,
            task: "Hello".to_string(),
            system_hint: None,
        }];
        session.step_index = 0; // about to process first (and only) step result
        let config = AgentConfig::default();
        let input = AgentTaskInput {
            task_description: "Hello".to_string(),
            context: TaskContext::default(),
        };
        let outcome = agent_step(
            input,
            &mut session,
            &config,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": "Hi there." }),
            }),
        )
        .unwrap();
        match outcome {
            AgentStepOutcome::Complete(result) => {
                assert_eq!(result.final_answer.as_deref(), Some("Hi there."));
                assert_eq!(result.tool_traces.len(), 1);
                assert_eq!(result.tool_traces[0].tool_name, "llm_generate");
            }
            _ => panic!("expected Complete"),
        }
    }

    /// Tools-only: "delete file X" → orchestrator emits single FsDelete intent.
    #[test]
    fn start_session_tools_only_delete_emits_fs_delete() {
        let input = RouterInput {
            task_description: "delete foo.txt".to_string(),
            context: TaskContext::default(),
            config: RouterConfig {
                prefer_tools_only: true,
                ..Default::default()
            },
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(session.state, TaskState::Executing);
        assert_eq!(session.plan.len(), 1);
        match &session.plan[0] {
            ToolCallIntent::FsDelete { rel_path, workspace_id, .. } => {
                assert_eq!(rel_path, "foo.txt");
                assert_eq!(workspace_id, "ws1");
            }
            _ => panic!("expected FsDelete intent"),
        }
        assert!(first.is_some());
    }

    /// State machine: tools-only delete — first step returns NeedTool(FsDelete), then Ok result → Complete.
    #[test]
    fn orchestrator_tools_only_delete_state_machine() {
        let mut session = SessionState::new("s1".to_string(), "delete bar.txt".to_string());
        let config = AgentConfig {
            default_workspace_id: Some("w1".to_string()),
            default_workspace_root: Some("/tmp/proj".to_string()),
            router_config: RouterConfig {
                prefer_tools_only: true,
                ..Default::default()
            },
        };
        let input = AgentTaskInput {
            task_description: "delete bar.txt".to_string(),
            context: TaskContext::default(),
        };

        let out1 = agent_step(input.clone(), &mut session, &config, None).unwrap();
        let intent = match &out1 {
            AgentStepOutcome::NeedTool { intent, .. } => intent.clone(),
            _ => panic!("expected NeedTool, got {:?}", out1),
        };
        match &intent {
            ToolCallIntent::FsDelete { rel_path, .. } => assert_eq!(rel_path, "bar.txt"),
            _ => panic!("expected FsDelete"),
        }

        let out2 = agent_step(
            input,
            &mut session,
            &config,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "deleted": true }),
            }),
        )
        .unwrap();
        match out2 {
            AgentStepOutcome::Complete(result) => {
                assert_eq!(result.tool_traces.len(), 1);
                assert_eq!(result.tool_traces[0].tool_name, "fs_delete");
            }
            _ => panic!("expected Complete"),
        }
    }
}
