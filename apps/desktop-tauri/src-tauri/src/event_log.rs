//! Durable event log for high-risk/advanced actions (Sprint 5).
//!
//! Location: `{appdata}/Oxcer/logs/events.log`. In-memory toasts are for immediate
//! UX feedback only; durable history is always read from this file.
//!
//! Retention: keep entries for at least 30 days or up to 10MB, whichever comes first.
//! After that, rotate by removing/overwriting oldest entries.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

const LOG_DIR: &str = "logs";
const LOG_FILENAME: &str = "events.log";
const MAX_AGE_DAYS: i64 = 30;
const MAX_BYTES: u64 = 10 * 1024 * 1024; // 10MB

/// One line in events.log: JSON with timestamp, event_type, workspace_id, details.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct EventEntry {
    pub timestamp: String,
    pub event_type: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
}

fn log_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join(LOG_DIR).join(LOG_FILENAME)
}

/// Append one event to logs/events.log, then apply retention (30 days or 10MB).
pub fn append(
    app_config_dir: &Path,
    event_type: &str,
    workspace_id: Option<&str>,
    details: Option<&serde_json::Value>,
) -> Result<(), String> {
    let dir = app_config_dir.join(LOG_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = log_path(app_config_dir);

    let entry = EventEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        event_type: event_type.to_string(),
        workspace_id: workspace_id.map(String::from),
        details: details.cloned(),
    };
    let line = serde_json::to_string(&entry).map_err(|e| e.to_string())?;

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

/// Retention: drop entries older than MAX_AGE_DAYS, then drop oldest until under MAX_BYTES.
fn rotate_if_needed(app_config_dir: &Path) -> Result<(), String> {
    let path = log_path(app_config_dir);
    let cutoff = chrono::Utc::now() - chrono::Duration::days(MAX_AGE_DAYS);
    rotate_retention(&path, cutoff)
}

/// Core retention: keep entries with timestamp >= cutoff, then keep newest entries up to MAX_BYTES (drop oldest first).
/// Used by rotate_if_needed and by unit/integration tests with a fixed cutoff for determinism.
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
        if let Ok(entry) = serde_json::from_str::<EventEntry>(line) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&entry.timestamp) {
                let utc = dt.with_timezone(&chrono::Utc);
                if utc >= cutoff {
                    with_ts.push((utc, line.clone()));
                }
            }
        }
    }
    with_ts.sort_by_key(|(t, _)| *t);

    // Keep newest entries that fit in MAX_BYTES (drop oldest first).
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        ts: chrono::DateTime<chrono::Utc>,
        event_type: &str,
        details: Option<serde_json::Value>,
    ) -> String {
        let entry = EventEntry {
            timestamp: ts.to_rfc3339(),
            event_type: event_type.to_string(),
            workspace_id: None,
            details,
        };
        serde_json::to_string(&entry).unwrap()
    }

    #[test]
    fn log_path_joins_correctly() {
        let p = log_path(Path::new("/tmp/app"));
        assert!(p.to_string_lossy().contains("logs"));
        assert!(p.to_string_lossy().contains("events.log"));
    }

    /// Age-based trimming: entries older than 30 days are removed.
    #[test]
    fn rotation_removes_entries_older_than_30_days() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        let dir = app_config.join(LOG_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let path = log_path(app_config);

        let now = chrono::Utc::now();
        let ts_40d = now - chrono::Duration::days(40);
        let ts_31d = now - chrono::Duration::days(31);
        let ts_10d = now - chrono::Duration::days(10);
        let cutoff = now - chrono::Duration::days(MAX_AGE_DAYS);

        let lines = [
            make_entry(ts_40d, "old_40", Some(serde_json::json!({"marker": "40d"}))),
            make_entry(ts_31d, "old_31", Some(serde_json::json!({"marker": "31d"}))),
            make_entry(ts_10d, "recent", Some(serde_json::json!({"marker": "10d"}))),
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();

        rotate_retention(&path, cutoff).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let kept: Vec<&str> = content.lines().filter(|s| !s.is_empty()).collect();
        assert_eq!(kept.len(), 1, "only entry newer than 30 days should remain");
        assert!(
            kept[0].contains("\"marker\":\"10d\""),
            "the 10d entry should be kept"
        );
        assert!(!content.contains("\"marker\":\"40d\""));
        assert!(!content.contains("\"marker\":\"31d\""));
    }

    /// Size-based trimming: file over 10MB is reduced to ≤10MB by dropping oldest entries; newest remain.
    #[test]
    fn rotation_trims_to_10mb_keeping_newest() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        let dir = app_config.join(LOG_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let path = log_path(app_config);

        let now = chrono::Utc::now();
        let cutoff = now - chrono::Duration::days(MAX_AGE_DAYS);
        let line_size = 300usize;
        let target_bytes = MAX_BYTES as usize + 1024 * 1024;
        let n_lines = target_bytes / line_size;
        let mut lines = Vec::with_capacity(n_lines);
        for i in 0..n_lines {
            let ts = now - chrono::Duration::seconds(i as i64);
            let entry = EventEntry {
                timestamp: ts.to_rfc3339(),
                event_type: "dummy".to_string(),
                workspace_id: None,
                details: Some(
                    serde_json::json!({ "index": i, "padding": "x".repeat(line_size - 80) }),
                ),
            };
            lines.push(serde_json::to_string(&entry).unwrap());
        }
        let content = lines.join("\n");
        assert!(content.len() as u64 > MAX_BYTES);
        std::fs::write(&path, content).unwrap();

        rotate_retention(&path, cutoff).unwrap();

        let meta = std::fs::metadata(&path).unwrap();
        assert!(
            meta.len() <= MAX_BYTES,
            "file must be ≤10MB after rotation, got {} bytes",
            meta.len()
        );

        let after = std::fs::read_to_string(&path).unwrap();
        let parsed: Vec<EventEntry> = after
            .lines()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect();
        let indices: Vec<i64> = parsed
            .iter()
            .filter_map(|e| {
                e.details
                    .as_ref()
                    .and_then(|d| d.get("index"))
                    .and_then(|v| v.as_i64())
            })
            .collect();
        assert!(!indices.is_empty());
        let min_kept = *indices.iter().min().unwrap();
        let max_kept = *indices.iter().max().unwrap();
        assert_eq!(min_kept, 0, "newest entry (index 0) should be kept");
        assert!(
            max_kept < (n_lines as i64) - 1,
            "oldest entries should have been dropped"
        );
    }

    /// Age trimming even when file is under 10MB: entries older than 30 days are still removed.
    #[test]
    fn rotation_removes_old_entries_even_when_under_10mb() {
        let tmp = tempfile::tempdir().unwrap();
        let app_config = tmp.path();
        let dir = app_config.join(LOG_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let path = log_path(app_config);

        let now = chrono::Utc::now();
        let ts_40d = now - chrono::Duration::days(40);
        let ts_35d = now - chrono::Duration::days(35);
        let ts_10d = now - chrono::Duration::days(10);
        let cutoff = now - chrono::Duration::days(MAX_AGE_DAYS);

        let lines = [
            make_entry(ts_40d, "old_40", Some(serde_json::json!({"marker": "40d"}))),
            make_entry(ts_35d, "old_35", Some(serde_json::json!({"marker": "35d"}))),
            make_entry(ts_10d, "recent", Some(serde_json::json!({"marker": "10d"}))),
        ];
        let content = lines.join("\n");
        assert!((content.len() as u64) < MAX_BYTES);
        std::fs::write(&path, content).unwrap();

        rotate_retention(&path, cutoff).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        let kept: Vec<&str> = after.lines().filter(|s| !s.is_empty()).collect();
        assert_eq!(kept.len(), 1);
        assert!(kept[0].contains("\"marker\":\"10d\""));
        assert!(!after.contains("\"marker\":\"40d\""));
        assert!(!after.contains("\"marker\":\"35d\""));
    }
}
