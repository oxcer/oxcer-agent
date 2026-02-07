#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use tauri::{AppHandle, Manager};
use tauri_plugin_fs;

use oxcer_core::fs;
use oxcer_core::shell;

/// Helper to build the FS context from the running application handle.
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

/// Shell Service: run a catalog-defined command by id with validated params.
/// No free-form shell; only commands from the default catalog are allowed.
#[tauri::command]
fn cmd_shell_run(
    app: AppHandle,
    workspace_root: String,
    command_id: String,
    params: serde_json::Value,
) -> Result<shell::ShellResult, shell::ShellError> {
    let ctx = shell::ShellContext {
        workspace_roots: vec![fs::WorkspaceRoot {
            id: "default".to_string(),
            name: "default".to_string(),
            path: PathBuf::from(&workspace_root),
        }],
        default_workspace_id: "default".to_string(),
    };
    let catalog = shell::default_catalog();
    shell::shell_run(
        shell::ShellCaller::Ui,
        &ctx,
        &catalog,
        &command_id,
        params,
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

/// Returns the Tauri context for the app. In test builds we use a mock so that
/// `cargo test` can compile without OUT_DIR / generate_context!(); real runs
/// use the bundled context. See docs/DEVELOPMENT.md for workflow notes.
fn app_context() -> tauri::Context<tauri::Wry> {
    #[cfg(test)]
    {
        tauri::test::mock_context(tauri::test::noop_assets())
    }

    #[cfg(not(test))]
    {
        tauri::generate_context!()
    }
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
            let handle = app.handle();
            let ctx = build_fs_context(&handle);
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
            cmd_fs_write_file,
            cmd_shell_run
        ])
        .run(app_context())
        .expect("error while running Oxcer Tauri application");
}

