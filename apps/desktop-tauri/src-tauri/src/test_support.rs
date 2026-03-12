//! Test-only app builder for integration tests.
//!
//! Compiled only with `--features test`. Uses Tauri's mock context so no
//! window or native runtime is required.

use std::sync::Mutex;

use tauri::test::{mock_context, noop_assets};
use tauri::Manager;

use crate::router::PendingApprovalsStore;
use crate::settings::{load as settings_load, AppSettings};

/// Builds a minimal Tauri app with the same state (settings, pending approvals)
/// as the real app, using a mock context and temp config dir.
///
/// Use this in `tests/workspace_cleanup.rs` to exercise `workspace_cleanup_on_delete`
/// without running the full app or event loop.
pub fn build_test_app() -> tauri::App {
    let context = mock_context(noop_assets());
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(PendingApprovalsStore::new())
        .manage(Mutex::new(AppSettings::default()))
        .setup(|app| {
            let handle = app.handle();
            let app_config_dir = handle.path().app_config_dir().expect("app_config_dir");
            std::fs::create_dir_all(&app_config_dir).expect("create config dir");
            let loaded = settings_load(&app_config_dir);
            if let Some(state) = handle.try_state::<Mutex<AppSettings>>() {
                let mut guard = state.lock().expect("settings lock");
                *guard = loaded;
            }
            Ok(())
        })
        .build(context)
        .expect("build test app")
}
