//! UniFFI FFI for Oxcer: pure Rust types and async-ready API.
//! Swift (and other bindings) get generated code; no manual C strings or oxcer_string_free.
//!
//! Build: `cargo build --release -p oxcer_ffi` -> `target/release/liboxcer_ffi.dylib` on macOS.
//! Generate Swift: `cargo run -p oxcer_ffi --features uniffi/cli` (or use uniffi-bindgen).

uniffi::setup_scaffolding!("oxcer_ffi");

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

// ── Structured logging ────────────────────────────────────────────────────────

/// Structured agent lifecycle event. Always includes `session_id` and `event` fields.
/// Trailing tokens after the event name are forwarded verbatim to `tracing::event!`,
/// so `%value` (Display) and `?value` (Debug) formatters work as expected.
///
/// Usage:
///   agent_event!(INFO, sid, "event_name")
///   agent_event!(INFO, sid, "event_name", key = val, key2 = %val2, key3 = ?val3,)
macro_rules! agent_event {
    ($level:ident, $sid:expr, $event:expr) => {
        ::tracing::event!(::tracing::Level::$level, session_id = %$sid, event = $event)
    };
    ($level:ident, $sid:expr, $event:expr, $($rest:tt)*) => {
        ::tracing::event!(::tracing::Level::$level, session_id = %$sid, event = $event, $($rest)*)
    };
}

/// One-time tracing subscriber initialisation.
/// Uses `OnceLock` so the dylib is safe to load by any host (Swift app, tests, Tauri).
/// `try_init()` silently no-ops if another subscriber was already registered.
fn ensure_logging_init() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let filter = std::env::var("OXCER_LOG").unwrap_or_else(|_| "info".to_string());
        let _ = tracing_subscriber::fmt()
            .json()
            .flatten_event(true) // merge fields to top-level → `jq '.session_id'` works
            .with_writer(std::io::stdout)
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .try_init();
        // Forward existing log::info! / log::warn! calls to the tracing pipeline.
        let _ = tracing_log::LogTracer::init();
    });
}

// ─────────────────────────────────────────────────────────────────────────────

use oxcer_core::llm::{
    download_file, CloudLlmEngine, DownloadProgressCallback, GenerationParams, LlmEngine,
    LocalPhi3Engine,
};

// -----------------------------------------------------------------------------
// Lazy model loading: Zero-Copy Arc pattern.
//
// LAZY_MODEL_ROOT: Set by ensure_local_model (files-only phase). Stores path for lazy init.
// GLOBAL_ENGINE: Populated lazily by get_or_init_engine() on first inference.
//
// INVARIANT: ensure_local_model_impl only ensures files exist; it does NOT load the engine.
// get_or_init_engine() loads LocalPhi3Engine on first use (generate_text, etc.).
// -----------------------------------------------------------------------------

/// Shared ownership of the engine. Clone = pointer copy, not data copy.
type SharedEngine = Arc<Box<dyn LlmEngine>>;

/// Resolved model root path. Set by ensure_local_model (setup phase). Read by get_or_init_engine (lazy load).
static LAZY_MODEL_ROOT: OnceLock<PathBuf> = OnceLock::new();
static GLOBAL_ENGINE: OnceLock<SharedEngine> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// Read-only access. Returns the Arc (clone = O(1) refcount bump). Cannot trigger a load.
#[inline(always)]
fn get_global_engine() -> Option<SharedEngine> {
    GLOBAL_ENGINE.get().cloned()
}

/// Lazy init: load engine on first use. Requires ensure_local_model to have run first.
fn get_or_init_engine() -> Result<SharedEngine, OxcerError> {
    if let Some(engine) = get_global_engine() {
        return Ok(engine);
    }
    let model_root = LAZY_MODEL_ROOT.get().ok_or_else(|| OxcerError::Generic {
        message: "Model files not ensured. Call ensure_local_model first.".to_string(),
    })?;
    let _guard = INIT_LOCK.lock().map_err(|e| OxcerError::Generic {
        message: format!("Init lock poisoned: {}", e),
    })?;
    if let Some(engine) = get_global_engine() {
        return Ok(engine);
    }
    let engine = LocalPhi3Engine::new(model_root).map_err(|e| OxcerError::Generic {
        message: e.to_string(),
    })?;
    let shared_engine: SharedEngine = Arc::new(Box::new(engine));
    let _ = GLOBAL_ENGINE.get_or_init(|| shared_engine.clone());
    Ok(shared_engine)
}

// -----------------------------------------------------------------------------
// Cloud engine slot: mutable, RwLock-protected.
//
// CLOUD_ENGINE_SLOT holds an optional CloudLlmEngine chosen by the user in
// Settings. When populated, get_active_engine() returns it in preference to the
// local GLOBAL_ENGINE. Swift calls activate_cloud_provider / deactivate_cloud_provider
// to control this slot; the FSM and tool layer are never aware of the switch.
// -----------------------------------------------------------------------------

/// Slot for the user-selected cloud engine. None = use local engine (default).
static CLOUD_ENGINE_SLOT: OnceLock<RwLock<Option<SharedEngine>>> = OnceLock::new();

/// Returns a reference to the RwLock (initialising it the first time).
fn cloud_engine_slot() -> &'static RwLock<Option<SharedEngine>> {
    CLOUD_ENGINE_SLOT.get_or_init(|| RwLock::new(None))
}

/// Return the active engine: cloud slot if populated, local engine otherwise.
/// Called by `generate_text`; never by the FSM.
fn get_active_engine() -> Result<SharedEngine, OxcerError> {
    // Check cloud slot first (read lock — cheap).
    {
        let slot = cloud_engine_slot()
            .read()
            .map_err(|e| OxcerError::Generic {
                message: format!("Cloud engine slot read lock poisoned: {}", e),
            })?;
        if let Some(engine) = slot.as_ref() {
            return Ok(engine.clone());
        }
    }
    // Fall back to the local Llama engine.
    get_or_init_engine()
}

/// Tokio runtime for spawn_blocking. Ensures heavy inference runs off the async executor thread.
fn blocking_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Runtime::new()
            .expect("Failed to create tokio runtime for FFI blocking tasks")
    })
}

// -----------------------------------------------------------------------------
// Download progress callback (Swift implements this; Rust calls it)
// -----------------------------------------------------------------------------

#[uniffi::export(callback_interface)]
pub trait DownloadCallback: Send + Sync {
    /// Progress from 0.0 to 1.0. Message is a user-friendly status.
    fn on_progress(&self, progress: f64, message: String);
}

/// Adapter: forwards to the UniFFI callback so oxcer-core's download_file can use it.
struct FfiDownloadCallbackAdapter {
    inner: Arc<dyn DownloadCallback>,
}

impl DownloadProgressCallback for FfiDownloadCallbackAdapter {
    fn on_progress(&self, progress: f64, message: String) {
        self.inner.on_progress(progress, message);
    }
}

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
use oxcer_core::telemetry::{
    list_sessions_from_dir, load_session_log_from_dir, LogEvent as CoreLogEvent,
    LogMetrics as CoreLogMetrics, SessionSummary as CoreSessionSummary,
};

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
        default_app_config_dir().ok_or_else(|| OxcerError::Generic {
            message: "app_config_dir required (or set default data dir)".to_string(),
        })
    } else {
        Ok(std::path::PathBuf::from(app_config_dir))
    }
}

/// Llama-3-8B-Instruct Q4_K_M (bartowski community quantization, ~4.9 GiB).
/// Requires approximately 6 GiB of unified memory for comfortable inference on Apple Silicon.
const LLAMA3_GGUF_URL: &str = "https://huggingface.co/bartowski/Meta-Llama-3-8B-Instruct-GGUF/resolve/main/Meta-Llama-3-8B-Instruct-Q4_K_M.gguf";

/// Ensure the local model FILES are present (download if needed). Does NOT load the engine.
/// Engine is loaded lazily on first inference via get_or_init_engine().
async fn ensure_local_model_impl(
    app_config_dir: &Path,
    callback: Arc<dyn DownloadCallback>,
) -> Result<(), OxcerError> {
    // 1. FAST PATH: If model root already stored (files ensured), return immediately.
    if LAZY_MODEL_ROOT.get().is_some() || GLOBAL_ENGINE.get().is_some() {
        callback.on_progress(1.0, "Ready".to_string());
        return Ok(());
    }

    let config_dir = app_config_dir.to_path_buf();
    let models_dir = config_dir.join("models");
    let model_root = models_dir.join("llama3");
    // Download target — the file as received from the CDN.
    let model_gguf = model_root.join("Meta-Llama-3-8B-Instruct-Q4_K_M.gguf");
    // Loader expects `model.gguf` in the model root; we symlink the downloaded file there.
    let model_gguf_for_loader = model_root.join("model.gguf");

    let file_exists = model_gguf.is_file()
        && std::fs::metadata(&model_gguf)
            .map(|m| m.len() > 0)
            .unwrap_or(false);

    if file_exists {
        callback.on_progress(1.0, "Ready".to_string());
    } else {
        callback.on_progress(0.0, "Starting Llama-3 download (~4.9 GiB)...".to_string());

        std::fs::create_dir_all(&model_root).map_err(|e| OxcerError::Generic {
            message: format!("Failed to create models dir: {}", e),
        })?;

        let adapter = Arc::new(FfiDownloadCallbackAdapter {
            inner: Arc::clone(&callback),
        });
        download_file(LLAMA3_GGUF_URL, &model_gguf, adapter)
            .await
            .map_err(|e| OxcerError::Generic {
                message: format!("Download failed: {}", e),
            })?;

        // Create a `model.gguf` symlink so the loader finds the expected filename.
        if model_gguf_for_loader.exists() || model_gguf_for_loader.is_symlink() {
            let _ = std::fs::remove_file(&model_gguf_for_loader);
        }
        #[cfg(unix)]
        {
            let _ =
                std::os::unix::fs::symlink(model_gguf.file_name().unwrap(), &model_gguf_for_loader);
        }
        // Fallback: copy if symlink creation failed (e.g. on non-Unix targets).
        if !model_gguf_for_loader.exists() {
            std::fs::copy(&model_gguf, &model_gguf_for_loader).map_err(|e| {
                OxcerError::Generic {
                    message: format!("Failed to link model.gguf: {}", e),
                }
            })?;
        }
    }

    callback.on_progress(1.0, "Model Ready!".to_string());

    // 2. LAZY LOAD: Store path only. Engine is loaded on first inference via get_or_init_engine().
    let _ = LAZY_MODEL_ROOT.set(model_root);

    Ok(())
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
    let cfg: ConfigFileDto = serde_json::from_str(&content).map_err(|e| OxcerError::Generic {
        message: format!("list_workspaces: parse config.json: {}", e),
    })?;
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

    fn resolve_approval(
        &self,
        _request_id: &str,
        _approved: bool,
    ) -> Result<serde_json::Value, String> {
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
        ..AgentConfig::default()
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

#[uniffi::export]
pub fn ping() -> String {
    "pong".to_string()
}

#[uniffi::export]
pub fn list_workspaces(app_config_dir: String) -> Result<Vec<WorkspaceInfo>, OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    list_workspaces_impl(&dir)
}

#[uniffi::export]
pub fn list_sessions(app_config_dir: String) -> Result<Vec<SessionSummary>, OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    let summaries: Vec<CoreSessionSummary> =
        list_sessions_from_dir(&dir).map_err(|e| OxcerError::Generic { message: e })?;
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

#[uniffi::export]
pub fn load_session_log(
    session_id: String,
    app_config_dir: String,
) -> Result<Vec<LogEvent>, OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    let events = load_session_log_from_dir(&dir, &session_id)
        .map_err(|e| OxcerError::Generic { message: e })?;
    Ok(events.into_iter().map(|e| log_event_to_ffi(&e)).collect())
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn ensure_local_model(
    app_config_dir: String,
    callback: Box<dyn DownloadCallback>,
) -> Result<(), OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    ensure_local_model_impl(&dir, Arc::from(callback)).await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn generate_text(prompt: String) -> Result<String, OxcerError> {
    ensure_logging_init();
    let t0_gt = std::time::Instant::now();
    tracing::info!(
        event = "generate_text_enter",
        prompt_len = prompt.len(),
        "generate_text start"
    );

    // 1. Route to the active engine: cloud slot (if activated) or local Llama.
    // INVARIANT: Only generate_text routes through get_active_engine().
    // The FSM, tool layer, and approval gates are never aware of which engine is active.
    let engine_ref = get_active_engine()?;

    // 2. Clone the pointer (near-zero cost; does NOT copy the model)
    let engine_ptr = engine_ref.clone();

    // 3. Move the pointer into the thread; closure captures Arc, not raw struct
    // First ? unwraps JoinError; second ? unwraps inner Result<String, OxcerError>
    let prompt_len = prompt.len();
    let result = blocking_runtime()
        .spawn_blocking(move || {
            engine_ptr
                .generate(&prompt, &GenerationParams::default())
                .map_err(|e| {
                    tracing::error!(
                        event = "generate_text_error",
                        prompt_len = prompt_len,
                        err = %e,
                        "generate_text failed"
                    );
                    OxcerError::Generic {
                        message: e.to_string(),
                    }
                })
        })
        .await
        .map_err(|e| OxcerError::Generic {
            message: e.to_string(),
        })??;

    tracing::info!(
        event = "generate_text_done",
        text_len = result.len(),
        elapsed_ms = t0_gt.elapsed().as_millis() as f64,
        "generate_text done"
    );
    Ok(result)
}

// -----------------------------------------------------------------------------
// Step-based FFI API (Swift-driven execution loop)
// -----------------------------------------------------------------------------

/// Opaque JSON blob of the orchestrator session state.
/// Swift must not modify this. Pass it back unchanged on every call.
#[derive(uniffi::Record, Clone, Debug)]
pub struct FfiSessionState {
    pub session_json: String,
}

/// One tool intent emitted by the orchestrator, serialized for Swift.
#[derive(uniffi::Record, Clone, Debug)]
pub struct FfiToolIntent {
    /// Discriminant: "llm_generate" | "fs_list_dir" | "fs_read_file" |
    /// "fs_write_file" | "fs_delete" | "fs_rename" | "fs_move" | "shell_run"
    pub kind: String,
    /// Full intent as JSON so Swift can decode all fields.
    pub intent_json: String,
}

/// Result of one tool execution from the Swift executor back to Rust.
#[derive(uniffi::Record, Clone, Debug)]
pub struct FfiStepResult {
    pub ok: bool,
    /// JSON payload when ok == true. For LlmGenerate use {"text": "..."}.
    pub payload_json: Option<String>,
    /// Error message when ok == false.
    pub error: Option<String>,
}

/// Outcome of one orchestrator step.
#[derive(uniffi::Record, Clone, Debug)]
pub struct FfiStepOutcome {
    /// Discriminant: "need_tool" | "complete" | "awaiting_approval"
    pub status: String,
    /// Present when status == "need_tool".
    pub intent: Option<FfiToolIntent>,
    /// Present when status == "awaiting_approval".
    pub approval_request_id: Option<String>,
    /// Present when status == "complete".
    pub final_answer: Option<String>,
    /// Updated session state. Always present. Pass back unchanged on the next call.
    pub session: FfiSessionState,
}

#[uniffi::export]
pub fn ffi_agent_step(
    task_description: String,
    workspace_id: Option<String>,
    workspace_root: Option<String>,
    app_config_dir: Option<String>,
    session_json: Option<String>,
    last_result: Option<FfiStepResult>,
) -> Result<FfiStepOutcome, OxcerError> {
    use oxcer_core::orchestrator::{
        AgentConfig, AgentSessionState, AgentStepOutcome, AgentTaskInput, StepResult,
        ToolCallIntent,
    };
    use oxcer_core::semantic_router::{RouterConfig, TaskContext};

    // ── Tracing: entry ────────────────────────────────────────────────────────
    ensure_logging_init();
    let t0 = std::time::Instant::now();
    let is_new = session_json.is_none();
    let last_result_desc = match &last_result {
        None => "none".to_string(),
        Some(r) if r.ok => format!(
            "ok({}bytes)",
            r.payload_json.as_deref().map(|s| s.len()).unwrap_or(0)
        ),
        Some(r) => format!(
            "err({})",
            r.error
                .as_deref()
                .unwrap_or("unknown")
                .chars()
                .take(80)
                .collect::<String>()
        ),
    };
    // ─────────────────────────────────────────────────────────────────────────

    let mut session: AgentSessionState = if let Some(ref json) = session_json {
        serde_json::from_str(json).map_err(|e| OxcerError::Generic {
            message: format!("ffi_agent_step: invalid session_json: {}", e),
        })?
    } else {
        AgentSessionState::new(uuid::Uuid::new_v4().to_string(), task_description.clone())
    };

    // Log ENTER now that session.session_id is available.
    agent_event!(INFO, session.session_id, "ffi_agent_step_enter",
        kind = if is_new { "new_session" } else { "continue_session" },
        last_result = %last_result_desc,
        task = %task_description.chars().take(60).collect::<String>(),
    );

    let config = AgentConfig {
        router_config: RouterConfig::default(),
        default_workspace_id: workspace_id,
        default_workspace_root: workspace_root,
        ..AgentConfig::default()
    };
    let _ = app_config_dir; // reserved for future use (e.g. telemetry path)

    let input = AgentTaskInput {
        task_description,
        context: TaskContext::default(),
    };

    let step_result: Option<StepResult> = last_result.map(|r| {
        if r.ok {
            let payload: serde_json::Value = r
                .payload_json
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(serde_json::Value::Null);
            StepResult::Ok { payload }
        } else {
            StepResult::Err {
                message: r.error.unwrap_or_else(|| "unknown error".to_string()),
            }
        }
    });

    // Helper used in both the DONE log and the ffi_outcome match below.
    fn intent_kind(i: &ToolCallIntent) -> &'static str {
        match i {
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
    }

    let outcome = oxcer_core::orchestrator::agent_step(input, &mut session, &config, step_result)
        .map_err(|e| {
        tracing::error!(
            session_id = %session.session_id,
            event = "ffi_agent_step_error",
            elapsed_ms = t0.elapsed().as_millis() as f64,
            err = %e,
            "ffi_agent_step failed"
        );
        OxcerError::Generic { message: e }
    })?;

    // ── Tracing: outcome ──────────────────────────────────────────────────────
    let outcome_kind = match &outcome {
        AgentStepOutcome::NeedTool { intent, .. } => {
            format!("need_tool({})", intent_kind(intent))
        }
        AgentStepOutcome::AwaitingApproval { request_id, .. } => {
            format!("awaiting_approval({})", request_id)
        }
        AgentStepOutcome::Complete(r) => {
            format!(
                "complete(answer_len={})",
                r.final_answer.as_deref().unwrap_or("").len()
            )
        }
    };
    agent_event!(INFO, session.session_id, "ffi_agent_step_done",
        outcome = %outcome_kind,
        step_index = session.step_index,
        plan_len = session.plan.len(),
        elapsed_ms = t0.elapsed().as_millis() as f64,
        state = ?session.state,
    );
    // ─────────────────────────────────────────────────────────────────────────

    let session_json_out = serde_json::to_string(&session).map_err(|e| OxcerError::Generic {
        message: format!("ffi_agent_step: session serialization failed: {}", e),
    })?;
    let ffi_session = FfiSessionState {
        session_json: session_json_out,
    };

    let ffi_outcome = match outcome {
        AgentStepOutcome::NeedTool { intent, .. } => {
            let kind = intent_kind(&intent).to_string();
            let intent_json = serde_json::to_string(&intent).map_err(|e| OxcerError::Generic {
                message: format!("ffi_agent_step: intent serialization failed: {}", e),
            })?;
            FfiStepOutcome {
                status: "need_tool".to_string(),
                intent: Some(FfiToolIntent { kind, intent_json }),
                approval_request_id: None,
                final_answer: None,
                session: ffi_session,
            }
        }
        AgentStepOutcome::AwaitingApproval { request_id, .. } => FfiStepOutcome {
            status: "awaiting_approval".to_string(),
            intent: None,
            approval_request_id: Some(request_id),
            final_answer: None,
            session: ffi_session,
        },
        AgentStepOutcome::Complete(result) => FfiStepOutcome {
            status: "complete".to_string(),
            intent: None,
            approval_request_id: None,
            final_answer: result.final_answer,
            session: ffi_session,
        },
    };

    Ok(ffi_outcome)
}

// -----------------------------------------------------------------------------
// TerminalExecutor — ReAct-style deterministic ls tool
// -----------------------------------------------------------------------------

#[uniffi::export]
pub fn ffi_terminal_execute(
    llm_output: String,
    working_dir: Option<String>,
) -> Result<String, OxcerError> {
    ensure_logging_init();
    tracing::debug!(
        event = "ffi_terminal_execute",
        "ffi_terminal_execute called"
    );

    oxcer_core::terminal::TerminalExecutor::execute_llm_action(&llm_output, working_dir.as_deref())
        .map_err(|e| OxcerError::Generic {
            message: e.to_string(),
        })
}

// -----------------------------------------------------------------------------
// MCP Tool Suite — workspace-scoped Claude-style file tools
// -----------------------------------------------------------------------------

#[uniffi::export]
pub fn ffi_mcp_execute(workspace_root: String, tool_json: String) -> String {
    ensure_logging_init();
    tracing::debug!(event = "ffi_mcp_execute", workspace = %workspace_root);
    let executor = oxcer_core::mcp::McpExecutor::new(workspace_root);
    executor.execute_json(&tool_json)
}

// -----------------------------------------------------------------------------
// Sub-Agent Orchestrator — Explore → Plan → Execute pipeline
// -----------------------------------------------------------------------------

#[uniffi::export(async_runtime = "tokio")]
pub async fn ffi_orchestrate(
    query: String,
    workspace_root: Option<String>,
    memory_path: Option<String>,
) -> Result<String, OxcerError> {
    ensure_logging_init();
    let root = workspace_root.unwrap_or_else(|| ".".to_string());
    let mem_path = memory_path.unwrap_or_else(|| {
        // Default: ~/.config/oxcer/memory.md
        dirs_next::config_dir()
            .map(|p| {
                p.join("oxcer")
                    .join("memory.md")
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|| "/dev/null".to_string())
    });

    tracing::info!(
        event = "ffi_orchestrate_enter",
        query_len = query.len(),
        workspace = %root,
    );

    // Try to get the LLM engine. Failure is non-fatal: we run tool phases only.
    let engine_opt = get_or_init_engine().ok();

    let result = blocking_runtime()
        .spawn_blocking(move || {
            if let Some(engine) = engine_opt {
                // Wrap the engine so it satisfies LlmCallback.
                struct EngineLlm {
                    engine: SharedEngine,
                }
                impl oxcer_core::subagent::LlmCallback for EngineLlm {
                    fn generate(&self, prompt: &str) -> String {
                        use oxcer_core::llm::GenerationParams;
                        self.engine
                            .generate(prompt, &GenerationParams::default())
                            .unwrap_or_else(|e| format!("[LLM_ERROR: {}]", e))
                    }
                }
                oxcer_core::subagent::orchestrate(
                    &query,
                    &root,
                    &mem_path,
                    Some(&EngineLlm { engine }),
                )
            } else {
                // No engine — tool phases only.
                oxcer_core::subagent::orchestrate(&query, &root, &mem_path, None)
            }
        })
        .await
        .map_err(|e| OxcerError::Generic {
            message: format!("orchestrate join error: {}", e),
        })?;

    tracing::info!(event = "ffi_orchestrate_done", answer_len = result.len());
    Ok(result)
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn orchestrate_query(
    query: String,
    workspace_root: Option<String>,
    app_config_dir: Option<String>,
) -> Result<String, OxcerError> {
    ensure_logging_init();

    let root = workspace_root.unwrap_or_else(|| {
        dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .to_string_lossy()
            .to_string()
    });

    let db_dir = app_config_dir.unwrap_or_else(|| root.clone());

    tracing::info!(
        event = "orchestrate_query_enter",
        query_len = query.len(),
        workspace = %root,
    );

    let engine_opt = get_or_init_engine().ok();

    let result = blocking_runtime()
        .spawn_blocking(move || -> Result<String, OxcerError> {
            use oxcer_core::db::StateDb;
            use oxcer_core::executor::UniversalExecutor;
            use oxcer_core::fsm::{AgentFsm, LlmCallback};

            let executor = UniversalExecutor::new(&root).map_err(|e| OxcerError::Generic {
                message: format!("executor init failed: {e}"),
            })?;

            // Open persistent DB in app_config_dir, or fall back to in-memory.
            let db_path = std::path::Path::new(&db_dir).join("oxcer_episodic.db");
            let db = StateDb::open(&db_path)
                .unwrap_or_else(|_| StateDb::open_in_memory().expect("in-memory DB never fails"));

            let fsm = AgentFsm::new(executor, db, 8);

            if let Some(engine) = engine_opt {
                struct EngineLlm {
                    engine: SharedEngine,
                }
                impl LlmCallback for EngineLlm {
                    fn generate(&self, prompt: &str) -> String {
                        self.engine
                            .generate(prompt, &GenerationParams::default())
                            .unwrap_or_else(|e| format!("[ERROR: {e}]"))
                    }
                }
                let llm = EngineLlm { engine };
                fsm.run(&query, &llm).map_err(|e| OxcerError::Generic {
                    message: format!("fsm error: {e}"),
                })
            } else {
                // No engine loaded — run the FSM with a stub that always returns [NO_TOOL].
                struct NoOpLlm;
                impl LlmCallback for NoOpLlm {
                    fn generate(&self, _prompt: &str) -> String {
                        "[NO_TOOL]".to_string()
                    }
                }
                // With NoOpLlm ActionSelection always returns NoTool → Finalize is called next.
                // Finalize then receives "[NO_TOOL]" as its answer which passes validate_final_answer.
                // Return a degraded but informative message.
                let _ = fsm.run(&query, &NoOpLlm);
                Ok(format!(
                    "[LLM_NOT_LOADED] Query received: {query}. \
                     Load a model via ensure_local_model to enable full agent responses."
                ))
            }
        })
        .await
        .map_err(|e| OxcerError::Generic {
            message: format!("orchestrate_query join error: {e}"),
        })??;

    tracing::info!(event = "orchestrate_query_done", answer_len = result.len());
    Ok(result)
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn run_agent_task(payload: AgentRequestPayload) -> Result<AgentResponse, OxcerError> {
    ensure_logging_init();
    tracing::info!(
        event = "run_agent_task_enter",
        task_len = payload.task_description.len(),
        "run_agent_task start"
    );

    let task = payload.task_description.trim();
    if task.is_empty() {
        return Err(OxcerError::Generic {
            message: "task_description required".to_string(),
        });
    }
    let task_description = payload.task_description;
    let workspace_id = payload.workspace_id;
    let workspace_root = payload.workspace_root;
    let context: Option<CoreTaskContext> = payload.context.as_ref().map(|c| CoreTaskContext {
        workspace_id: c.workspace_id.clone(),
        selected_paths: c.selected_paths.clone().unwrap_or_default(),
        risk_hints: c.risk_hints.unwrap_or(false),
    });

    // First ? unwraps JoinError; second ? unwraps inner Result<AgentResponse, OxcerError>
    let result = blocking_runtime()
        .spawn_blocking(move || {
            agent_request_impl(task_description, workspace_id, workspace_root, context)
        })
        .await
        .map_err(|e| OxcerError::Generic {
            message: e.to_string(),
        })??;

    tracing::info!(event = "run_agent_task_done", "run_agent_task done");
    Ok(result)
}

// =============================================================================
// Cloud Provider Settings — test connection
// =============================================================================

use oxcer_core::cloud_provider::{test_provider_connection, ProviderKind as CoreProviderKind};

/// Mirrors `oxcer_core::cloud_provider::ProviderKind` for the Swift boundary.
/// Swift receives this as a generated enum (camelCase variants via UniFFI).
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum FfiProviderKind {
    /// On-device Meta Llama 3 via llama.cpp + Metal. No API key required.
    LocalLlama,
    /// OpenAI ChatGPT (`gpt-4o-mini` default).
    OpenAi,
    /// Anthropic Claude (`claude-3-5-haiku-20241022` default).
    Anthropic,
    /// Google Gemini (`gemini-2.0-flash` default).
    Gemini,
    /// xAI Grok (`grok-2-1212` default).
    Grok,
}

impl From<FfiProviderKind> for CoreProviderKind {
    fn from(ffi: FfiProviderKind) -> Self {
        match ffi {
            FfiProviderKind::LocalLlama => CoreProviderKind::LocalLlama,
            FfiProviderKind::OpenAi => CoreProviderKind::OpenAI,
            FfiProviderKind::Anthropic => CoreProviderKind::Anthropic,
            FfiProviderKind::Gemini => CoreProviderKind::Gemini,
            FfiProviderKind::Grok => CoreProviderKind::Grok,
        }
    }
}

impl From<CoreProviderKind> for FfiProviderKind {
    fn from(core: CoreProviderKind) -> Self {
        match core {
            CoreProviderKind::LocalLlama => FfiProviderKind::LocalLlama,
            CoreProviderKind::OpenAI => FfiProviderKind::OpenAi,
            CoreProviderKind::Anthropic => FfiProviderKind::Anthropic,
            CoreProviderKind::Gemini => FfiProviderKind::Gemini,
            CoreProviderKind::Grok => FfiProviderKind::Grok,
        }
    }
}

/// Result returned to Swift after a `test_cloud_provider` call.
#[derive(uniffi::Record, Clone, Debug)]
pub struct FfiProviderTestResult {
    /// `true` if the provider returned a successful response.
    pub ok: bool,
    /// Echoes the provider under test.
    pub provider: FfiProviderKind,
    /// Human-readable message: confirmation on success, error description on failure.
    /// Always non-empty; safe to display directly in the UI.
    pub message: String,
}

/// Test connectivity and API-key validity for a cloud provider.
///
/// Performs the cheapest valid request for the given provider (max_tokens=1 where supported)
/// and maps the response to a user-friendly `FfiProviderTestResult`.
///
/// This function never throws — all errors are encoded in `FfiProviderTestResult.ok = false`.
/// Swift callers do not need a `try`.
#[uniffi::export(async_runtime = "tokio")]
pub async fn test_cloud_provider(
    provider: FfiProviderKind,
    api_key: String,
) -> FfiProviderTestResult {
    ensure_logging_init();
    let core_provider = CoreProviderKind::from(provider.clone());
    tracing::info!(
        event = "test_cloud_provider_enter",
        provider = ?core_provider,
    );

    let result = test_provider_connection(core_provider, &api_key).await;

    tracing::info!(
        event = "test_cloud_provider_done",
        ok = result.ok,
        provider = ?result.provider,
    );

    FfiProviderTestResult {
        ok: result.ok,
        provider: FfiProviderKind::from(result.provider),
        message: result.message,
    }
}

// =============================================================================
// Cloud engine DI — activate / deactivate
// =============================================================================

/// Activate a cloud LLM provider for the current session.
///
/// After this call, `generate_text` routes all inference requests to the chosen
/// provider. The FSM, tool layer, and approval gates are unaffected — only the
/// `LlmEngine` implementation changes.
///
/// Call this:
/// - After a successful `test_cloud_provider` confirms the key is valid.
/// - On app launch when `useCloudModel == true` and a saved key exists.
/// - When the user changes the provider while `useCloudModel` is on.
#[uniffi::export]
pub fn activate_cloud_provider(
    provider: FfiProviderKind,
    api_key: String,
) -> Result<(), OxcerError> {
    ensure_logging_init();
    let core_provider = CoreProviderKind::from(provider.clone());
    tracing::info!(
        event = "activate_cloud_provider",
        provider = ?core_provider,
    );

    let engine = CloudLlmEngine::new(core_provider, api_key);
    let shared: SharedEngine = Arc::new(Box::new(engine));

    let mut slot = cloud_engine_slot()
        .write()
        .map_err(|e| OxcerError::Generic {
            message: format!("Cloud engine slot write lock poisoned: {}", e),
        })?;
    *slot = Some(shared);
    Ok(())
}

/// Deactivate the cloud LLM provider.
///
/// After this call, `generate_text` falls back to the local Llama engine.
/// This is a no-op if no cloud engine is currently active.
#[uniffi::export]
pub fn deactivate_cloud_provider() {
    ensure_logging_init();
    tracing::info!(event = "deactivate_cloud_provider");
    if let Ok(mut slot) = cloud_engine_slot().write() {
        *slot = None;
    }
}

// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // list_workspaces (exported) — Stage 4: live implementation via config.json
    // -------------------------------------------------------------------------

    #[test]
    fn list_workspaces_empty_dir_returns_empty_vec() {
        let dir = tempfile::tempdir().unwrap();
        let result = list_workspaces(dir.path().display().to_string()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_workspaces_reads_config_json() {
        let dir = tempfile::tempdir().unwrap();
        let config = r#"{"workspaces":[{"id":"ws-1","name":"Alpha","root_path":"/tmp/alpha"}]}"#;
        std::fs::write(dir.path().join("config.json"), config).unwrap();
        let result = list_workspaces(dir.path().display().to_string()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "ws-1");
        assert_eq!(result[0].name, "Alpha");
        assert_eq!(result[0].root_path, "/tmp/alpha");
    }

    // -------------------------------------------------------------------------
    // list_workspaces_impl — lower-level unit tests (called by the exported fn)
    // -------------------------------------------------------------------------

    #[test]
    fn list_workspaces_impl_reads_config_json() {
        let dir = tempfile::tempdir().unwrap();
        let config = r#"{"workspaces":[{"id":"ws-1","name":"Alpha","root_path":"/tmp/alpha"}]}"#;
        std::fs::write(dir.path().join("config.json"), config).unwrap();

        let result = list_workspaces_impl(dir.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "ws-1");
        assert_eq!(result[0].name, "Alpha");
        assert_eq!(result[0].root_path, "/tmp/alpha");
    }

    #[test]
    fn list_workspaces_impl_missing_config_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let result = list_workspaces_impl(dir.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_workspaces_impl_multiple_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let config = r#"{"workspaces":[
            {"id":"ws-1","name":"Alpha","root_path":"/tmp/alpha"},
            {"id":"ws-2","name":"Beta","root_path":"/tmp/beta"}
        ]}"#;
        std::fs::write(dir.path().join("config.json"), config).unwrap();

        let result = list_workspaces_impl(dir.path()).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[1].id, "ws-2");
    }

    #[test]
    fn list_workspaces_impl_malformed_json_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.json"), b"not json").unwrap();
        let result = list_workspaces_impl(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("parse config.json"));
    }

    #[test]
    fn list_workspaces_impl_entry_missing_root_path_fails_parse() {
        // ConfigWorkspaceDto.root_path has no serde(default), so a missing field
        // fails the entire deserialization (the error surfaces as OxcerError).
        let dir = tempfile::tempdir().unwrap();
        let config = r#"{"workspaces":[{"id":"ws-no-path"}]}"#;
        std::fs::write(dir.path().join("config.json"), config).unwrap();
        let result = list_workspaces_impl(dir.path());
        assert!(result.is_err(), "expected Err for missing root_path field");
    }

    // -------------------------------------------------------------------------
    // Other existing FFI contract tests
    // -------------------------------------------------------------------------

    #[test]
    fn list_sessions_returns_result() {
        let r = list_sessions("/nonexistent".to_string());
        let _ = r; // Either Ok([]) or Err — both are valid.
    }

    #[test]
    fn load_session_log_requires_session_id() {
        let r = load_session_log("some-session".to_string(), String::new());
        if r.is_ok() {
            assert!(r.unwrap().iter().all(|e| !e.session_id.is_empty()));
        }
    }

    #[tokio::test]
    async fn run_agent_task_fails_without_task_description() {
        let payload = AgentRequestPayload {
            task_description: String::new(),
            workspace_id: None,
            workspace_root: None,
            context: None,
            app_config_dir: None,
        };
        let r = run_agent_task(payload).await;
        assert!(r.is_err());
    }
}
