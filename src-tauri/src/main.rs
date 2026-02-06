#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_fs;

mod fs;

/// Helper to build the FS context from the running application handle.
///
/// For Sprint 2 the workspace roots list is minimal and static. In future
/// sprints this will be loaded from persisted app configuration so users
/// can manage workspace roots via Settings UI.
fn build_fs_context(app: &AppHandle) -> fs::AppFsContext {
    let app_config_dir = app
        .path()
        .app_config_dir()
        .expect("app_config_dir should be available");

    fs::AppFsContext {
        app_config_dir,
        workspace_roots: Vec::new(),
    }
}

#[tauri::command]
fn cmd_fs_list_dir(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
) -> Result<Vec<fs::DirEntryMetadata>, fs::FsError> {
    let ctx = fs::AppFsContext {
        app_config_dir: app
            .path()
            .app_config_dir()
            .expect("app_config_dir should be available"),
        workspace_roots: vec![fs::WorkspaceRoot {
            id: "default".to_string(),
            name: "default".to_string(),
            path: PathBuf::from(workspace_root),
        }],
    };

    fs::fs_list_dir(
        fs::FsCaller::Ui,
        &ctx,
        fs::BaseDirKind::Workspace {
            id: "default".to_string(),
        },
        &rel_path,
    )
}

#[tauri::command]
fn cmd_fs_read_file(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
) -> Result<fs::FsReadResult, fs::FsError> {
    let ctx = fs::AppFsContext {
        app_config_dir: app
            .path()
            .app_config_dir()
            .expect("app_config_dir should be available"),
        workspace_roots: vec![fs::WorkspaceRoot {
            id: "default".to_string(),
            name: "default".to_string(),
            path: PathBuf::from(workspace_root),
        }],
    };

    fs::fs_read_file(
        fs::FsCaller::Ui,
        &ctx,
        fs::BaseDirKind::Workspace {
            id: "default".to_string(),
        },
        &rel_path,
    )
}

#[tauri::command]
fn cmd_fs_write_file(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    contents: Vec<u8>,
) -> Result<(), fs::FsError> {
    let ctx = fs::AppFsContext {
        app_config_dir: app
            .path()
            .app_config_dir()
            .expect("app_config_dir should be available"),
        workspace_roots: vec![fs::WorkspaceRoot {
            id: "default".to_string(),
            name: "default".to_string(),
            path: PathBuf::from(workspace_root),
        }],
    };

    fs::fs_write_file(
        fs::FsCaller::Ui,
        &ctx,
        fs::BaseDirKind::Workspace {
            id: "default".to_string(),
        },
        &rel_path,
        &contents,
    )
}

fn main() {
    tauri::Builder::default()
        // NOTE: The FS plugin is initialized with narrow, capability-scoped
        // access. All higher-level filesystem operations must go through our
        // internal FS Service + Security Policy instead of calling std::fs
        // directly, to avoid accidentally exposing sensitive locations like
        // SSH keys, cloud credentials, or keychains.
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            // In the future we may store the FS context in managed state
            // for reuse. For now we just ensure the app config dir exists.
            let ctx = build_fs_context(app);
            std::fs::create_dir_all(&ctx.app_config_dir)
                .expect("failed to create app config directory");
            Ok(())
        })
        // NOTE: FS commands are routed through the FS Service + Security
        // Policy layer. No destructive operations (delete/rename/move) are
        // exposed in Sprint 2.
        .invoke_handler(tauri::generate_handler![
            cmd_fs_list_dir,
            cmd_fs_read_file,
            cmd_fs_write_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running Oxcer Tauri application");
}

