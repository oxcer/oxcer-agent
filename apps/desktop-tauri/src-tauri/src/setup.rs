//! First-run setup wizard: local model download and LLM profile selection.
//!
//! Ensures LLM config (llm_profiles.yaml, models.yaml) exists under app config;
//! reports whether the local model is present; runs download with progress events;
//! persists profile (local-only / hybrid) after setup.

/// Warning shown above external API key inputs. Cannot be hidden; must appear every time the user edits external API settings.
pub const EXTERNAL_API_WARNING: &str = r#"Using external AI APIs sends your requests to the provider's servers.

Do **not** include highly sensitive information (passwords, full credit card numbers, government IDs, medical/health data, etc.) in prompts when external APIs are enabled.

For sensitive data, use the **local-only** mode so data stays on this device."#;

use std::path::Path;

use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;

use oxcer_core::llm::{ensure_model_present, DownloadProgress};

use crate::settings::{self, AppSettings};

/// Default LLM config files (embedded from oxcer-core). Written to app_config_dir/llm_config on first use.
const LLM_PROFILES_YAML: &str = include_str!("../../../../oxcer-core/config/llm_profiles.yaml");
const MODELS_YAML: &str = include_str!("../../../../oxcer-core/config/models.yaml");

/// Ensure app_config_dir/llm_config exists and contains llm_profiles.yaml and models.yaml.
/// Creates the directory and writes default content if files are missing.
pub fn ensure_llm_config_dir(app_config_dir: &Path) -> Result<std::path::PathBuf, String> {
    let dir = app_config_dir.join("llm_config");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create llm_config dir: {}", e))?;

    let profiles_path = dir.join("llm_profiles.yaml");
    if !profiles_path.is_file() {
        std::fs::write(&profiles_path, LLM_PROFILES_YAML)
            .map_err(|e| format!("Failed to write llm_profiles.yaml: {}", e))?;
    }

    let models_path = dir.join("models.yaml");
    if !models_path.is_file() {
        std::fs::write(&models_path, MODELS_YAML)
            .map_err(|e| format!("Failed to write models.yaml: {}", e))?;
    }

    Ok(dir)
}

/// Check if local model (phi3-small) is present: required files exist and are non-empty.
fn local_model_present(models_dir: &Path) -> bool {
    let root = models_dir.join("phi3-small");
    if !root.is_dir() {
        return false;
    }
    let gguf = root.join("model.gguf");
    let tokenizer = root.join("tokenizer.json");
    gguf.is_file()
        && std::fs::metadata(&gguf)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        && tokenizer.is_file()
        && std::fs::metadata(&tokenizer)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
}

/// Status for the setup wizard UI.
#[derive(serde::Serialize)]
pub struct SetupStatus {
    /// Local model files are missing and need to be downloaded.
    pub needs_local_model: bool,
    /// User has completed the setup wizard (downloaded model and chose profile).
    pub setup_complete: bool,
    /// Current or default profile: "local-only" or "hybrid".
    pub profile: String,
}

/// Get setup status for the wizard: whether local model is present, setup complete, and current profile.
pub fn get_setup_status(app: &AppHandle) -> Result<SetupStatus, String> {
    let app_config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e: tauri::Error| e.to_string())?;
    let models_dir = app_config_dir.join("models");
    let needs_local_model = !local_model_present(&models_dir);

    let state = app
        .try_state::<std::sync::Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let settings = state.lock().map_err(
        |e: std::sync::PoisonError<std::sync::MutexGuard<'_, AppSettings>>| e.to_string(),
    )?;
    let setup_complete = settings.llm.setup_complete;
    let profile = settings.llm.profile.clone();

    Ok(SetupStatus {
        needs_local_model,
        setup_complete,
        profile: if profile.is_empty() {
            "local-only".to_string()
        } else {
            profile
        },
    })
}

/// Start downloading the local model in a background thread. Emits:
/// - `llm_download_progress` with `{ file_name, bytes_downloaded, total_bytes }`
/// - `llm_download_complete` with `{ success: bool, error?: string }`
pub fn start_model_download(app: AppHandle) -> Result<(), String> {
    let app_config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e: tauri::Error| e.to_string())?;
    let config_dir = ensure_llm_config_dir(&app_config_dir)?;
    let models_dir = app_config_dir.join("models");
    std::fs::create_dir_all(&models_dir)
        .map_err(|e| format!("Failed to create models dir: {}", e))?;

    std::thread::spawn(move || {
        let app = app.clone();
        let result = ensure_model_present(
            "phi3-small",
            &config_dir,
            &models_dir,
            Some(&|p: DownloadProgress| {
                let _ = app.emit(
                    "llm_download_progress",
                    serde_json::json!({
                        "file_name": p.file_name,
                        "bytes_downloaded": p.bytes_downloaded,
                        "total_bytes": p.total_bytes,
                    }),
                );
            }),
        );

        let (success, error) = match result {
            Ok(_) => (true, None::<String>),
            Err(e) => (false, Some(e.to_string())),
        };
        let _ = app.emit(
            "llm_download_complete",
            serde_json::json!({
                "success": success,
                "error": error,
            }),
        );
    });

    Ok(())
}

/// Mark setup as complete and persist the chosen profile (local-only or hybrid).
pub fn complete_setup(app: &AppHandle, profile: String) -> Result<(), String> {
    let app_config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e: tauri::Error| e.to_string())?;
    let state = app
        .try_state::<std::sync::Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;

    let mut guard = state.lock().map_err(
        |e: std::sync::PoisonError<std::sync::MutexGuard<'_, AppSettings>>| e.to_string(),
    )?;
    guard.llm.setup_complete = true;
    guard.llm.profile = if profile.is_empty() {
        "local-only".to_string()
    } else {
        profile
    };
    settings::save(&app_config_dir, &guard)?;
    Ok(())
}
