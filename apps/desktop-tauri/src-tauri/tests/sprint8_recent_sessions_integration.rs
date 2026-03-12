//! Sprint 8 §7.2 integration tests: list_sessions / load_session_log from JSONL.
//!
//! Writes a session file via oxcer_core::telemetry::log_event, then exercises
//! telemetry_viewer::list_sessions_from_dir and load_session_log_from_dir.
//! Manual QA: In the Launcher dashboard, open "Recent Sessions", confirm the list
//! loads and that filtering by component and decision works.

use oxcer::telemetry_viewer;
use oxcer_core::telemetry::{log_event, LogMetrics};

const SESSION_ID: &str = "sprint8-recent";

#[test]
fn list_sessions_from_dir_returns_summary_for_written_session_log() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();

    // Write one session log (same format as oxcer-core integration test)
    log_event(
        app_config,
        SESSION_ID,
        Some("r1"),
        "test",
        "semantic_router",
        "classify",
        Some("success"),
        LogMetrics::default(),
        serde_json::json!({ "category": "tools_heavy" }),
    )
    .unwrap();
    log_event(
        app_config,
        SESSION_ID,
        Some("r1"),
        "test",
        "orchestrator",
        "session_complete",
        Some("success"),
        LogMetrics::default(),
        serde_json::json!({ "outcome": "success" }),
    )
    .unwrap();

    let list = telemetry_viewer::list_sessions_from_dir(app_config).unwrap();
    assert_eq!(list.len(), 1, "one session file => one summary");
    assert_eq!(list[0].session_id, SESSION_ID);
    assert!(list[0].success);
}

#[test]
fn load_session_log_from_dir_returns_events_for_session() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();

    log_event(
        app_config,
        SESSION_ID,
        None,
        "test",
        "security",
        "policy_evaluate",
        Some("allow"),
        LogMetrics::default(),
        serde_json::json!({ "rule_id": "test_rule" }),
    )
    .unwrap();

    let events = telemetry_viewer::load_session_log_from_dir(app_config, SESSION_ID).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].component, "security");
    assert_eq!(events[0].action, "policy_evaluate");
    assert_eq!(events[0].decision.as_deref(), Some("allow"));
}
