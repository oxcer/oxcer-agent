//! Integration tests for event_log: append and rotation via public API.
//!
//! Uses a temporary directory for config; does not touch the real app config.

use oxcer::event_log;

#[test]
fn append_writes_to_logs_events_log() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();

    event_log::append(
        app_config,
        "test_event",
        Some("ws-1"),
        Some(&serde_json::json!({ "key": "value" })),
    )
    .unwrap();

    let log_path = app_config.join("logs").join("events.log");
    assert!(log_path.exists(), "logs/events.log should exist");
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(content.contains("\"event_type\":\"test_event\""));
    assert!(content.contains("\"workspace_id\":\"ws-1\""));
    assert!(content.contains("\"key\":\"value\""));
}

#[test]
fn rotate_retention_keeps_only_recent_entries_by_cutoff() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();
    let dir = app_config.join("logs");
    std::fs::create_dir_all(&dir).unwrap();
    let path = app_config.join("logs").join("events.log");

    let now = chrono::Utc::now();
    let cutoff = now - chrono::Duration::days(30);
    let old_ts = now - chrono::Duration::days(40);
    let new_ts = now - chrono::Duration::days(5);

    let old_entry = serde_json::json!({
        "timestamp": old_ts.to_rfc3339(),
        "event_type": "old",
        "workspace_id": null,
        "details": { "marker": "old" }
    });
    let new_entry = serde_json::json!({
        "timestamp": new_ts.to_rfc3339(),
        "event_type": "new",
        "workspace_id": null,
        "details": { "marker": "new" }
    });
    let content = format!(
        "{}\n{}",
        serde_json::to_string(&old_entry).unwrap(),
        serde_json::to_string(&new_entry).unwrap()
    );
    std::fs::write(&path, content).unwrap();

    event_log::rotate_retention(&path, cutoff).unwrap();

    let after = std::fs::read_to_string(&path).unwrap();
    assert!(!after.contains("\"marker\":\"old\""));
    assert!(after.contains("\"marker\":\"new\""));
}
