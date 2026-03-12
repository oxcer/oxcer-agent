//! Command helpers used by the binary and by integration tests.
//!
//! This module lives in the lib so that `workspace_cleanup_on_delete` can be
//! exercised from `tests/workspace_cleanup.rs` without duplicating logic.

use std::path::Path;
use std::sync::Mutex;

use tauri::Emitter;
use tauri::Manager;

use crate::event_log;
use crate::router::PendingApprovalsStore;
use crate::settings::{self, AppSettings};

/// Core workspace cleanup: remove from state, cancel pending approvals, save config, log event.
/// Call this from tests with temp state/store; the Tauri command uses `workspace_cleanup_on_delete` which pulls state from the app and then calls this.
pub fn workspace_cleanup_impl(
    app_config_dir: &Path,
    state: &Mutex<AppSettings>,
    store: Option<&PendingApprovalsStore>,
    workspace_id: &str,
    emit: impl FnOnce(&str),
) -> Result<(), String> {
    let root_path = {
        let guard = state.lock().expect("settings lock");
        guard
            .workspace_directories
            .iter()
            .find(|w| w.id == workspace_id)
            .map(|w| w.path.clone())
            .ok_or_else(|| "Workspace not found".to_string())?
    };

    if let Some(s) = store {
        s.cancel_pending_for_workspace_root(&root_path);
    }

    let mut guard = state.lock().expect("settings lock");
    guard.workspace_directories.retain(|w| w.id != workspace_id);
    settings::save(app_config_dir, &guard)?;

    let _ = event_log::append(
        app_config_dir,
        "workspace_removed",
        Some(workspace_id),
        Some(&serde_json::json!({ "root_path": root_path })),
    );

    emit(workspace_id);

    Ok(())
}

/// Single service for workspace deletion: cancel sessions, remove from config, event log.
/// Policy allowlist is derived from settings so it updates automatically after save.
/// Any future workspace-scoped cache/index should be cleared here.
pub fn workspace_cleanup_on_delete(
    app: &tauri::AppHandle,
    workspace_id: &str,
) -> Result<(), String> {
    let app_config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let state = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let store = app.try_state::<PendingApprovalsStore>();
    workspace_cleanup_impl(
        &app_config_dir,
        &state,
        store.as_deref(),
        workspace_id,
        |id| {
            let _ = app.emit("workspace_deleted", id);
        },
    )
}
