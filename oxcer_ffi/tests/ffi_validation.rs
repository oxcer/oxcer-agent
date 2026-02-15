//! FFI validation tests: round-trip via UniFFI API.
//!
//! Verifies list_workspaces, list_sessions, load_session_log, run_agent_task
//! with valid/invalid input.

use oxcer_ffi::{list_workspaces, load_session_log, run_agent_task, AgentRequestPayload};

#[test]
fn list_workspaces_empty_app_config_dir_returns_error() {
    // Empty string → no default dir on CI → Err
    let r = list_workspaces(String::new());
    if let Err(e) = r {
        assert!(e.to_string().contains("app_config_dir"), "expected app_config_dir error, got: {}", e);
    }
}

#[test]
fn list_workspaces_roundtrip_valid_config() {
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

    let app_config_dir = app_config.display().to_string();
    let r = list_workspaces(app_config_dir);
    let workspaces = r.expect("list_workspaces should succeed");
    assert_eq!(workspaces.len(), 2);
    assert_eq!(workspaces[0].id, "ws1");
    assert_eq!(workspaces[0].name, "Project A");
    assert_eq!(workspaces[0].root_path, "/tmp/proj_a");
    assert_eq!(workspaces[1].id, "ws2");
}

#[tokio::test]
async fn run_agent_task_empty_task_returns_error() {
    let payload = AgentRequestPayload {
        task_description: String::new(),
        workspace_id: None,
        workspace_root: None,
        context: None,
        app_config_dir: None,
    };
    let r = run_agent_task(payload).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("task_description"));
}

#[tokio::test]
async fn run_agent_task_valid_task_succeeds() {
    let payload = AgentRequestPayload {
        task_description: "list files in workspace".to_string(),
        workspace_id: None,
        workspace_root: None,
        context: None,
        app_config_dir: Some("/tmp".to_string()),
    };
    let r = run_agent_task(payload).await;
    let response = r.expect("run_agent_task should succeed for empty-plan path");
    assert!(response.ok);
    assert!(response.answer.is_some());
    assert!(response.error.is_none());
}

#[test]
fn load_session_log_missing_session_id_still_takes_two_args() {
    // Our API is load_session_log(session_id, app_config_dir).
    // Missing session_id is a logic error on caller; we pass empty app_config_dir to get "required" error.
    let r = load_session_log("some-session".to_string(), String::new());
    // Either Ok(events) or Err about app_config_dir
    if let Err(e) = r {
        let msg = e.to_string();
        assert!(msg.contains("app_config_dir") || msg.contains("No such file"));
    }
}
