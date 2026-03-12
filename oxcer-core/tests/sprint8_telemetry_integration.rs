//! Sprint 8 §7.2 integration test: small end-to-end session trace.
//!
//! Writes telemetry events (semantic_router, security, llm_client, orchestrator)
//! via log_event, then reads the per-session JSONL and asserts:
//! - Session file exists and parses as LogEvent lines.
//! - Events from semantic_router, llm_client, security, orchestrator are present.
//! - At least one event has non-zero tokens_in, tokens_out, or latency_ms.

use oxcer_core::telemetry::{log_event, LogEvent, LogMetrics};

const SESSION_ID: &str = "sprint8-e2e";

#[test]
fn e2e_session_trace_produces_jsonl_with_expected_components_and_metrics() {
    let tmp = tempfile::tempdir().unwrap();
    let app_config = tmp.path();

    // 1) Semantic router classify
    log_event(
        app_config,
        SESSION_ID,
        Some("req-1"),
        "test",
        "semantic_router",
        "classify",
        Some("success"),
        LogMetrics {
            tokens_in: Some(12),
            ..Default::default()
        },
        serde_json::json!({
            "category": "tools_heavy",
            "strategy": "tools_only",
            "input_length_chars": 50
        }),
    )
    .unwrap();

    // 2) Security policy_evaluate
    log_event(
        app_config,
        SESSION_ID,
        Some("req-1"),
        "test",
        "security",
        "policy_evaluate",
        Some("approval_required"),
        LogMetrics::default(),
        serde_json::json!({
            "tool": "fs_delete",
            "workspace_id": "ws1",
            "rule_id": "agent_fs_delete",
            "rule_reason": "destructive"
        }),
    )
    .unwrap();

    // 3) LLM invoke with metrics
    log_event(
        app_config,
        SESSION_ID,
        Some("req-2"),
        "test",
        "llm_client",
        "invoke",
        Some("success"),
        LogMetrics {
            tokens_in: Some(100),
            tokens_out: Some(40),
            latency_ms: Some(250),
            cost_usd: Some(0.002),
        },
        serde_json::json!({ "model": "gemini-1.5-flash" }),
    )
    .unwrap();

    // 4) Orchestrator session_complete
    log_event(
        app_config,
        SESSION_ID,
        None,
        "test",
        "orchestrator",
        "session_complete",
        Some("success"),
        LogMetrics::default(),
        serde_json::json!({ "outcome": "success" }),
    )
    .unwrap();

    // Read per-session file (session_id is used as filename after sanitization; "sprint8-e2e" is already safe)
    let session_path = app_config
        .join("logs")
        .join(format!("{}.jsonl", SESSION_ID));
    assert!(
        session_path.exists(),
        "session log file should exist: {:?}",
        session_path
    );

    let content = std::fs::read_to_string(&session_path).unwrap();
    let lines: Vec<&str> = content.lines().filter(|s| !s.is_empty()).collect();
    assert!(
        !lines.is_empty(),
        "session log should have at least one line"
    );

    let events: Vec<LogEvent> = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<LogEvent>(line).ok())
        .collect();
    assert_eq!(
        events.len(),
        lines.len(),
        "every line should parse as LogEvent"
    );

    let components: std::collections::HashSet<&str> =
        events.iter().map(|e| e.component.as_str()).collect();
    assert!(
        components.contains("semantic_router"),
        "should have semantic_router event"
    );
    assert!(
        components.contains("security"),
        "should have security event"
    );
    assert!(
        components.contains("llm_client"),
        "should have llm_client event"
    );
    assert!(
        components.contains("orchestrator"),
        "should have orchestrator event"
    );

    let has_metrics = events.iter().any(|e| {
        e.metrics.tokens_in.unwrap_or(0) > 0
            || e.metrics.tokens_out.unwrap_or(0) > 0
            || e.metrics.latency_ms.unwrap_or(0) > 0
    });
    assert!(
        has_metrics,
        "at least one event should have non-zero tokens_in, tokens_out, or latency_ms"
    );
}
