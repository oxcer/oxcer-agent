//! FFI validation tests: round-trip JSON, invalid input, memory safety.
//!
//! Verifies JSON in/out shapes and that invalid input produces structured errors
//! rather than panics or undefined behavior.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

fn with_json_input<F, R>(json: &str, f: F) -> R
where
    F: FnOnce(*const c_char) -> R,
{
    let cstr = CString::new(json).unwrap();
    f(cstr.as_ptr())
}

#[test]
fn ffi_null_input_returns_error_json_not_null() {
    let out = oxcer_ffi::oxcer_list_workspaces(std::ptr::null());
    assert!(!out.is_null());
    let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_string() };
    oxcer_ffi::oxcer_string_free(out as *mut c_char);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("error").is_some(), "null input should return error object: {}", s);
}

#[test]
fn ffi_list_workspaces_roundtrip_valid_config() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();
    std::fs::create_dir_all(app_config).unwrap();
    let config_path = app_config.join("config.json");
    let config = serde_json::json!({
        "workspaces": [
            { "id": "ws1", "name": "Project A", "root_path": "/tmp/proj_a" },
            { "id": "ws2", "name": "Project B", "root_path": "/tmp/proj_b" }
        ]
    });
    std::fs::write(&config_path, config.to_string()).unwrap();

    let input = format!(r#"{{"app_config_dir":"{}"}}"#, app_config.display());
    let out = with_json_input(&input, |ptr| oxcer_ffi::oxcer_list_workspaces(ptr));
    assert!(!out.is_null());
    let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_string() };
    oxcer_ffi::oxcer_string_free(out as *mut c_char);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("error").is_none(), "expected success, got: {}", s);
    let workspaces = v.get("workspaces").and_then(|w| w.as_array()).unwrap();
    assert_eq!(workspaces.len(), 2);
    assert_eq!(workspaces[0]["id"], "ws1");
    assert_eq!(workspaces[0]["root_path"], "/tmp/proj_a");
}

#[test]
fn ffi_agent_request_empty_plan_succeeds() {
    // Task "list files" with no workspace_root → empty plan → completes with "(no answer text)"
    let input = r#"{"task_description":"list files in workspace","app_config_dir":"/tmp"}"#;
    let out = with_json_input(input, |ptr| oxcer_ffi::oxcer_agent_request(ptr));
    assert!(!out.is_null());
    let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_string() };
    oxcer_ffi::oxcer_string_free(out as *mut c_char);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("error").is_none(), "expected success for empty-plan path, got: {}", s);
    assert_eq!(v.get("ok"), Some(&serde_json::json!(true)));
    assert!(v.get("answer").is_some());
}

#[test]
fn ffi_agent_request_invalid_json_returns_error() {
    let input = "{ invalid }";
    let out = with_json_input(input, |ptr| oxcer_ffi::oxcer_agent_request(ptr));
    assert!(!out.is_null());
    let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_string() };
    oxcer_ffi::oxcer_string_free(out as *mut c_char);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("error").is_some(), "invalid JSON should return error: {}", s);
}

#[test]
fn ffi_load_session_log_missing_session_id_returns_error() {
    let input = r#"{"app_config_dir":"/tmp"}"#;
    let out = with_json_input(input, |ptr| oxcer_ffi::oxcer_load_session_log(ptr));
    assert!(!out.is_null());
    let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_string() };
    oxcer_ffi::oxcer_string_free(out as *mut c_char);
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v.get("error").is_some());
}
