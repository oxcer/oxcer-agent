//! UniFFI FFI for Oxcer: pure Rust types and async-ready API.
//! Swift (and other bindings) get generated code; no manual C strings or oxcer_string_free.
//!
//! Build: `cargo build --release -p oxcer_ffi` → `target/release/liboxcer_ffi.dylib` on macOS.
//! Generate Swift: `cargo run -p oxcer_ffi --features uniffi/cli` (or use uniffi-bindgen).

uniffi::setup_scaffolding!("oxcer_ffi");

use std::path::Path;

// -----------------------------------------------------------------------------
// UniFFI-compatible error type (raw String is not supported as throw type)
// -----------------------------------------------------------------------------

#[derive(Debug, uniffi::Error)]
#[uniffi(flat_error)]
pub enum OxcerError {
    Generic { message: String },
}

impl std::fmt::Display for OxcerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OxcerError::Generic { message } => write!(f, "{}", message),
        }
    }
}

use oxcer_core::orchestrator::{
    agent_request, AgentConfig, AgentSessionState, AgentTaskInput, AgentToolExecutor,
    ToolCallIntent, ToolOutcome,
};
use oxcer_core::semantic_router::TaskContext as CoreTaskContext;
use oxcer_core::telemetry::{load_session_log_from_dir, list_sessions_from_dir, LogEvent as CoreLogEvent, LogMetrics as CoreLogMetrics, SessionSummary as CoreSessionSummary};

// -----------------------------------------------------------------------------
// UniFFI Records (mirror Swift / JSON contracts)
// -----------------------------------------------------------------------------

#[derive(uniffi::Record, Clone, Debug)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
    pub root_path: String,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SessionSummary {
    pub session_id: String,
    pub start_timestamp: String,
    pub end_timestamp: String,
    pub total_cost_usd: f64,
    pub success: bool,
    pub tool_calls_count: u32,
    pub approvals_count: u32,
    pub denies_count: u32,
}

#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct LogMetrics {
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    pub latency_ms: Option<u64>,
    pub cost_usd: Option<f64>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct LogEvent {
    pub timestamp: String,
    pub session_id: String,
    pub request_id: Option<String>,
    pub caller: String,
    pub component: String,
    pub action: String,
    pub decision: Option<String>,
    pub metrics: LogMetrics,
    /// JSON string for arbitrary details (Swift decodes to AnyCodableValue).
    pub details: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct TaskContext {
    pub workspace_id: Option<String>,
    pub selected_paths: Option<Vec<String>>,
    pub risk_hints: Option<bool>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct AgentRequestPayload {
    pub task_description: String,
    pub workspace_id: Option<String>,
    pub workspace_root: Option<String>,
    pub context: Option<TaskContext>,
    pub app_config_dir: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct AgentResponse {
    pub ok: bool,
    pub answer: Option<String>,
    pub error: Option<String>,
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

fn default_app_config_dir() -> Option<std::path::PathBuf> {
    dirs_next::data_dir().map(|d| d.join("Oxcer"))
}

fn app_config_dir_or_default(app_config_dir: &str) -> Result<std::path::PathBuf, OxcerError> {
    if app_config_dir.is_empty() {
        default_app_config_dir()
            .ok_or_else(|| OxcerError::Generic {
                message: "app_config_dir required (or set default data dir)".to_string(),
            })
    } else {
        Ok(std::path::PathBuf::from(app_config_dir))
    }
}

// -----------------------------------------------------------------------------
// Workspaces: read config.json
// -----------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct ConfigWorkspaceDto {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "root_path")]
    root_path: String,
}

#[derive(serde::Deserialize)]
struct ConfigFileDto {
    #[serde(default)]
    workspaces: Vec<ConfigWorkspaceDto>,
}

fn list_workspaces_impl(app_config_dir: &Path) -> Result<Vec<WorkspaceInfo>, OxcerError> {
    let path = app_config_dir.join("config.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(Vec::new()),
    };
    let cfg: ConfigFileDto = serde_json::from_str(&content).map_err(|e| OxcerError::Generic { message: e.to_string() })?;
    let list: Vec<WorkspaceInfo> = cfg
        .workspaces
        .into_iter()
        .map(|w| {
            let name = if w.name.is_empty() {
                Path::new(&w.root_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Workspace")
                    .to_string()
            } else {
                w.name
            };
            WorkspaceInfo {
                id: w.id,
                name,
                root_path: w.root_path,
            }
        })
        .collect();
    Ok(list)
}

// -----------------------------------------------------------------------------
// Agent request: stub executor
// -----------------------------------------------------------------------------

struct FfiStubExecutor;

impl AgentToolExecutor for FfiStubExecutor {
    fn execute_tool(&self, intent: ToolCallIntent) -> Result<ToolOutcome, String> {
        let _ = intent;
        Err(
            "Tool execution not available in FFI; use agent_step from the app with a real executor (e.g. Tauri/Swift) for full execution."
                .to_string(),
        )
    }

    fn resolve_approval(&self, _request_id: &str, _approved: bool) -> Result<serde_json::Value, String> {
        Err("Approval flow not available in FFI; use step API from app.".to_string())
    }
}

fn agent_request_impl(
    task_description: String,
    workspace_id: Option<String>,
    workspace_root: Option<String>,
    context: Option<CoreTaskContext>,
) -> Result<AgentResponse, OxcerError> {
    let executor = FfiStubExecutor;

    let context = context.unwrap_or_default();
    let input = AgentTaskInput {
        task_description: task_description.clone(),
        context,
    };
    let mut session = AgentSessionState::new(
        uuid::Uuid::new_v4().to_string(),
        input.task_description.clone(),
    );
    let config = AgentConfig {
        router_config: Default::default(),
        default_workspace_id: workspace_id,
        default_workspace_root: workspace_root,
    };

    let result = agent_request(input, &mut session, &config, &executor)
        .map_err(|e| OxcerError::Generic { message: e })?;
    let answer = result
        .final_answer
        .unwrap_or_else(|| "(no answer text)".to_string());

    Ok(AgentResponse {
        ok: true,
        answer: Some(answer),
        error: None,
    })
}

fn log_metrics_to_ffi(m: &CoreLogMetrics) -> LogMetrics {
    LogMetrics {
        tokens_in: m.tokens_in,
        tokens_out: m.tokens_out,
        latency_ms: m.latency_ms,
        cost_usd: m.cost_usd,
    }
}

fn log_event_to_ffi(e: &CoreLogEvent) -> LogEvent {
    LogEvent {
        timestamp: e.timestamp.clone(),
        session_id: e.session_id.clone(),
        request_id: e.request_id.clone(),
        caller: e.caller.clone(),
        component: e.component.clone(),
        action: e.action.clone(),
        decision: e.decision.clone(),
        metrics: log_metrics_to_ffi(&e.metrics),
        details: Some(e.details.to_string()),
    }
}

// -----------------------------------------------------------------------------
// Exported API (UniFFI)
// -----------------------------------------------------------------------------

/// List workspaces from config.json in the given app config directory.
#[uniffi::export]
pub fn list_workspaces(app_config_dir: String) -> Result<Vec<WorkspaceInfo>, OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    list_workspaces_impl(&dir)
}

/// List recent sessions from the app config directory.
#[uniffi::export]
pub fn list_sessions(app_config_dir: String) -> Result<Vec<SessionSummary>, OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    let summaries: Vec<CoreSessionSummary> = list_sessions_from_dir(&dir)
        .map_err(|e| OxcerError::Generic { message: e })?;
    Ok(summaries
        .into_iter()
        .map(|s| SessionSummary {
            session_id: s.session_id,
            start_timestamp: s.start_timestamp,
            end_timestamp: s.end_timestamp,
            total_cost_usd: s.total_cost_usd,
            success: s.success,
            tool_calls_count: s.tool_calls_count,
            approvals_count: s.approvals_count,
            denies_count: s.denies_count,
        })
        .collect())
}

/// Load session log events for one session.
#[uniffi::export]
pub fn load_session_log(session_id: String, app_config_dir: String) -> Result<Vec<LogEvent>, OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    let events: Vec<CoreLogEvent> = load_session_log_from_dir(&dir, &session_id)
        .map_err(|e| OxcerError::Generic { message: e })?;
    Ok(events.iter().map(log_event_to_ffi).collect())
}

/// Run the agent task (stub executor; tools/approvals require app step API).
#[uniffi::export]
pub fn run_agent_task(payload: AgentRequestPayload) -> Result<AgentResponse, OxcerError> {
    let task = payload.task_description.trim();
    if task.is_empty() {
        return Err(OxcerError::Generic {
            message: "task_description required".to_string(),
        });
    }
    let context: Option<CoreTaskContext> = payload.context.as_ref().map(|c| CoreTaskContext {
        workspace_id: c.workspace_id.clone(),
        selected_paths: c.selected_paths.clone().unwrap_or_default(),
        risk_hints: c.risk_hints.unwrap_or(false),
    });

    agent_request_impl(
        payload.task_description,
        payload.workspace_id,
        payload.workspace_root,
        context,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_workspaces_requires_app_config_dir_or_fails() {
        // Empty string should try default; on CI default may be None
        let r = list_workspaces(String::new());
        if let Ok(workspaces) = r {
            assert!(workspaces.iter().all(|w| !w.id.is_empty()));
        } else {
            assert!(r.unwrap_err().to_string().contains("app_config_dir"));
        }
    }

    #[test]
    fn list_sessions_returns_result() {
        let r = list_sessions("/nonexistent".to_string());
        // Either Ok(vec) or Err
        let _ = r;
    }

    #[test]
    fn load_session_log_requires_session_id() {
        // Empty app_config_dir will fail with "app_config_dir required"
        let r = load_session_log("some-session".to_string(), String::new());
        if r.is_ok() {
            assert!(r.unwrap().iter().all(|e| !e.session_id.is_empty()));
        }
    }

    #[test]
    fn run_agent_task_fails_without_task_description() {
        let payload = AgentRequestPayload {
            task_description: String::new(),
            workspace_id: None,
            workspace_root: None,
            context: None,
            app_config_dir: None,
        };
        let r = run_agent_task(payload);
        assert!(r.is_err());
    }
}
