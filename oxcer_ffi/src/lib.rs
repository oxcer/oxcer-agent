//! C FFI for Oxcer: UTF-8 JSON in/out. Swift (or other C callers) pass JSON strings and receive
//! allocated UTF-8 strings; call `oxcer_string_free` to free returned pointers.
//!
//! Build: `cargo build --release -p oxcer_ffi` → `target/release/liboxcer_ffi.dylib` on macOS.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;

use oxcer_core::orchestrator::{
    agent_request, AgentConfig, AgentSessionState, AgentTaskInput, AgentToolExecutor,
    ToolCallIntent, ToolOutcome,
};
use oxcer_core::semantic_router::TaskContext;
use oxcer_core::telemetry::{load_session_log_from_dir, list_sessions_from_dir};

// -----------------------------------------------------------------------------
// Helpers: C string ↔ Rust
// -----------------------------------------------------------------------------

/// Safe conversion: null or invalid UTF-8 → None; otherwise Some(s).
fn ptr_to_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr).to_str().ok().map(String::from) }
}

/// Allocate a C string from Rust; caller must call oxcer_string_free.
fn return_string(s: String) -> *const c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => std::ptr::null(),
    }
}

/// Default app config dir (e.g. macOS: ~/Library/Application Support/Oxcer).
fn default_app_config_dir() -> Option<std::path::PathBuf> {
    dirs_next::data_dir().map(|d| d.join("Oxcer"))
}

fn app_config_dir_from_json(input: &serde_json::Value) -> Option<std::path::PathBuf> {
    input
        .get("app_config_dir")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .or_else(default_app_config_dir)
}

// -----------------------------------------------------------------------------
// Workspaces: read config.json (same schema as Tauri settings)
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

fn list_workspaces_impl(app_config_dir: &Path) -> Result<Vec<serde_json::Value>, String> {
    let path = app_config_dir.join("config.json");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Ok(Vec::new()),
    };
    let cfg: ConfigFileDto = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    let list: Vec<serde_json::Value> = cfg
        .workspaces
        .into_iter()
        .map(|w| {
            serde_json::json!({
                "id": w.id,
                "name": if w.name.is_empty() {
                    Path::new(&w.root_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Workspace")
                        .to_string()
                } else {
                    w.name
                },
                "root_path": w.root_path,
            })
        })
        .collect();
    Ok(list)
}

// -----------------------------------------------------------------------------
// Agent request: stub executor (all tools return error; use step API from app for full execution)
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
    context: Option<TaskContext>,
) -> Result<serde_json::Value, String> {
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

    let result = agent_request(input, &mut session, &config, &executor)?;
    let answer = result
        .final_answer
        .unwrap_or_else(|| "(no answer text)".to_string());

    Ok(serde_json::json!({
        "ok": true,
        "answer": answer,
        "error": serde_json::Value::Null,
    }))
}

// -----------------------------------------------------------------------------
// FFI: JSON in → call core → JSON out; all returned strings must be freed with oxcer_string_free
// -----------------------------------------------------------------------------

fn json_response_ok<T: serde::Serialize>(value: T) -> *const c_char {
    match serde_json::to_string(&value) {
        Ok(s) => return_string(s),
        Err(e) => return_string(serde_json::json!({ "ok": false, "error": e.to_string() }).to_string()),
    }
}

fn json_response_err(msg: &str) -> *const c_char {
    return_string(
        serde_json::json!({ "ok": false, "error": msg }).to_string(),
    )
}

/// Input: `{}` or `{ "app_config_dir": "/path" }`.
/// Output: `{ "workspaces": [ { "id", "name", "root_path" }, ... ] }`.
#[no_mangle]
pub extern "C" fn oxcer_list_workspaces(json_in: *const c_char) -> *const c_char {
    let input_str = match ptr_to_str(json_in) {
        Some(s) => s,
        None => return json_response_err("invalid or null input"),
    };
    let input: serde_json::Value = match serde_json::from_str(&input_str) {
        Ok(v) => v,
        Err(_) => serde_json::json!({}),
    };
    let app_config_dir = match app_config_dir_from_json(&input) {
        Some(d) => d,
        None => return json_response_err("app_config_dir required (or set default data dir)"),
    };
    match list_workspaces_impl(&app_config_dir) {
        Ok(workspaces) => json_response_ok(serde_json::json!({ "workspaces": workspaces })),
        Err(e) => json_response_err(&e),
    }
}

/// Input: `{}` or `{ "app_config_dir": "/path" }`.
/// Output: `[ { "session_id", "start_timestamp", "end_timestamp", "total_cost_usd", "success", "tool_calls_count", "approvals_count", "denies_count" }, ... ]`.
#[no_mangle]
pub extern "C" fn oxcer_list_sessions(json_in: *const c_char) -> *const c_char {
    let input_str = match ptr_to_str(json_in) {
        Some(s) => s,
        None => return json_response_err("invalid or null input"),
    };
    let input: serde_json::Value = match serde_json::from_str(&input_str) {
        Ok(v) => v,
        Err(_) => serde_json::json!({}),
    };
    let app_config_dir = match app_config_dir_from_json(&input) {
        Some(d) => d,
        None => return json_response_err("app_config_dir required"),
    };
    match list_sessions_from_dir(&app_config_dir) {
        Ok(summaries) => {
            let arr: Vec<serde_json::Value> = summaries
                .into_iter()
                .map(|s| {
                    serde_json::json!({
                        "session_id": s.session_id,
                        "start_timestamp": s.start_timestamp,
                        "end_timestamp": s.end_timestamp,
                        "total_cost_usd": s.total_cost_usd,
                        "success": s.success,
                        "tool_calls_count": s.tool_calls_count,
                        "approvals_count": s.approvals_count,
                        "denies_count": s.denies_count,
                    })
                })
                .collect();
            return_string(serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string()))
        }
        Err(e) => json_response_err(&e),
    }
}

/// Input: `{ "session_id": "..." }` and optional `"app_config_dir"`.
/// Output: `[ LogEvent, ... ]` (same schema as telemetry).
#[no_mangle]
pub extern "C" fn oxcer_load_session_log(json_in: *const c_char) -> *const c_char {
    let input_str = match ptr_to_str(json_in) {
        Some(s) => s,
        None => return json_response_err("invalid or null input"),
    };
    let input: serde_json::Value = match serde_json::from_str(&input_str) {
        Ok(v) => v,
        Err(_) => return json_response_err("invalid JSON input"),
    };
    let session_id = input
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or("session_id required");
    let session_id = match session_id {
        Ok(s) => s,
        Err(e) => return json_response_err(e),
    };
    let app_config_dir = match app_config_dir_from_json(&input) {
        Some(d) => d,
        None => return json_response_err("app_config_dir required"),
    };
    match load_session_log_from_dir(&app_config_dir, session_id) {
        Ok(events) => return_string(
            serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => json_response_err(&e),
    }
}

/// Input: `{ "task_description": "...", "workspace_id": "...", "workspace_root": "...", "context": {...}, "llm_api_key": "...", "llm_base_url": "...", "model_id": "..." }`.
/// Output: `{ "ok": true, "answer": "...", "error": null }` or `{ "ok": false, "error": "..." }`.
#[no_mangle]
pub extern "C" fn oxcer_agent_request(json_in: *const c_char) -> *const c_char {
    let input_str = match ptr_to_str(json_in) {
        Some(s) => s,
        None => return json_response_err("invalid or null input"),
    };
    let input: serde_json::Value = match serde_json::from_str(&input_str) {
        Ok(v) => v,
        Err(e) => return json_response_err(&e.to_string()),
    };
    let task_description = input
        .get("task_description")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or("task_description required");
    let task_description = match task_description {
        Ok(t) => t,
        Err(e) => return json_response_err(e),
    };
    let workspace_id = input.get("workspace_id").and_then(|v| v.as_str()).map(String::from);
    let workspace_root = input.get("workspace_root").and_then(|v| v.as_str()).map(String::from);
    let context = input
        .get("context")
        .and_then(|v| serde_json::from_value::<TaskContext>(v.clone()).ok());

    match agent_request_impl(task_description, workspace_id, workspace_root, context) {
        Ok(out) => return_string(out.to_string()),
        Err(e) => json_response_err(&e),
    }
}

/// Free a string returned by any oxcer_* FFI function. Safe to call with null.
#[no_mangle]
pub extern "C" fn oxcer_string_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(ptr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn ffi_input(s: &str) -> *const c_char {
        CString::new(s).unwrap().into_raw()
    }

    #[test]
    fn list_workspaces_requires_app_config_dir_or_fails() {
        let out = oxcer_list_workspaces(ffi_input("{}"));
        assert!(!out.is_null());
        let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_string() };
        oxcer_string_free(out as *mut c_char);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        // If default_app_config_dir is None (e.g. in test env), we get error
        if v.get("workspaces").is_some() {
            assert!(v.get("workspaces").unwrap().is_array());
        } else {
            assert!(v.get("error").is_some());
        }
    }

    #[test]
    fn list_sessions_returns_array_or_error() {
        let out = oxcer_list_sessions(ffi_input(r#"{"app_config_dir":"/nonexistent"}"#));
        assert!(!out.is_null());
        let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_string() };
        oxcer_string_free(out as *mut c_char);
        // Either array of sessions or error; JSON must be valid
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v.is_array() || v.get("error").is_some());
    }

    #[test]
    fn load_session_log_requires_session_id() {
        let out = oxcer_load_session_log(ffi_input("{}"));
        assert!(!out.is_null());
        let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_string() };
        oxcer_string_free(out as *mut c_char);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        // Missing session_id returns error object
        assert!(v.get("error").is_some(), "expected error when session_id missing");
    }

    #[test]
    fn agent_request_fails_without_task_description() {
        let out = oxcer_agent_request(ffi_input("{}"));
        assert!(!out.is_null());
        let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_string() };
        oxcer_string_free(out as *mut c_char);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("error").is_some());
    }

    #[test]
    fn string_free_null_safe() {
        oxcer_string_free(std::ptr::null_mut());
    }
}
