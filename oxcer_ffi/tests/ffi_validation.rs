//! FFI integration tests: round-trip via UniFFI API.
//!
//! Each group is labelled with the migration stage it validates.

use oxcer_ffi::{list_workspaces, load_session_log, run_agent_task, AgentRequestPayload};

// -----------------------------------------------------------------------------
// Stage 4: Live contract
//
// Rust: pub fn list_workspaces(dir: String) -> Result<Vec<WorkspaceInfo>, OxcerError>
// connected to list_workspaces_impl() reading config.json.
// -----------------------------------------------------------------------------

#[test]
fn list_workspaces_stage4_empty_dir_returns_empty_vec() {
    let tmp = tempfile::tempdir().unwrap();
    let r = list_workspaces(tmp.path().display().to_string());
    let workspaces = r.expect("empty dir should return Ok([])");
    assert!(workspaces.is_empty());
}

#[test]
fn list_workspaces_stage4_roundtrip_valid_config() {
    let tmp = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "workspaces": [
            { "id": "ws1", "name": "Project A", "root_path": "/tmp/proj_a" },
            { "id": "ws2", "name": "Project B", "root_path": "/tmp/proj_b" }
        ]
    });
    std::fs::write(tmp.path().join("config.json"), config.to_string()).unwrap();

    let workspaces = list_workspaces(tmp.path().display().to_string())
        .expect("list_workspaces should succeed with valid config");
    assert_eq!(workspaces.len(), 2);
    assert_eq!(workspaces[0].id, "ws1");
    assert_eq!(workspaces[0].name, "Project A");
    assert_eq!(workspaces[0].root_path, "/tmp/proj_a");
    assert_eq!(workspaces[1].id, "ws2");
}

// -----------------------------------------------------------------------------
// Other FFI contracts (unchanged across all stages)
// -----------------------------------------------------------------------------

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

#[test]
fn load_session_log_missing_session_id_still_takes_two_args() {
    let r = load_session_log("some-session".to_string(), String::new());
    if let Err(e) = r {
        let msg = e.to_string();
        assert!(msg.contains("app_config_dir") || msg.contains("No such file"));
    }
}
