//! Structured telemetry for the agent stack (Sprint 8).
//!
//! Single JSON event schema used across semantic router, LLM client, security policy,
//! and tools. Events are appended as one JSON line to:
//! - `logs/{session_id}.jsonl` (per-session trace),
//! - `logs/telemetry.jsonl` (rolling, 30-day/10MB retention).
//!
//! **Memory bounds:** To prevent runaway memory usage when logs grow large:
//! - `load_session_log_from_dir` returns at most `MAX_EVENTS_PER_SESSION_LOG` (2000) events,
//!   preferring the latest (tail), via line-by-line streaming.
//! - `list_sessions_from_dir` skips files > `MAX_SESSION_FILE_BYTES_FOR_SUMMARY` (50 MB);
//!   for files between ~768 KB and 50 MB, reads only head + tail chunks.
//!
//! **Security:** Callers must pass only scrubbed content in `details` (e.g. via
//! Sprint 7 prompt scrubbing). Raw secrets must never be written to these logs.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const LOG_DIR: &str = "logs";
const TELEMETRY_FILENAME: &str = "telemetry.jsonl";
const MAX_AGE_DAYS: i64 = 30;
const MAX_BYTES: u64 = 10 * 1024 * 1024; // 10MB

/// Maximum number of events returned when loading a session log.
/// Prevents unbounded memory growth when session files grow large (e.g. long-running sessions).
/// Matches Swift SessionDetailViewModel maxSessionEventsCount.
const MAX_EVENTS_PER_SESSION_LOG: usize = 2000;

/// Maximum file size (bytes) we will read when building session summaries.
/// Files larger than this are skipped to avoid multi-GB allocations during list_sessions.
/// 50 MB is enough for ~50k–100k typical JSONL events; summaries are approximate for huge files anyway.
const MAX_SESSION_FILE_BYTES_FOR_SUMMARY: u64 = 50 * 1024 * 1024;

/// Metrics attached to a telemetry event (tokens, latency, cost).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LogMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// One structured telemetry event (one line of JSON in logs).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEvent {
    pub timestamp: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub caller: String,
    pub component: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    pub metrics: LogMetrics,
    pub details: serde_json::Value,
}

/// Summary of a session for the Recent Sessions list (Sprint 8 §5).
#[derive(Clone, Debug, Serialize, Deserialize)]
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

/// Sanitize `session_id` for use as a filename (alphanumeric, hyphen, underscore only).
fn sanitize_session_id_for_filename(session_id: &str) -> String {
    session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn logs_dir(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join(LOG_DIR)
}

fn telemetry_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join(LOG_DIR).join(TELEMETRY_FILENAME)
}

fn session_log_path(app_config_dir: &Path, session_id: &str) -> PathBuf {
    let safe = sanitize_session_id_for_filename(session_id);
    let name = if safe.is_empty() { "default" } else { &safe };
    app_config_dir.join(LOG_DIR).join(format!("{}.jsonl", name))
}

/// Append one telemetry event to both per-session and rolling telemetry logs.
///
/// - `logs/{session_id}.jsonl`: one file per session (no automatic retention).
/// - `logs/telemetry.jsonl`: rolling log with 30-day/10MB retention.
///
/// Callers must ensure `details` has already been scrubbed (Sprint 7); raw secrets
/// must never be passed here.
pub fn log_event(
    app_config_dir: &Path,
    session_id: &str,
    request_id: Option<&str>,
    caller: &str,
    component: &str,
    action: &str,
    decision: Option<&str>,
    metrics: LogMetrics,
    details: serde_json::Value,
) -> Result<(), String> {
    let logs = logs_dir(app_config_dir);
    std::fs::create_dir_all(&logs).map_err(|e| format!("create logs dir: {}", e))?;

    let event = LogEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        session_id: session_id.to_string(),
        request_id: request_id.map(String::from),
        caller: caller.to_string(),
        component: component.to_string(),
        action: action.to_string(),
        decision: decision.map(String::from),
        metrics,
        details,
    };
    let line = serde_json::to_string(&event).map_err(|e| format!("serialize event: {}", e))?;

    // Per-session trace
    let session_path = session_log_path(app_config_dir, session_id);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&session_path)
        .map_err(|e| format!("open session log {:?}: {}", session_path, e))?;
    writeln!(f, "{}", line).map_err(|e| format!("write session log: {}", e))?;
    f.sync_all().map_err(|e| format!("sync session log: {}", e))?;

    // Rolling telemetry
    let telemetry_path = telemetry_path(app_config_dir);
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&telemetry_path)
        .map_err(|e| format!("open telemetry log: {}", e))?;
    writeln!(f, "{}", line).map_err(|e| format!("write telemetry log: {}", e))?;
    f.sync_all().map_err(|e| format!("sync telemetry log: {}", e))?;

    rotate_telemetry_if_needed(app_config_dir)?;
    Ok(())
}

/// Apply 30-day/10MB retention to telemetry.jsonl (keep newest entries).
fn rotate_telemetry_if_needed(app_config_dir: &Path) -> Result<(), String> {
    let path = telemetry_path(app_config_dir);
    let cutoff = chrono::Utc::now() - chrono::Duration::days(MAX_AGE_DAYS);
    rotate_retention(&path, cutoff)
}

/// Keep lines with timestamp >= cutoff, then keep newest up to MAX_BYTES.
/// Exposed for tests.
pub fn rotate_retention(
    path: &Path,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader
        .lines()
        .filter_map(|r| r.ok())
        .filter(|s| !s.is_empty())
        .collect();
    if lines.is_empty() {
        return Ok(());
    }

    let mut with_ts: Vec<(chrono::DateTime<chrono::Utc>, String)> = Vec::new();
    for line in &lines {
        if let Ok(entry) = serde_json::from_str::<LogEvent>(line) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&entry.timestamp) {
                let utc = dt.with_timezone(&chrono::Utc);
                if utc >= cutoff {
                    with_ts.push((utc, line.clone()));
                }
            }
        }
    }
    with_ts.sort_by_key(|(t, _)| *t);

    let mut kept_bytes: u64 = 0;
    let mut kept: Vec<String> = Vec::new();
    for (_, s) in with_ts.iter().rev() {
        let line_bytes = (s.len() + 1) as u64;
        if kept_bytes + line_bytes > MAX_BYTES {
            break;
        }
        kept_bytes += line_bytes;
        kept.push(s.clone());
    }
    kept.reverse();

    let out = kept.join("\n");
    std::fs::write(path, out).map_err(|e| format!("write after rotation: {}", e))?;
    Ok(())
}

/// Maximum bytes to read from the head of a session file when building a summary.
/// Used for files larger than this; we read head + tail only to bound memory.
const SUMMARY_HEAD_READ_BYTES: u64 = 256 * 1024; // 256 KB

/// Maximum bytes to read from the tail of a session file when building a summary.
const SUMMARY_TAIL_READ_BYTES: u64 = 512 * 1024; // 512 KB

/// List recent sessions from app_config_dir/logs/*.jsonl (excludes telemetry.jsonl).
///
/// Per-file memory is bounded:
/// - Files larger than `MAX_SESSION_FILE_BYTES_FOR_SUMMARY` (50 MB) are skipped with a log.
/// - Files larger than `SUMMARY_HEAD_READ_BYTES + SUMMARY_TAIL_READ_BYTES` are read partially
///   (head + tail only); the resulting `SessionSummary` is approximate.
/// - Smaller files are read in full for accurate summaries.
pub fn list_sessions_from_dir(app_config_dir: &Path) -> Result<Vec<SessionSummary>, String> {
    let logs_dir = app_config_dir.join(LOG_DIR);
    if !logs_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut summaries = Vec::new();
    let dir_entries = std::fs::read_dir(&logs_dir).map_err(|e| e.to_string())?;
    for entry in dir_entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().map_or(true, |e| e != "jsonl") {
            continue;
        }
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name == TELEMETRY_FILENAME {
            continue;
        }
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        let file_size = match std::fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(_) => continue,
        };

        if file_size > MAX_SESSION_FILE_BYTES_FOR_SUMMARY {
            eprintln!(
                "[telemetry] list_sessions: skipping {} ({} MB > {} MB limit)",
                path.display(),
                file_size / (1024 * 1024),
                MAX_SESSION_FILE_BYTES_FOR_SUMMARY / (1024 * 1024)
            );
            continue;
        }

        let events = match read_session_events_bounded(&path, file_size) {
            Ok(ev) => ev,
            Err(e) => {
                eprintln!("[telemetry] list_sessions: read {:?}: {}", path, e);
                continue;
            }
        };

        if events.is_empty() {
            continue;
        }

        let start_timestamp = events.first().map(|e| e.timestamp.clone()).unwrap_or_default();
        let end_timestamp = events.last().map(|e| e.timestamp.clone()).unwrap_or_default();
        let total_cost_usd: f64 = events.iter().filter_map(|e| e.metrics.cost_usd).sum();
        let success = events
            .iter()
            .rev()
            .find(|e| e.action == "session_complete" || e.action == "session_summary")
            .map(|e| {
                e.decision.as_deref() == Some("success")
                    || e.details.get("outcome").and_then(|v| v.as_str()) == Some("success")
            })
            .unwrap_or(false);
        let tool_calls_count = events
            .iter()
            .filter(|e| e.component == "security" && e.action == "policy_evaluate")
            .count() as u32;
        let approvals_count = events
            .iter()
            .filter(|e| {
                e.component == "security"
                    && e.action == "approval_decision"
                    && e.decision.as_deref() == Some("approve")
            })
            .count() as u32;
        let denies_count = events
            .iter()
            .filter(|e| {
                (e.component == "security"
                    && e.action == "approval_decision"
                    && e.decision.as_deref() == Some("deny"))
                    || (e.component == "security"
                        && e.action == "policy_evaluate"
                        && e.decision.as_deref() == Some("deny"))
            })
            .count() as u32;
        summaries.push(SessionSummary {
            session_id,
            start_timestamp,
            end_timestamp,
            total_cost_usd,
            success,
            tool_calls_count,
            approvals_count,
            denies_count,
        });
    }
    summaries.sort_by(|a, b| b.end_timestamp.cmp(&a.end_timestamp));
    summaries.truncate(100);
    Ok(summaries)
}

/// Read events from a session log file with bounded memory.
/// - Small files: read fully.
/// - Large files: read head + tail only (summary is approximate).
fn read_session_events_bounded(path: &Path, file_size: u64) -> Result<Vec<LogEvent>, String> {
    let threshold = SUMMARY_HEAD_READ_BYTES + SUMMARY_TAIL_READ_BYTES;
    if file_size <= threshold {
        read_session_events_full(path)
    } else {
        read_session_events_head_tail(path, file_size)
    }
}

/// Read the entire session file (small files only).
fn read_session_events_full(path: &Path) -> Result<Vec<LogEvent>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_jsonl_lines(&content)
}

/// Read only head + tail of a large session file.
/// The first line of the tail chunk may be a fragment (cut mid-line); parse_jsonl_lines skips it.
fn read_session_events_head_tail(path: &Path, file_size: u64) -> Result<Vec<LogEvent>, String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;

    let mut head_buf = vec![0u8; SUMMARY_HEAD_READ_BYTES as usize];
    let head_len = f.read(&mut head_buf).map_err(|e| e.to_string())?;
    head_buf.truncate(head_len);
    let head_str = String::from_utf8_lossy(&head_buf);

    let tail_start = file_size.saturating_sub(SUMMARY_TAIL_READ_BYTES);
    f.seek(SeekFrom::Start(tail_start)).map_err(|e| e.to_string())?;
    let mut tail_buf = vec![0u8; (file_size - tail_start) as usize];
    f.read_exact(&mut tail_buf).map_err(|e| e.to_string())?;
    let tail_str = String::from_utf8_lossy(&tail_buf);

    let mut head_events = parse_jsonl_lines(&head_str)?;
    let tail_events = parse_jsonl_lines(&tail_str)?;
    head_events.extend(tail_events);
    Ok(head_events)
}

/// Parse non-empty lines as LogEvent, skipping malformed lines.
fn parse_jsonl_lines(content: &str) -> Result<Vec<LogEvent>, String> {
    let mut events = Vec::new();
    for line in content.lines().filter(|s| !s.trim().is_empty()) {
        if let Ok(ev) = serde_json::from_str::<LogEvent>(line.trim()) {
            events.push(ev);
        }
    }
    Ok(events)
}

/// Load one session's log events from app_config_dir/logs/{session_id}.jsonl.
///
/// Returns at most `MAX_EVENTS_PER_SESSION_LOG` (2000) events, preferring the **latest** (tail).
/// Uses line-by-line streaming via `BufReader` so the full file is never loaded into memory.
/// Corrupted or partially written files: skips bad lines, returns best-effort parsed events.
pub fn load_session_log_from_dir(
    app_config_dir: &Path,
    session_id: &str,
) -> Result<Vec<LogEvent>, String> {
    let safe = sanitize_session_id_for_filename(session_id);
    let name = if safe.is_empty() { "default" } else { &safe };
    let path = app_config_dir.join(LOG_DIR).join(format!("{}.jsonl", name));
    let file = std::fs::File::open(&path).map_err(|e| format!("open {:?}: {}", path, e))?;
    let reader = BufReader::new(file);

    // Ring buffer: we keep only the last MAX_EVENTS events. Peak memory = O(MAX_EVENTS).
    let mut tail: VecDeque<LogEvent> = VecDeque::with_capacity(MAX_EVENTS_PER_SESSION_LOG + 1);
    let mut skipped_lines: u32 = 0;

    for line in reader.lines() {
        let line = line.map_err(|e| format!("read {:?}: {}", path, e))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<LogEvent>(line) {
            Ok(ev) => {
                tail.push_back(ev);
                if tail.len() > MAX_EVENTS_PER_SESSION_LOG {
                    tail.pop_front();
                }
            }
            Err(_) => {
                skipped_lines += 1;
            }
        }
    }

    if skipped_lines > 0 {
        eprintln!(
            "[telemetry] load_session_log {:?}: skipped {} malformed line(s)",
            path, skipped_lines
        );
    }

    Ok(tail.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_sanitized_for_filename() {
        assert_eq!(sanitize_session_id_for_filename("abc-123"), "abc-123");
        assert_eq!(sanitize_session_id_for_filename("a/b/c"), "a_b_c");
        assert_eq!(sanitize_session_id_for_filename(""), "");
    }

    #[test]
    fn log_event_writes_both_files_and_rotation_keeps_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        std::fs::create_dir_all(app_config.join(LOG_DIR)).unwrap();

        let metrics = LogMetrics {
            tokens_in: Some(10),
            tokens_out: Some(20),
            latency_ms: Some(100),
            cost_usd: None,
        };

        log_event(
            app_config,
            "sess-1",
            Some("req-1"),
            "agent",
            "semantic_router",
            "classify",
            Some("fallback"),
            metrics,
            serde_json::json!({"intent": "edit"}),
        )
        .unwrap();

        let session_path = session_log_path(app_config, "sess-1");
        let telemetry_path = telemetry_path(app_config);
        assert!(session_path.exists());
        assert!(telemetry_path.exists());

        let session_content = std::fs::read_to_string(&session_path).unwrap();
        assert!(session_content.contains("\"session_id\":\"sess-1\""));
        assert!(session_content.contains("\"component\":\"semantic_router\""));
        assert!(session_content.contains("\"action\":\"classify\""));
        assert!(session_content.contains("\"decision\":\"fallback\""));
        assert!(session_content.contains("\"intent\":\"edit\""));

        let telemetry_content = std::fs::read_to_string(&telemetry_path).unwrap();
        assert!(telemetry_content.contains("\"session_id\":\"sess-1\""));
    }

    /// Serialization omits None metrics (skip_serializing_if).
    #[test]
    fn log_event_serializes_without_optional_metrics() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        std::fs::create_dir_all(app_config.join(LOG_DIR)).unwrap();

        log_event(
            app_config,
            "sess",
            None,
            "agent",
            "test",
            "ping",
            None,
            LogMetrics::default(),
            serde_json::json!({}),
        )
        .unwrap();

        let session_path = session_log_path(app_config, "sess");
        let content = std::fs::read_to_string(&session_path).unwrap();
        let line = content.lines().next().unwrap();
        // None metrics should be omitted from JSON
        assert!(!line.contains("\"tokens_in\""), "tokens_in should be omitted when None");
        assert!(!line.contains("\"tokens_out\""), "tokens_out should be omitted when None");
        assert!(!line.contains("\"latency_ms\""), "latency_ms should be omitted when None");
        assert!(!line.contains("\"cost_usd\""), "cost_usd should be omitted when None");
        let parsed: LogEvent = serde_json::from_str(line).unwrap();
        assert_eq!(parsed.component, "test");
        assert_eq!(parsed.action, "ping");
        assert!(parsed.metrics.tokens_in.is_none());
        assert!(parsed.metrics.cost_usd.is_none());
    }

    /// One JSON line per log_event call.
    #[test]
    fn log_event_writes_one_line_per_call() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        std::fs::create_dir_all(app_config.join(LOG_DIR)).unwrap();

        for i in 0..3 {
            log_event(
                app_config,
                "multi",
                None,
                "agent",
                "test",
                "step",
                Some("ok"),
                LogMetrics {
                    tokens_in: Some(i * 10),
                    ..Default::default()
                },
                serde_json::json!({ "index": i }),
            )
            .unwrap();
        }

        let session_path = session_log_path(app_config, "multi");
        let content = std::fs::read_to_string(&session_path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|s| !s.is_empty()).collect();
        assert_eq!(lines.len(), 3, "expected one line per log_event call");
        for (i, line) in lines.iter().enumerate() {
            let ev: LogEvent = serde_json::from_str(line).unwrap();
            assert_eq!(ev.details.get("index").and_then(|v| v.as_u64()), Some(i as u64));
        }
    }

    /// load_session_log returns at most MAX_EVENTS and prefers the latest (tail).
    #[test]
    fn load_session_log_caps_and_returns_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        std::fs::create_dir_all(app_config.join(LOG_DIR)).unwrap();

        // Write 2500 events
        for i in 0..2500 {
            log_event(
                app_config,
                "cap-test",
                None,
                "agent",
                "test",
                "step",
                Some("ok"),
                LogMetrics::default(),
                serde_json::json!({ "index": i }),
            )
            .unwrap();
        }

        let events = load_session_log_from_dir(app_config, "cap-test").unwrap();
        assert_eq!(
            events.len(),
            MAX_EVENTS_PER_SESSION_LOG,
            "should return at most {} events",
            MAX_EVENTS_PER_SESSION_LOG
        );
        // Should be the latest 2000 (indices 500..2500)
        let first_idx = events.first().and_then(|e| e.details.get("index").and_then(|v| v.as_u64()));
        let last_idx = events.last().and_then(|e| e.details.get("index").and_then(|v| v.as_u64()));
        assert_eq!(first_idx, Some(500), "first event should be index 500 (tail)");
        assert_eq!(last_idx, Some(2499), "last event should be index 2499");
    }

    /// load_session_log skips malformed lines and returns best-effort.
    #[test]
    fn load_session_log_skips_malformed_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        std::fs::create_dir_all(app_config.join(LOG_DIR)).unwrap();

        let path = session_log_path(app_config, "malformed-test");
        let valid = r#"{"timestamp":"2024-01-15T00:00:00Z","session_id":"x","caller":"x","component":"x","action":"x","metrics":{},"details":{}}
{ not valid json }
{"timestamp":"2024-01-15T00:00:01Z","session_id":"x","caller":"x","component":"x","action":"x","metrics":{},"details":{}}
"#;
        std::fs::write(&path, valid).unwrap();

        let events = load_session_log_from_dir(app_config, "malformed-test").unwrap();
        assert_eq!(events.len(), 2, "should have 2 valid events, skip malformed line");
    }

    /// list_sessions reads head+tail for files larger than SUMMARY_HEAD+TAIL threshold.
    #[test]
    fn list_sessions_head_tail_for_large_file() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        std::fs::create_dir_all(app_config.join(LOG_DIR)).unwrap();

        // Create a file > SUMMARY_HEAD_READ_BYTES + SUMMARY_TAIL_READ_BYTES (~768 KB)
        let path = session_log_path(app_config, "large-session");
        let padding: String = "x".repeat(500); // ~500 bytes per event
        let mut lines = Vec::new();
        for i in 0..2000 {
            let ev = serde_json::json!({
                "timestamp": "2024-01-15T00:00:00Z",
                "session_id": "large-session",
                "caller": "test",
                "component": "test",
                "action": "step",
                "metrics": {},
                "details": { "i": i, "pad": padding }
            });
            lines.push(serde_json::to_string(&ev).unwrap());
        }
        std::fs::write(&path, lines.join("\n")).unwrap();

        let summaries = list_sessions_from_dir(app_config).unwrap();
        let s = summaries.iter().find(|s| s.session_id == "large-session").unwrap();
        assert!(!s.start_timestamp.is_empty());
        assert!(!s.end_timestamp.is_empty());
    }

    /// list_sessions returns summaries for small session files.
    #[test]
    fn list_sessions_bounded_read_small_file() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        std::fs::create_dir_all(app_config.join(LOG_DIR)).unwrap();

        log_event(
            app_config,
            "list-bounded",
            None,
            "agent",
            "orchestrator",
            "session_complete",
            Some("success"),
            LogMetrics::default(),
            serde_json::json!({ "outcome": "success" }),
        )
        .unwrap();

        let summaries = list_sessions_from_dir(app_config).unwrap();
        let s = summaries.iter().find(|s| s.session_id == "list-bounded").unwrap();
        assert!(s.success);
    }

    /// Policy decision log shape: rule_id and decision in details (security metrics).
    #[test]
    fn log_event_policy_decision_contains_rule_id_and_decision() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        std::fs::create_dir_all(app_config.join(LOG_DIR)).unwrap();

        let details = serde_json::json!({
            "tool": "fs",
            "operation": "read",
            "workspace_id": "ws1",
            "rule_id": "EXPLICIT_ALLOW",
            "rule_reason": "EXPLICIT_ALLOW",
        });
        log_event(
            app_config,
            "policy-test",
            None,
            "agent",
            "security",
            "policy_evaluate",
            Some("allow"),
            LogMetrics::default(),
            details,
        )
        .unwrap();

        let session_path = session_log_path(app_config, "policy-test");
        let content = std::fs::read_to_string(&session_path).unwrap();
        let ev: LogEvent = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(ev.component, "security");
        assert_eq!(ev.action, "policy_evaluate");
        assert_eq!(ev.decision.as_deref(), Some("allow"));
        assert_eq!(ev.details.get("rule_id").and_then(|v| v.as_str()), Some("EXPLICIT_ALLOW"));
        assert_eq!(ev.details.get("rule_reason").and_then(|v| v.as_str()), Some("EXPLICIT_ALLOW"));
    }
}
