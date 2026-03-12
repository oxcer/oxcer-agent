//! Integration tests for workspace delete and cleanup.
//!
//! Run with: `cargo test -p oxcer workspace_cleanup`. Does not require Tauri app or `--features test`.
//! Verifies that workspace cleanup removes the workspace from config, cancels
//! pending approvals for that root, and appends a `workspace_removed` event.
//! Uses `workspace_cleanup_impl` with temp state so we don't need the main thread (macOS EventLoop).

use std::sync::Mutex;

use oxcer::commands::workspace_cleanup_impl;
use oxcer::router::{PendingApprovalsStore, PendingOperation};
use oxcer::settings::{save as settings_save, AppSettings, WorkspaceDirectory};
use oxcer_core::security::policy_engine::{Operation, PolicyCaller, PolicyTarget, ToolType};

const WORKSPACE_ID: &str = "ws-cleanup-test";

#[test]
fn workspace_cleanup_removes_workspace_and_pending_approvals() {
    let app_config_dir = tempfile::tempdir().unwrap();
    let app_config_path = app_config_dir.path();

    let workspace_root_dir = tempfile::tempdir().unwrap();
    let workspace_root_path = workspace_root_dir.path().display().to_string();

    let state = Mutex::new({
        let mut s = AppSettings::default();
        s.workspace_directories.push(WorkspaceDirectory {
            id: WORKSPACE_ID.to_string(),
            name: "Test Workspace".to_string(),
            path: workspace_root_path.clone(),
        });
        s
    });

    let store = PendingApprovalsStore::new();
    let record = store.create_record(
        "req-1".to_string(),
        PolicyCaller::AgentOrchestrator,
        ToolType::Fs,
        Operation::Delete,
        PolicyTarget::FsPath {
            canonical_path: format!("{}/some/file", workspace_root_path),
        },
        PendingOperation::FsDelete {
            workspace_root: workspace_root_path.clone(),
            rel_path: "some/file".to_string(),
        },
        "TEST_REASON".to_string(),
        "Delete test file".to_string(),
    );
    store.insert(record);

    settings_save(app_config_path, &*state.lock().unwrap()).expect("save config");

    let mut emitted = None;
    workspace_cleanup_impl(app_config_path, &state, Some(&store), WORKSPACE_ID, |id| {
        emitted = Some(id.to_string());
    })
    .expect("cleanup");

    assert_eq!(
        emitted.as_deref(),
        Some(WORKSPACE_ID),
        "emit callback should run"
    );

    {
        let guard = state.lock().unwrap();
        assert!(
            !guard
                .workspace_directories
                .iter()
                .any(|w| w.id == WORKSPACE_ID),
            "workspace should be removed from settings"
        );
    }

    let config_path = app_config_path.join("config.json");
    let config_str = std::fs::read_to_string(&config_path).unwrap_or_default();
    assert!(
        !config_str.contains(WORKSPACE_ID),
        "config.json should not contain workspace id"
    );

    let taken = store.take("req-1");
    assert!(
        taken.is_none(),
        "pending approval for deleted workspace root should be cancelled"
    );

    let log_path = app_config_path.join("logs").join("events.log");
    assert!(log_path.exists(), "events.log should exist");
    let log_content = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        log_content.contains("\"event_type\":\"workspace_removed\""),
        "event log should contain workspace_removed"
    );
    assert!(
        log_content.contains(format!("\"workspace_id\":\"{}\"", WORKSPACE_ID).as_str()),
        "event log should reference the removed workspace id"
    );
    assert!(
        log_content.contains("\"root_path\""),
        "event log details should include root_path"
    );
}
