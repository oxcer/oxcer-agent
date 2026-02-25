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

use serde::{Deserialize, Serialize};

// ── Structured logging ────────────────────────────────────────────────────────

/// Structured agent lifecycle event. Always includes `session_id` and `event` fields.
/// See oxcer_ffi/src/lib.rs for full documentation of this macro.
macro_rules! agent_event {
    ($level:ident, $sid:expr, $event:expr) => {
        ::tracing::event!(::tracing::Level::$level, session_id = %$sid, event = $event)
    };
    ($level:ident, $sid:expr, $event:expr, $($rest:tt)*) => {
        ::tracing::event!(::tracing::Level::$level, session_id = %$sid, event = $event, $($rest)*)
    };
}

// ─────────────────────────────────────────────────────────────────────────────

use crate::prompt_sanitizer::sanitize_task_for_llm;
use crate::semantic_router::{
    has_implicit_file_read_intent, has_implicit_fs_intent, route, RouterConfig, RouterDecision,
    RouterInput, Strategy, TaskContext,
};

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

    // "list files (in workspace)" / "list dir" / "ls" -> single FsListDir
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

    // "delete X" / "remove X" -> single FsDelete (always goes through policy + approval)
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

/// System hint forwarded in every LlmGenerate intent.
///
/// Runners that build the final prompt (e.g. `LlamaCppPhiRuntime`) must inject
/// this as the `<|system|>` block.  The text mirrors `DESKTOP_AGENT_SYSTEM_PROMPT`
/// in `local_phi3/runtime.rs` — both must stay in sync when the policy changes.
const AGENT_SYSTEM_HINT: &str = "\
You are Oxcer, a local desktop AI assistant. \
You are allowed to read, write, and list files on this machine.\n\
RULE 1 — File Contents: When the user asks you to summarize, describe, explain, or \
quote a file or document, you MUST call fs_read_file on that file FIRST. Base your \
answer solely on the returned content. Never invent, guess, or paraphrase file \
contents you have not read with fs_read_file. If fs_read_file fails or the file \
cannot be found, say so explicitly instead of fabricating a summary.\n\
RULE 2 — Directory Listings: When the user asks what is in a folder or directory, \
you MUST call fs_list_dir first and base your answer solely on the tool result. \
Never invent file names or folder structures.\n\
RULE 3 — File Creation: When the user asks you to \"make a summary file\", \
\"write this to a file\", \"create summary.md\", or any similar write request: \
use prior tool outputs from this conversation to determine what to write. Choose \
a sensible file name (e.g. summary.md) if the user has not specified one. Call \
fs_write_file to create the file. Do NOT ask \"which file?\" when you already \
listed or read files in this session — act on the context you already have.\n\
RULE 4 — No repeated questions: Never ask the user to repeat information that is \
already visible from a prior tool result in the same conversation. If the user \
says \"make summary.md with that\" after a directory listing, \"that\" refers to \
the files just listed — read and summarise them without asking which ones.\n\
If a tool fails, say so explicitly rather than inventing content.";

/// Placeholder substituted in `LlmGenerate.task` when the orchestrator has
/// accumulated a real filesystem tool result and needs to inject it into the prompt.
const FS_RESULT_PLACEHOLDER: &str = "{{FS_RESULT}}";

fn build_plan_with_llm(task: &str, strategy: Strategy) -> Vec<ToolCallIntent> {
    let task_sanitized = sanitize_task_for_llm(task);
    vec![ToolCallIntent::LlmGenerate {
        strategy,
        task: task_sanitized,
        system_hint: Some(AGENT_SYSTEM_HINT.to_string()),
    }]
}

/// Resolves a concrete filesystem path from a natural-language task string.
///
/// Recognises well-known macOS directory names (Desktop, Documents, Downloads)
/// and the user's home directory.  Returns
/// `(workspace_id, absolute_directory_path, rel_path)` where `rel_path` is always
/// `"."` (list the whole directory).
///
/// Returns `None` when no recognisable path can be extracted and no default
/// workspace root is available — the caller must guard against inventing a path.
fn extract_fs_path(
    task: &str,
    default_workspace_id: Option<&str>,
    default_workspace_root: Option<&str>,
) -> Option<(String, String, String)> {
    let task_lower = task.to_lowercase();
    let home = dirs_next::home_dir();

    // Well-known macOS user directories (checked in order — longer/more-specific first).
    let well_known = [
        ("documents", "Documents"),
        ("downloads", "Downloads"),
        ("desktop", "Desktop"),
    ];

    for (keyword, dir_name) in &well_known {
        if task_lower.contains(keyword) {
            let home_ref = home.as_ref()?;
            let full_path = home_ref.join(dir_name);
            let ws_id = default_workspace_id.unwrap_or("").to_string();
            return Some((ws_id, full_path.to_string_lossy().into_owned(), ".".to_string()));
        }
    }

    // "home folder" / "home directory" / "~" / bare "home"
    if task_lower.contains("home folder")
        || task_lower.contains("home directory")
        || task_lower.contains("home dir")
        || task_lower.contains("~")
        || task_lower.contains(" home")
    {
        let home_path = home?.to_string_lossy().into_owned();
        let ws_id = default_workspace_id.unwrap_or("").to_string();
        return Some((ws_id, home_path, ".".to_string()));
    }

    // Fall back to default workspace root (if provided and non-empty).
    if let Some(root) = default_workspace_root {
        if !root.is_empty() {
            let ws_id = default_workspace_id.unwrap_or("").to_string();
            return Some((ws_id, root.to_string(), ".".to_string()));
        }
    }

    None
}

/// Builds a two-step plan: first list the filesystem path, then ask the LLM
/// to summarise using the real tool result.
///
/// Step 1: `FsListDir` at the resolved path.
/// Step 2: `LlmGenerate` with `{{FS_RESULT}}` in the prompt — `next_action`
///         substitutes the accumulated listing before emitting the intent.
fn build_plan_fs_then_llm(
    task: &str,
    ws_id: String,
    ws_root: String,
    rel_path: String,
    strategy: Strategy,
) -> Vec<ToolCallIntent> {
    let llm_task = format!(
        "The user asked: \"{task}\"\n\n\
         Here is the actual directory listing returned by the filesystem tool:\n\
         {placeholder}\n\n\
         Using ONLY the information above, provide a concise summary. \
         Do NOT invent or add any file names or content that is not in the tool result.",
        task = sanitize_task_for_llm(task),
        placeholder = FS_RESULT_PLACEHOLDER,
    );

    vec![
        ToolCallIntent::FsListDir {
            workspace_id: ws_id,
            workspace_root: ws_root,
            rel_path,
        },
        ToolCallIntent::LlmGenerate {
            strategy,
            task: llm_task,
            system_hint: Some(AGENT_SYSTEM_HINT.to_string()),
        },
    ]
}

/// Tries to extract an explicit file path (token ending with a known extension, or an
/// absolute path) from the task string.
///
/// Returns `(workspace_id, workspace_root, rel_path)` or `None` if no recognisable
/// file token is found.
///
/// For absolute paths (starting with `/`): `workspace_root = "/"`, `rel_path = path[1..]`.
/// For bare filenames: the provided default workspace root is used.
fn extract_explicit_file_path(
    task: &str,
    default_workspace_id: Option<&str>,
    default_workspace_root: Option<&str>,
) -> Option<(String, String, String)> {
    const FILE_EXTS: &[&str] = &[
        ".pdf", ".md", ".txt", ".docx", ".doc", ".csv",
        ".json", ".yaml", ".yml", ".rst", ".tex", ".log",
        ".py", ".rs", ".js", ".ts", ".swift",
    ];
    let ws_id = default_workspace_id.unwrap_or("").to_string();

    for token in task.split_whitespace() {
        let cleaned = token.trim_matches(|c: char| {
            matches!(c, '"' | '\'' | ',' | ';' | ')' | '(' | '[' | ']')
        });
        let lower = cleaned.to_lowercase();

        // Token ends with a known file extension
        if FILE_EXTS.iter().any(|ext| lower.ends_with(ext)) {
            if cleaned.starts_with('/') {
                let rel = cleaned.trim_start_matches('/').to_string();
                return Some((ws_id, "/".to_string(), rel));
            } else if let Some(root) = default_workspace_root {
                if !root.is_empty() {
                    return Some((ws_id, root.to_string(), cleaned.to_string()));
                }
            }
        }

        // Absolute path without a known extension but looks like a file
        // (contains a dot, does not end with '/', at least one '/' after the root)
        if cleaned.starts_with('/')
            && cleaned.contains('.')
            && !cleaned.ends_with('/')
            && !cleaned.ends_with('.')
            && cleaned.matches('/').count() >= 2
        {
            let rel = cleaned.trim_start_matches('/').to_string();
            return Some((ws_id, "/".to_string(), rel));
        }
    }

    None
}

/// Builds a two-step plan: first read the specific file, then ask the LLM to
/// summarise using only the real file content.
///
/// Step 1: `FsReadFile` at the resolved path.
/// Step 2: `LlmGenerate` with `{{FS_RESULT}}` in the prompt — `next_action`
///         substitutes the real file content before emitting the intent.
///
/// The LlmGenerate prompt explicitly forbids inventing content not present in
/// the tool output, preventing hallucination on file summaries.
fn build_plan_file_read_then_llm(
    task: &str,
    ws_id: String,
    ws_root: String,
    rel_path: String,
    strategy: Strategy,
) -> Vec<ToolCallIntent> {
    let llm_task = format!(
        "The user asked: \"{task}\"\n\n\
         Here is the actual file content returned by the filesystem tool:\n\
         {placeholder}\n\n\
         Using ONLY the file content above, provide a concise and accurate response. \
         Do NOT add, invent, or infer any information that is not present in the \
         tool result. If the content appears truncated or is unavailable, say so.",
        task = sanitize_task_for_llm(task),
        placeholder = FS_RESULT_PLACEHOLDER,
    );

    vec![
        ToolCallIntent::FsReadFile {
            workspace_id: ws_id,
            workspace_root: ws_root,
            rel_path,
        },
        ToolCallIntent::LlmGenerate {
            strategy,
            task: llm_task,
            system_hint: Some(AGENT_SYSTEM_HINT.to_string()),
        },
    ]
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
        Strategy::CheapModel if has_implicit_fs_intent(&task) => {
            // Two-step FS-first plan: list the real directory, then let the LLM
            // summarise using the actual listing (not invented content).
            match extract_fs_path(
                &task,
                default_workspace_id.as_deref(),
                default_workspace_root.as_deref(),
            ) {
                Some((ws_id, ws_root, rel_path)) => {
                    build_plan_fs_then_llm(&task, ws_id, ws_root, rel_path, Strategy::CheapModel)
                }
                None => {
                    // No resolvable path: guide the LLM to ask rather than invent.
                    vec![ToolCallIntent::LlmGenerate {
                        strategy: Strategy::CheapModel,
                        task: format!(
                            "The user asked: \"{task}\"\n\n\
                             You could not determine which folder or file they meant. \
                             Ask them to specify a full path (for example /Users/me/Desktop) \
                             rather than inventing or guessing a folder structure."
                        ),
                        system_hint: Some(AGENT_SYSTEM_HINT.to_string()),
                    }]
                }
            }
        }
        Strategy::CheapModel if has_implicit_file_read_intent(&task) => {
            // User wants to summarize or describe a specific file's content.
            // Sub-case A: explicit file path in task → read the file directly.
            // Sub-case B: no explicit path → list workspace so the model can identify
            //             the file and avoid fabricating its contents.
            if let Some((ws_id, ws_root, rel_path)) = extract_explicit_file_path(
                &task,
                default_workspace_id.as_deref(),
                default_workspace_root.as_deref(),
            ) {
                build_plan_file_read_then_llm(&task, ws_id, ws_root, rel_path, Strategy::CheapModel)
            } else {
                match extract_fs_path(
                    &task,
                    default_workspace_id.as_deref(),
                    default_workspace_root.as_deref(),
                ) {
                    Some((ws_id, ws_root, rel_path)) => {
                        // Reuse the existing FsListDir → LlmGenerate plan.
                        // The prompt already forbids fabricating content not in the tool result.
                        build_plan_fs_then_llm(&task, ws_id, ws_root, rel_path, Strategy::CheapModel)
                    }
                    None => vec![ToolCallIntent::LlmGenerate {
                        strategy: Strategy::CheapModel,
                        task: format!(
                            "The user asked: \"{task}\"\n\n\
                             You could not determine which file they meant. \
                             Ask them to specify the full path to the file rather than \
                             guessing or inventing its contents.",
                            task = sanitize_task_for_llm(&task),
                        ),
                        system_hint: Some(AGENT_SYSTEM_HINT.to_string()),
                    }],
                }
            }
        }
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
        let mut intent = session.plan[session.step_index].clone();

        // If the next intent is an LlmGenerate whose task contains the FS result
        // placeholder, substitute it with the real accumulated tool output before
        // emitting — this prevents the model from hallucinating filesystem content.
        if let ToolCallIntent::LlmGenerate { ref mut task, .. } = intent {
            if task.contains(FS_RESULT_PLACEHOLDER) {
                let fs_result = session
                    .accumulated_response
                    .as_deref()
                    .unwrap_or("(no tool result available)");
                *task = task.replace(FS_RESULT_PLACEHOLDER, fs_result);
            }
        }

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
    // ── Tracing: entry ────────────────────────────────────────────────────────
    let last_result_tag = match &last_result {
        None => "none",
        Some(StepResult::Ok { .. }) => "ok",
        Some(StepResult::Err { .. }) => "err",
        Some(StepResult::ApprovalPending { .. }) => "approval_pending",
    };
    agent_event!(DEBUG, session.session_id, "agent_step_enter",
        state = ?session.state,
        step_index = session.step_index,
        plan_len = session.plan.len(),
        last_result = last_result_tag,
    );
    // ─────────────────────────────────────────────────────────────────────────

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
        // ── Tracing: init outcome ─────────────────────────────────────────────
        let first_desc = first_intent.as_ref()
            .map(|i| intent_tool_name(i))
            .unwrap_or_else(|| "none".to_string());
        agent_event!(INFO, session.session_id, "agent_step_init",
            first_intent = %first_desc,
            plan_len = session.plan.len(),
        );
        // ─────────────────────────────────────────────────────────────────────
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
                agent_event!(INFO, session.session_id, "agent_step_done", outcome = "complete",);
                Ok(AgentStepOutcome::Complete(build_agent_task_result(session)))
            }
            OrchestratorAction::ToolCall { intent, session: s } => {
                *session = s;
                agent_event!(INFO, session.session_id, "agent_step_done",
                    outcome = "need_tool",
                    intent = %intent_tool_name(&intent),
                    step_index = session.step_index,
                    plan_len = session.plan.len(),
                );
                Ok(AgentStepOutcome::NeedTool {
                    intent,
                    session: session.clone(),
                })
            }
            OrchestratorAction::AwaitingApproval { request_id, session: s } => {
                *session = s;
                agent_event!(INFO, session.session_id, "agent_step_done",
                    outcome = "awaiting_approval",
                    request_id = %request_id,
                );
                Ok(AgentStepOutcome::AwaitingApproval {
                    request_id,
                    session: session.clone(),
                })
            }
        }
    } else {
        tracing::error!(
            session_id = %session.session_id,
            event = "agent_step_error",
            state = ?session.state,
            "last_result required when session already executing"
        );
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
            system_hint: Some(AGENT_SYSTEM_HINT.to_string()),
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

    /// Tools-only: "delete file X" -> orchestrator emits single FsDelete intent.
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

    /// Implicit FS request "summarize my desktop folder" -> two-step plan:
    /// [FsListDir(Desktop), LlmGenerate(with {{FS_RESULT}})]
    #[test]
    fn start_session_implicit_fs_produces_two_step_plan() {
        let input = RouterInput {
            task_description: "Please summarize my desktop folder".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(session.plan.len(), 2, "should have FsListDir + LlmGenerate");
        assert!(
            matches!(&session.plan[0], ToolCallIntent::FsListDir { .. }),
            "step 0 should be FsListDir"
        );
        assert!(
            matches!(&session.plan[1], ToolCallIntent::LlmGenerate { .. }),
            "step 1 should be LlmGenerate"
        );
        // LlmGenerate task must contain the placeholder (not yet substituted)
        if let ToolCallIntent::LlmGenerate { task, .. } = &session.plan[1] {
            assert!(
                task.contains(FS_RESULT_PLACEHOLDER),
                "LlmGenerate task must contain placeholder before substitution"
            );
        }
        // First emitted intent is FsListDir
        assert!(matches!(first, Some(ToolCallIntent::FsListDir { .. })));
    }

    /// After FsListDir returns real entries, next_action substitutes {{FS_RESULT}}
    /// in the LlmGenerate task before emitting it.
    #[test]
    fn next_action_substitutes_fs_result_placeholder() {
        let fs_step = ToolCallIntent::FsListDir {
            workspace_id: "ws1".to_string(),
            workspace_root: "/Users/test/Desktop".to_string(),
            rel_path: ".".to_string(),
        };
        let llm_task_template = format!(
            "The user asked: \"summarize my desktop\"\n\nTool result:\n{}\n\nSummarise.",
            FS_RESULT_PLACEHOLDER
        );
        let llm_step = ToolCallIntent::LlmGenerate {
            strategy: Strategy::CheapModel,
            task: llm_task_template,
            system_hint: None,
        };

        let mut session = SessionState::new("s1".to_string(), "summarize my desktop".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![fs_step, llm_step];
        session.step_index = 0;

        // Simulate FsListDir returning real entries
        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": "file1.txt\nphoto.png\nREADME.md" }),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::ToolCall { intent, .. } => match intent {
                ToolCallIntent::LlmGenerate { task, .. } => {
                    assert!(
                        !task.contains(FS_RESULT_PLACEHOLDER),
                        "placeholder must be substituted before emitting"
                    );
                    assert!(
                        task.contains("file1.txt"),
                        "real fs result must appear in the task"
                    );
                    assert!(task.contains("photo.png"));
                    assert!(task.contains("README.md"));
                }
                _ => panic!("expected LlmGenerate intent"),
            },
            _ => panic!("expected ToolCall, got Complete or AwaitingApproval"),
        }
    }

    /// "Summarize /tmp/paper.md" → plan must be [FsReadFile, LlmGenerate] (explicit path detected).
    #[test]
    fn start_session_file_read_with_explicit_path_builds_read_plan() {
        let input = RouterInput {
            task_description: "Summarize /tmp/paper.md".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp".to_string()),
        );
        assert_eq!(session.plan.len(), 2, "should have FsReadFile + LlmGenerate");
        assert!(
            matches!(&session.plan[0], ToolCallIntent::FsReadFile { .. }),
            "step 0 should be FsReadFile, got {:?}",
            &session.plan[0]
        );
        assert!(
            matches!(&session.plan[1], ToolCallIntent::LlmGenerate { .. }),
            "step 1 should be LlmGenerate"
        );
        // LlmGenerate task must contain the FS_RESULT placeholder (not yet substituted).
        if let ToolCallIntent::LlmGenerate { task, .. } = &session.plan[1] {
            assert!(
                task.contains(FS_RESULT_PLACEHOLDER),
                "LlmGenerate task must contain FS_RESULT placeholder before substitution"
            );
        }
        assert!(matches!(first, Some(ToolCallIntent::FsReadFile { .. })));
    }

    /// "Summarize the paper on climate change" (no explicit path) → plan starts with FsListDir.
    #[test]
    fn start_session_file_read_without_path_falls_back_to_list() {
        let input = RouterInput {
            task_description: "Summarize the paper on climate change".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert!(
            session.plan.len() >= 1,
            "plan must have at least one step"
        );
        assert!(
            matches!(&session.plan[0], ToolCallIntent::FsListDir { .. }),
            "step 0 should be FsListDir when no explicit path, got {:?}",
            &session.plan[0]
        );
        assert!(matches!(first, Some(ToolCallIntent::FsListDir { .. })));
    }

    /// When FsReadFile returns Err, the orchestrator must return "Error: ..." and NOT
    /// proceed to the LlmGenerate step — preventing a fabricated file summary.
    #[test]
    fn fs_read_file_error_returns_error_not_hallucination() {
        let mut session = SessionState::new("s1".to_string(), "Summarize paper.md".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![
            ToolCallIntent::FsReadFile {
                workspace_id: "ws1".to_string(),
                workspace_root: "/tmp".to_string(),
                rel_path: "paper.md".to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: Strategy::CheapModel,
                task: format!("Summarize: {}", FS_RESULT_PLACEHOLDER),
                system_hint: Some(AGENT_SYSTEM_HINT.to_string()),
            },
        ];
        session.step_index = 0;

        let action = next_action(
            session,
            Some(StepResult::Err {
                message: "No such file or directory: paper.md".to_string(),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::Complete { response, .. } => {
                assert!(
                    response.starts_with("Error:"),
                    "error result must begin with 'Error:' not a fabricated summary: {:?}",
                    response
                );
            }
            _ => panic!("expected Complete with error response"),
        }
    }

    /// State machine: tools-only delete — first step returns NeedTool(FsDelete), then Ok result -> Complete.
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
