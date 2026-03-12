//! Telemetry edge-case tests: corrupted lines, empty files, list_sessions with bad files.
//!
//! Complements sprint8_telemetry_integration.rs with robustness checks.

use oxcer_core::telemetry::{
    list_sessions_from_dir, load_session_log_from_dir,
};
use std::fs;

const SESSION_ID: &str = "sprint-telemetry-edge";

/// Corrupted JSONL line: list_sessions_from_dir skips unparseable lines, still returns valid sessions.
#[test]
fn list_sessions_skips_corrupted_lines_returns_valid_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();
    let logs_dir = app_config.join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let session_path = logs_dir.join(format!("{}.jsonl", SESSION_ID));

    // Write valid event + corrupted line + valid event
    let valid1 = r#"{"timestamp":"2025-01-01T00:00:01.000Z","session_id":"sprint-telemetry-edge","request_id":"r1","caller":"test","component":"router","action":"classify","metrics":{},"details":{}}"#;
    let corrupted = "{ invalid json }";
    let valid2 = r#"{"timestamp":"2025-01-01T00:00:02.000Z","session_id":"sprint-telemetry-edge","request_id":"r1","caller":"test","component":"orchestrator","action":"session_complete","decision":"success","metrics":{},"details":{"outcome":"success"}}"#;
    fs::write(
        &session_path,
        format!("{}\n{}\n{}\n", valid1, corrupted, valid2),
    )
    .unwrap();

    let summaries = list_sessions_from_dir(app_config).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].session_id, SESSION_ID);
    assert!(summaries[0].success);
}

/// load_session_log_from_dir returns only parseable events (skips corrupted lines).
#[test]
fn load_session_log_skips_corrupted_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();
    let logs_dir = app_config.join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let session_path = logs_dir.join(format!("{}.jsonl", SESSION_ID));

    let valid = r#"{"timestamp":"2025-01-01T00:00:01.000Z","session_id":"sprint-telemetry-edge","caller":"test","component":"security","action":"policy_evaluate","metrics":{},"details":{}}"#;
    fs::write(&session_path, format!("{}\n{{ bad }}\n", valid)).unwrap();

    let events = load_session_log_from_dir(app_config, SESSION_ID).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].component, "security");
}

/// Empty logs dir: list_sessions returns empty vec.
#[test]
fn list_sessions_empty_dir_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();
    let logs_dir = app_config.join("logs");
    fs::create_dir_all(&logs_dir).unwrap();

    let summaries = list_sessions_from_dir(app_config).unwrap();
    assert!(summaries.is_empty());
}

/// Nonexistent logs dir: list_sessions returns empty (not error).
#[test]
fn list_sessions_no_logs_dir_returns_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();
    // Do not create logs/ — it doesn't exist

    let summaries = list_sessions_from_dir(app_config).unwrap();
    assert!(summaries.is_empty());
}
