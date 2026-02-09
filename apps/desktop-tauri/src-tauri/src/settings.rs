//! Settings / config.json — SSOT for workspaces, default_model, security options.
//! Migrates from legacy settings.json if present.
//!
//! ## Config file schema (Sprint 5 — config.json)
//!
//! File location: `{appdata}/Oxcer/config.json` (e.g. macOS `~/Library/Application Support/Oxcer/config.json`).
//!
//! ```json
//! {
//!   "security": { "destructive_fs": { "enabled": false } },
//!   "workspaces": [
//!     { "id": "uuid", "name": "My Project", "root_path": "/path/to/dir" }
//!   ],
//!   "model": { "default_id": "gemini-2.5-flash" }
//! }
//! ```
//!
//! ## UI control ↔ config field mapping (1:1)
//!
//! | UI control | config.json field | type |
//! |------------|--------------------|------|
//! | "Allow destructive file operations" checkbox (Advanced) | `security.destructive_fs.enabled` | bool |
//! | Workspace list item id | `workspaces[].id` | string |
//! | Workspace list item display name | `workspaces[].name` | string |
//! | Workspace list item path | `workspaces[].root_path` | string |
//! | Default model dropdown | `model.default_id` | string |
//!
//! Backward compatibility: we also read `fs.destructive_operations_enabled` and
//! top-level `default_model` if the new keys are absent.

use std::path::{Path, PathBuf};

use oxcer_core::fs;
use serde::{Deserialize, Serialize};

const CONFIG_FILENAME: &str = "config.json";
const LEGACY_FILENAME: &str = "settings.json";
const DEFAULT_MODEL: &str = "gemini-2.5-flash";

/// Workspace as stored in config.json (id, name, root_path per Sprint 5 spec).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigWorkspace {
    pub id: String,
    /// Display name. Defaults derived from path if missing in file.
    #[serde(default)]
    pub name: String,
    #[serde(rename = "root_path")]
    pub root_path: String,
}

/// User-registered workspace directory (in-memory; maps to workspaces[] in config).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceDirectory {
    pub id: String,
    pub name: String,
    pub path: String,
}

impl From<ConfigWorkspace> for WorkspaceDirectory {
    fn from(w: ConfigWorkspace) -> Self {
        let name = if w.name.is_empty() {
            PathBuf::from(&w.root_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Workspace")
                .to_string()
        } else {
            w.name
        };
        Self {
            id: w.id,
            name,
            path: w.root_path,
        }
    }
}

/// Advanced / dangerous options (all off by default).
/// In-memory only; persisted as security.destructive_fs.enabled in config.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AdvancedSettings {
    /// Maps to config: security.destructive_fs.enabled
    #[serde(default)]
    pub allow_destructive_fs_without_hitl: bool,
}

/// Observability / metrics options (Sprint 8 §6).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservabilityOptions {
    /// Max session LLM cost (USD) before alerting. Default 0.5.
    #[serde(default = "default_max_session_cost_usd")]
    pub max_session_cost_usd: f64,
}

fn default_max_session_cost_usd() -> f64 {
    0.5
}

impl Default for ObservabilityOptions {
    fn default() -> Self {
        Self {
            max_session_cost_usd: default_max_session_cost_usd(),
        }
    }
}

/// Application settings (in-memory view of config.json).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppSettings {
    /// Maps to config: workspaces[]
    #[serde(default)]
    pub workspace_directories: Vec<WorkspaceDirectory>,
    /// Maps to config: model.default_id
    #[serde(default)]
    pub default_model_id: String,
    #[serde(default)]
    pub advanced: AdvancedSettings,
    #[serde(default)]
    pub observability: ObservabilityOptions,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            workspace_directories: Vec::new(),
            default_model_id: DEFAULT_MODEL.to_string(),
            advanced: AdvancedSettings::default(),
            observability: ObservabilityOptions::default(),
        }
    }
}

/// Raw config file structure for JSON (Sprint 5 spec).
/// Supports both new keys (security.*, model.default_id, workspaces[].name) and legacy (fs.*, default_model).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default)]
    pub security: SecurityOptions,
    #[serde(default)]
    pub workspaces: Vec<ConfigWorkspace>,
    #[serde(default)]
    pub model: ModelOptions,
    #[serde(default)]
    pub observability: ObservabilityOptions,
    // Legacy keys (read-only for migration)
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub fs: FsOptions,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SecurityOptions {
    #[serde(default)]
    pub destructive_fs: DestructiveFsOption,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DestructiveFsOption {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelOptions {
    /// Default model id for UI dropdown; not yet used for routing (Semantic Router in later sprint).
    #[serde(rename = "default_id", default)]
    pub default_id: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FsOptions {
    #[serde(default)]
    pub destructive_operations_enabled: bool,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            security: SecurityOptions::default(),
            workspaces: Vec::new(),
            model: ModelOptions::default(),
            observability: ObservabilityOptions::default(),
            default_model: DEFAULT_MODEL.to_string(),
            fs: FsOptions::default(),
        }
    }
}

fn config_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join(CONFIG_FILENAME)
}

fn legacy_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join(LEGACY_FILENAME)
}

const SETTINGS_CHANGES_LOG: &str = "settings_changes.log";

/// Log entry for fs.destructive_operations_enabled changes.
#[derive(serde::Serialize)]
struct DestructiveSettingLogEntry {
    timestamp: String,
    actor: &'static str,
    setting: &'static str,
    #[serde(rename = "from")]
    from_value: bool,
    #[serde(rename = "to")]
    to_value: bool,
}

/// Append a log entry when fs.destructive_operations_enabled changes.
/// Log file is next to config.json (settings_changes.log).
pub fn log_destructive_setting_change(
    app_config_dir: &Path,
    from: bool,
    to: bool,
) -> Result<(), String> {
    let log_path = app_config_dir.join(SETTINGS_CHANGES_LOG);
    let entry = DestructiveSettingLogEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        actor: "local_user",
        setting: "fs.destructive_operations_enabled",
        from_value: from,
        to_value: to,
    };
    let line = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{}", line).map_err(|e| e.to_string())?;
    Ok(())
}

/// Load settings from config.json. Migrates from settings.json if config.json doesn't exist.
/// Reads both new schema (security.destructive_fs.enabled, model.default_id, workspaces[].name) and legacy (fs.*, default_model).
pub fn load(app_config_dir: &Path) -> AppSettings {
    let config_path = config_path(app_config_dir);
    if let Ok(s) = std::fs::read_to_string(&config_path) {
        if let Ok(cfg) = serde_json::from_str::<ConfigFile>(&s) {
            let workspace_directories = cfg
                .workspaces
                .into_iter()
                .map(WorkspaceDirectory::from)
                .collect();
            let default_id = if !cfg.model.default_id.is_empty() {
                cfg.model.default_id
            } else if !cfg.default_model.is_empty() {
                cfg.default_model
            } else {
                DEFAULT_MODEL.to_string()
            };
            let destructive = cfg.security.destructive_fs.enabled
                || cfg.fs.destructive_operations_enabled;
            return AppSettings {
                workspace_directories,
                default_model_id: default_id,
                advanced: AdvancedSettings {
                    allow_destructive_fs_without_hitl: destructive,
                },
                observability: cfg.observability.clone(),
            };
        }
    }

    // Migrate from legacy settings.json
    let legacy_path = legacy_path(app_config_dir);
    if let Ok(s) = std::fs::read_to_string(&legacy_path) {
        if let Ok(legacy) = serde_json::from_str::<AppSettings>(&s) {
            let _ = save(app_config_dir, &legacy);
            let _ = std::fs::remove_file(&legacy_path);
            return legacy;
        }
    }

    AppSettings::default()
}

/// Save settings to config.json (Sprint 5 schema: security.*, model.default_id, workspaces[].name).
pub fn save(app_config_dir: &Path, settings: &AppSettings) -> Result<(), String> {
    let config_path = config_path(app_config_dir);
    let workspaces: Vec<ConfigWorkspace> = settings
        .workspace_directories
        .iter()
        .map(|w| ConfigWorkspace {
            id: w.id.clone(),
            name: w.name.clone(),
            root_path: w.path.clone(),
        })
        .collect();
    let cfg = ConfigFile {
        security: SecurityOptions {
            destructive_fs: DestructiveFsOption {
                enabled: settings.advanced.allow_destructive_fs_without_hitl,
            },
        },
        workspaces,
        model: ModelOptions {
            default_id: settings.default_model_id.clone(),
        },
        observability: settings.observability.clone(),
        default_model: settings.default_model_id.clone(),
        fs: FsOptions {
            destructive_operations_enabled: settings.advanced.allow_destructive_fs_without_hitl,
        },
    };
    let s = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&config_path, s).map_err(|e| e.to_string())
}

/// Paths that must never be registered as workspaces:
/// - root (`/`), HOME (`~`), or any parent of HOME.
pub fn is_forbidden_workspace_path(path: &Path) -> bool {
    let path = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => path.to_path_buf(),
    };
    // Never allow filesystem root
    if path == Path::new("/") {
        return true;
    }
    let Some(home) = dirs_next::home_dir() else {
        return false;
    };
    let home = match home.canonicalize() {
        Ok(h) => h,
        Err(_) => home,
    };
    path == home || path.ancestors().any(|a| a == home)
}

/// Effective FS policy for the Security Policy Engine.
/// Computed from config.json on startup and whenever settings change.
#[derive(Clone, Debug, serde::Serialize)]
pub struct EffectiveFsPolicy {
    /// Allowed workspace root paths (canonical where possible).
    pub allowed_workspaces: Vec<String>,
    /// Whether destructive operations (delete, rename, move) are enabled by user config.
    pub destructive_operations_enabled: bool,
}

/// Compute effective FS policy from app settings.
pub fn get_effective_fs_policy(settings: &AppSettings) -> EffectiveFsPolicy {
    let allowed_workspaces = settings
        .workspace_directories
        .iter()
        .map(|w| w.path.clone())
        .collect();
    EffectiveFsPolicy {
        allowed_workspaces,
        destructive_operations_enabled: settings.advanced.allow_destructive_fs_without_hitl,
    }
}

/// Convert workspace directories to oxcer_core FS roots.
pub fn to_workspace_roots(dirs: &[WorkspaceDirectory]) -> Vec<fs::WorkspaceRoot> {
    dirs.iter()
        .map(|w| fs::WorkspaceRoot {
            id: w.id.clone(),
            name: w.name.clone(),
            path: PathBuf::from(&w.path),
        })
        .collect()
}
