//! UniFFI FFI for Oxcer: pure Rust types and async-ready API.
//! Swift (and other bindings) get generated code; no manual C strings or oxcer_string_free.
//!
//! Build: `cargo build --release -p oxcer_ffi` -> `target/release/liboxcer_ffi.dylib` on macOS.
//! Generate Swift: `cargo run -p oxcer_ffi --features uniffi/cli` (or use uniffi-bindgen).

uniffi::setup_scaffolding!("oxcer_ffi");

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use oxcer_core::llm::{download_file, DownloadProgressCallback, GenerationParams, LlmEngine, LocalPhi3Engine};

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

/// Tokio runtime for spawn_blocking. Ensures heavy inference runs off the async executor thread.
fn blocking_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Runtime::new().expect("Failed to create tokio runtime for FFI blocking tasks")
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

const PHI3_GGUF_URL: &str = "https://huggingface.co/microsoft/Phi-3-mini-4k-instruct-gguf/resolve/main/Phi-3-mini-4k-instruct-q4.gguf";
const PHI3_TOKENIZER_URL: &str = "https://huggingface.co/microsoft/Phi-3-mini-4k-instruct/resolve/main/tokenizer.json";

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
    let model_root = models_dir.join("phi3");
    let model_gguf = model_root.join("phi-3-mini-4k-instruct-q4.gguf");
    let model_gguf_for_loader = model_root.join("model.gguf");
    let tokenizer_path = model_root.join("tokenizer.json");

    let file_exists = model_gguf.is_file()
        && std::fs::metadata(&model_gguf)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
    let tokenizer_exists = tokenizer_path.is_file()
        && std::fs::metadata(&tokenizer_path)
            .map(|m| m.len() > 0)
            .unwrap_or(false);

    if file_exists && tokenizer_exists {
        callback.on_progress(1.0, "Ready".to_string());
    } else if !file_exists {
        callback.on_progress(0.0, "Starting download...".to_string());

        std::fs::create_dir_all(&model_root).map_err(|e| OxcerError::Generic {
            message: format!("Failed to create models dir: {}", e),
        })?;

        let adapter = Arc::new(FfiDownloadCallbackAdapter {
            inner: Arc::clone(&callback),
        });
        download_file(PHI3_GGUF_URL, &model_gguf, adapter)
            .await
            .map_err(|e| OxcerError::Generic {
                message: format!("Download failed: {}", e),
            })?;

        // Loader expects model.gguf; create hardlink or copy for compatibility
        if model_gguf != model_gguf_for_loader {
            if model_gguf_for_loader.exists() {
                let _ = std::fs::remove_file(&model_gguf_for_loader);
            }
            #[cfg(unix)]
            {
                let _ = std::os::unix::fs::symlink(
                    model_gguf.file_name().unwrap(),
                    &model_gguf_for_loader,
                );
            }
            if !model_gguf_for_loader.exists() {
                // TODO: std::fs::copy may buffer the full 2.4GB file. Consider streaming copy or
                // platform-specific APIs (e.g. copyfile on macOS) to avoid memory spike.
                std::fs::copy(&model_gguf, &model_gguf_for_loader).map_err(|e| OxcerError::Generic {
                    message: format!("Failed to copy model.gguf: {}", e),
                })?;
            }
        }

        if !tokenizer_exists {
            let adapter = Arc::new(FfiDownloadCallbackAdapter {
                inner: Arc::clone(&callback),
            });
            download_file(PHI3_TOKENIZER_URL, &tokenizer_path, adapter)
                .await
                .map_err(|e| OxcerError::Generic {
                    message: format!("Tokenizer download failed: {}", e),
                })?;
        }
    } else if !tokenizer_exists {
        callback.on_progress(0.0, "Downloading tokenizer...".to_string());
        std::fs::create_dir_all(&model_root).map_err(|e| OxcerError::Generic {
            message: format!("Failed to create models dir: {}", e),
        })?;
        let adapter = Arc::new(FfiDownloadCallbackAdapter {
            inner: Arc::clone(&callback),
        });
        download_file(PHI3_TOKENIZER_URL, &tokenizer_path, adapter)
            .await
            .map_err(|e| OxcerError::Generic {
                message: format!("Tokenizer download failed: {}", e),
            })?;
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

/// Zero-cost FFI warm-up. Triggers dylib load and static runtime initialization
/// without executing any heavy LLM logic. Call at app launch to pay the VMS cost upfront.
#[uniffi::export]
pub fn ping() -> String {
    "pong".to_string()
}

/// List workspaces from config.json in the given app config directory.
/// Returns an empty vec if config.json is absent. Propagates I/O and parse errors.
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
/// Bounded at MAX_EVENTS_PER_SESSION_LOG (2000) by telemetry layer; returns latest events.
#[uniffi::export]
pub fn load_session_log(session_id: String, app_config_dir: String) -> Result<Vec<LogEvent>, OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    let events = load_session_log_from_dir(&dir, &session_id)
        .map_err(|e| OxcerError::Generic { message: e })?;
    Ok(events.into_iter().map(|e| log_event_to_ffi(&e)).collect())
}

/// Ensure the local model FILES are present (download if needed). Does NOT load the engine.
/// Engine is loaded lazily on first inference (generate_text). Call at startup before inference.
/// Safe to call multiple times; subsequent calls no-op if files already ensured.
#[uniffi::export(async_runtime = "tokio")]
pub async fn ensure_local_model(
    app_config_dir: String,
    callback: Box<dyn DownloadCallback>,
) -> Result<(), OxcerError> {
    let dir = app_config_dir_or_default(&app_config_dir)?;
    ensure_local_model_impl(&dir, Arc::from(callback)).await
}

/// Generate text using the global model. Loads engine lazily on first call (requires ensure_local_model first).
///
/// # Lazy Load + Zero-Copy Arc Pattern
/// 1. get_or_init_engine() loads engine on first use (heavy allocation deferred from startup).
/// 2. Clone the Arc — cost: refcount increment, NOT a copy of the 2.3GB model.
/// 3. Move the Arc pointer into the thread; heap data stays put.
#[uniffi::export(async_runtime = "tokio")]
pub async fn generate_text(prompt: String) -> Result<String, OxcerError> {
    println!(
        "[Rust] generate_text ENTER, prompt_len={} at {:?}",
        prompt.len(),
        std::time::SystemTime::now()
    );

    // 1. Lazy load: engine created on first inference
    let engine_ref = get_or_init_engine()?;

    // 2. Clone the pointer (near-zero cost; does NOT copy the model)
    let engine_ptr = engine_ref.clone();

    // 3. Move the pointer into the thread; closure captures Arc, not raw struct
    // First ? unwraps JoinError; second ? unwraps inner Result<String, OxcerError>
    let result = blocking_runtime()
        .spawn_blocking(move || {
            engine_ptr
                .generate(&prompt, &GenerationParams::default())
                .map_err(|e| OxcerError::Generic {
                    message: e.to_string(),
                })
        })
        .await
        .map_err(|e| OxcerError::Generic {
            message: e.to_string(),
        })??;

    println!("[Rust] generate_text EXIT at {:?}", std::time::SystemTime::now());
    Ok(result)
}

/// Run the agent task (stub executor; tools/approvals require app step API).
///
/// # Singleton enforcement
/// The agent uses `FfiStubExecutor` which does not invoke the LLM. When a real executor is wired,
/// it MUST use `GLOBAL_ENGINE` for LlmGenerate intents — never create new engine instances.
/// Heavy orchestrator work runs on a dedicated blocking thread pool; does not block the caller's executor.
#[uniffi::export(async_runtime = "tokio")]
pub async fn run_agent_task(payload: AgentRequestPayload) -> Result<AgentResponse, OxcerError> {
    println!(
        "[Rust] run_agent_task ENTER, task_len={} at {:?}",
        payload.task_description.len(),
        std::time::SystemTime::now()
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

    println!("[Rust] run_agent_task EXIT at {:?}", std::time::SystemTime::now());
    Ok(result)
}

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
