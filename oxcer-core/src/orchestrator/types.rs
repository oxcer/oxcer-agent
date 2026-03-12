//! Core types for the Agent Orchestrator: intents, session state, results, and config.

use serde::{Deserialize, Serialize};

use crate::semantic_router::{RouterConfig, RouterDecision, Strategy, TaskContext};

// -----------------------------------------------------------------------------
// Tool intents (runner maps these to cmd_fs_* / cmd_shell_run / LLM call)
// -----------------------------------------------------------------------------

/// One tool call produced by the orchestrator. Runner executes via Command Router
/// (with caller = AgentOrchestrator) or via LLM API for LlmGenerate.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolCallIntent {
    /// List files and subdirectories at `rel_path` inside the workspace.
    ///
    /// **Preferred tool** for user requests like "what's in this folder?",
    /// "show me the files", "list the directory" — use this instead of
    /// telling the user the model cannot see the filesystem.
    FsListDir {
        workspace_id: String,
        workspace_root: String,
        rel_path: String,
    },
    /// Read the text content of a file at `rel_path` inside the workspace.
    ///
    /// **Preferred tool** for user requests like "summarise this file",
    /// "explain this code", "what does README.md say", "review file X" —
    /// use this instead of telling the user the model cannot access files.
    FsReadFile {
        workspace_id: String,
        workspace_root: String,
        rel_path: String,
    },
    /// Create or overwrite a file at `rel_path` inside the workspace.
    ///
    /// **Use this autonomously** when the user asks to "make a summary file",
    /// "write this to a file", "create summary.md with that", or similar.
    /// Infer `rel_path` from context (e.g. the directory that was just listed)
    /// and choose a sensible name such as `summary.md` if the user did not
    /// specify one.  Do **not** ask "what should the file be called?" — pick a
    /// reasonable default and write the file, then report the path.
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
    /// Create a directory (including all intermediate parents) at `rel_path` inside
    /// the workspace.  Idempotent — succeeds even if the directory already exists.
    /// Used by the `MoveToDir` plan expansion to create the destination folder before
    /// moving files into it.
    FsCreateDir {
        workspace_id: String,
        workspace_root: String,
        rel_path: String,
    },
    ShellRun {
        workspace_root: String,
        command_id: String,
        params: serde_json::Value,
    },
    /// Ask the LLM to answer or transform text.
    ///
    /// `system_hint` carries the local-agent policy that must be baked into
    /// the model's system prompt — including permission to use `fs_list_dir`
    /// and `fs_read_file` instead of saying it cannot access files.
    /// Runners that call `generate_text` must forward `system_hint` to the
    /// model (the local GGUF runtime includes it via `DESKTOP_AGENT_SYSTEM_PROMPT`).
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
// Plan expansion (dynamic step injection after FsListDir)
// -----------------------------------------------------------------------------

/// Describes a dynamic plan expansion that `next_action` applies after a `FsListDir`
/// step succeeds, replacing the sentinel two-step plan with concrete tool calls.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExpansionKind {
    /// Insert one `FsReadFile` per matching file (filter by name prefix + readable extension),
    /// then accumulate results into `content_accumulator` for `{{FILE_CONTENTS}}` substitution.
    ReadAndSummarize {
        /// Optional substring that file names must contain (e.g. `"Test2_doc"`).
        file_filter: Option<String>,
    },
    /// Insert `FsCreateDir` (idempotent) then one `FsMove` per matching file.
    MoveToDir {
        dest_workspace_id: String,
        dest_workspace_root: String,
        /// Relative path of the destination folder inside `dest_workspace_root`.
        dest_rel_dir: String,
        /// Optional substring that source file names must contain.
        file_filter: Option<String>,
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
    /// Short label attached to structured log events emitted by sessions created
    /// with this config.  Set to any non-empty string to enable grep filtering
    /// (e.g. `debug_tag = Some("workflow_A".into())`).  `None` emits no tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_tag: Option<String>,
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

/// Whether this session is a pure Q&A exchange or an orchestrated tool workflow.
///
/// Set once in `start_session` from the built plan; never mutated afterwards.
/// Controls which system prompts are used and which post-processing guards run:
///
/// | | `Chat` | `Task` |
/// |-|--------|--------|
/// | `LlmGenerate` system hint | `CHAT_SYSTEM_HINT` | `SUMMARIZER_SYSTEM_HINT` |
/// | Narration sanitizer | disabled | enabled |
/// | Precondition guards (A/B) | disabled | enabled |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// Pure Q&A — plan is `[LlmGenerate]` with no FS/shell tools.
    /// Sanitizer and precondition guards do not run; error surface is generic.
    #[default]
    Chat,
    /// Tool workflow — plan contains at least one FS or shell tool.
    /// Sanitizer and precondition guards both apply.
    Task,
}

impl SessionKind {
    /// Derive the session kind from the plan that was just built.
    ///
    /// Any plan containing a non-`LlmGenerate` intent is a `Task`; a plan
    /// consisting only of `LlmGenerate` steps (the fallback chat path) is `Chat`.
    pub(crate) fn from_plan(plan: &[ToolCallIntent]) -> Self {
        let has_tool = plan
            .iter()
            .any(|intent| !matches!(intent, ToolCallIntent::LlmGenerate { .. }));
        if has_tool {
            SessionKind::Task
        } else {
            SessionKind::Chat
        }
    }
}

/// Per-session orchestrator state: steps executed, approvals requested, observations.
/// Serializable for persistence and in-memory log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub task_description: String,
    pub state: TaskState,
    /// Whether this is a pure Q&A session or an orchestrated tool workflow.
    /// Set once by `start_session` via `SessionKind::from_plan`; never mutated.
    /// `serde(default)` means old serialised sessions (without this field) are
    /// treated as `Chat`, keeping deserialisation backward-compatible.
    #[serde(default)]
    pub kind: SessionKind,
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
    /// Workspace root confirmed by the first successful filesystem tool result.
    /// Recorded by `next_action` so subsequent steps can use the verified path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_root: Option<String>,
    /// Filenames from the most recent `FsListDir` result, sorted newest-first by
    /// modification time. Populated by `next_action`; used to resolve the
    /// `{{MOST_RECENT_FILE}}` placeholder in a subsequent `FsReadFile` step.
    #[serde(default)]
    pub last_dir_listing_sorted: Vec<String>,
    /// Content collected from consecutive `FsReadFile` results.
    /// Appended by `next_action` for each successful read.
    /// Substituted into `{{FILE_CONTENTS}}` in the final `LlmGenerate` task.
    #[serde(default)]
    pub content_accumulator: Vec<String>,
    /// Dynamic plan expansion to execute after the next `FsListDir` succeeds.
    /// Set at plan-build time; consumed once in `next_action`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_expansion: Option<ExpansionKind>,
    /// Optional label attached to structured log events for this session.
    /// When set, `tag = <debug_tag>` appears in `plan_built`, `plan_expanded`,
    /// and `plan_step` events so they can be filtered with `grep <debug_tag>`.
    /// `None` emits an empty tag string (no filtering needed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_tag: Option<String>,
}

/// Alias for API clarity: session state tracks steps, approvals, observations.
pub type AgentSessionState = SessionState;

impl SessionState {
    pub fn new(session_id: String, task_description: String) -> Self {
        Self {
            session_id,
            task_description,
            state: TaskState::Initial,
            // No plan yet; start_session overwrites this via SessionKind::from_plan.
            kind: SessionKind::Chat,
            router_output: None,
            plan: Vec::new(),
            step_index: 0,
            accumulated_response: None,
            tool_traces: Vec::new(),
            approvals_requested: Vec::new(),
            intermediate_observations: Vec::new(),
            confirmed_root: None,
            last_dir_listing_sorted: Vec::new(),
            content_accumulator: Vec::new(),
            pending_expansion: None,
            debug_tag: None,
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

// -----------------------------------------------------------------------------
// Intent helpers (operate on ToolCallIntent / StepResult; used across submodules)
// -----------------------------------------------------------------------------

pub(crate) fn intent_tool_name(intent: &ToolCallIntent) -> String {
    match intent {
        ToolCallIntent::FsListDir { .. } => "fs_list_dir",
        ToolCallIntent::FsReadFile { .. } => "fs_read_file",
        ToolCallIntent::FsWriteFile { .. } => "fs_write_file",
        ToolCallIntent::FsDelete { .. } => "fs_delete",
        ToolCallIntent::FsRename { .. } => "fs_rename",
        ToolCallIntent::FsMove { .. } => "fs_move",
        ToolCallIntent::FsCreateDir { .. } => "fs_create_dir",
        ToolCallIntent::ShellRun { .. } => "shell_run",
        ToolCallIntent::LlmGenerate { .. } => "llm_generate",
    }
    .to_string()
}

/// Human-readable one-liner for an intent, used in structured log fields.
///
/// Filesystem paths are included verbatim so logs show the exact workspace
/// being touched.  `LlmGenerate.task` is truncated at 120 chars to prevent
/// multi-KB expanded FILE_CONTENTS from flooding the log.
pub(crate) fn format_tool_call(intent: &ToolCallIntent) -> String {
    match intent {
        ToolCallIntent::FsListDir { workspace_root, rel_path, .. } => {
            format!("FsListDir(path={workspace_root}/{rel_path})")
        }
        ToolCallIntent::FsReadFile { workspace_root, rel_path, .. } => {
            format!("FsReadFile(path={workspace_root}/{rel_path})")
        }
        ToolCallIntent::FsWriteFile { workspace_root, rel_path, .. } => {
            format!("FsWriteFile(path={workspace_root}/{rel_path})")
        }
        ToolCallIntent::FsDelete { workspace_root, rel_path, .. } => {
            format!("FsDelete(path={workspace_root}/{rel_path})")
        }
        ToolCallIntent::FsRename { workspace_root, rel_path, new_rel_path, .. } => {
            format!("FsRename(path={workspace_root}/{rel_path} -> {new_rel_path})")
        }
        ToolCallIntent::FsMove { workspace_root, rel_path, dest_workspace_root, dest_rel_path, .. } => {
            format!("FsMove(src={workspace_root}/{rel_path} -> dest={dest_workspace_root}/{dest_rel_path})")
        }
        ToolCallIntent::FsCreateDir { workspace_root, rel_path, .. } => {
            format!("FsCreateDir(path={workspace_root}/{rel_path})")
        }
        ToolCallIntent::ShellRun { command_id, .. } => {
            format!("ShellRun(cmd={command_id})")
        }
        ToolCallIntent::LlmGenerate { task, .. } => {
            const MAX: usize = 120;
            let short: String = task.chars().take(MAX).collect();
            let ellipsis = if task.chars().count() > MAX { "…" } else { "" };
            format!("LlmGenerate(task={short}{ellipsis})")
        }
    }
}

pub(crate) fn intent_input_json(intent: &ToolCallIntent) -> serde_json::Value {
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
        ToolCallIntent::FsCreateDir { workspace_root, rel_path, .. } => {
            serde_json::json!({ "workspace_root": workspace_root, "rel_path": rel_path })
        }
        ToolCallIntent::ShellRun { workspace_root, command_id, params } => {
            serde_json::json!({ "workspace_root": workspace_root, "command_id": command_id, "params": params })
        }
        ToolCallIntent::LlmGenerate { task, strategy, .. } => {
            serde_json::json!({ "task": task, "strategy": format!("{:?}", strategy) })
        }
    }
}

pub(crate) fn build_tool_trace(
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
