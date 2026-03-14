//! Persist AgentSessionLog to logs/agent_sessions.jsonl with 30-day/10MB retention.
//! One JSON object per line; retention reuses the same policy as events.log.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use oxcer_core::agent_session_log::AgentSessionLog;

const LOG_DIR: &str = "logs";
const AGENT_SESSIONS_FILENAME: &str = "agent_sessions.jsonl";
const MAX_AGE_DAYS: i64 = 30;
const MAX_BYTES: u64 = 10 * 1024 * 1024; // 10MB

fn log_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join(LOG_DIR).join(AGENT_SESSIONS_FILENAME)
}

/// Append one completed session log as a single JSON line, then apply retention.
pub fn append_session_log(app_config_dir: &Path, log: &AgentSessionLog) -> Result<(), String> {
    let dir = app_config_dir.join(LOG_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = log_path(app_config_dir);

    let line = serde_json::to_string(log).map_err(|e| e.to_string())?;

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{}", line).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;

    rotate_if_needed(app_config_dir)?;
    Ok(())
}

/// Retention: drop entries with completed_at older than MAX_AGE_DAYS, then drop oldest until under MAX_BYTES.
fn rotate_if_needed(app_config_dir: &Path) -> Result<(), String> {
    let path = log_path(app_config_dir);
    let cutoff = chrono::Utc::now() - chrono::Duration::days(MAX_AGE_DAYS);
    rotate_retention(&path, cutoff)
}

/// Keep lines with completed_at >= cutoff, then keep newest up to MAX_BYTES.
pub fn rotate_retention(path: &Path, cutoff: chrono::DateTime<chrono::Utc>) -> Result<(), String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(()),
    };
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader
        .lines()
        .map_while(|r| r.ok())
        .filter(|s| !s.is_empty())
        .collect();
    if lines.is_empty() {
        return Ok(());
    }

    let mut with_ts: Vec<(chrono::DateTime<chrono::Utc>, String)> = Vec::new();
    for line in &lines {
        if let Ok(entry) = serde_json::from_str::<AgentSessionLog>(line) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&entry.completed_at) {
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
    std::fs::write(path, out).map_err(|e| e.to_string())?;
    Ok(())
}
