#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Oxcer Tauri launcher — Command Router entry point.
//!
//! ## "Agent = untrusted client" contract
//!
//! **Invariant:** The Agent Orchestrator (and any AI agent) must NEVER call
//! FS/Shell/Web tools directly. All privileged operations MUST go through:
//!
//!   Command Router → Security Policy Engine → optional HITL Approval → tool
//!
//! This module is the ONLY surface for FS/Shell operations. The `invoke_handler`
//! below registers the sole commands (`cmd_fs_*`, `cmd_shell_run`). There are
//! no direct `fs::` or `shell::` Tauri commands — those modules are called
//! internally only AFTER policy evaluation. Agents invoke these commands with
//! `caller: "agent_orchestrator"`; the policy engine enforces stricter rules
//! (e.g. write/exec → REQUIRE_APPROVAL) for agents.
//!
//! Oxcer's primary UI is a native Swift app. The Tauri backend currently uses a
//! hidden window but can be evolved into a tray app or pure daemon if needed.

use std::path::PathBuf;
use std::sync::Mutex;

use http::{header::CONTENT_TYPE, Response, StatusCode};
use tauri::menu::{MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder};
use tauri_plugin_dialog::DialogExt;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_fs;
use uuid::Uuid;

use oxcer_core::fs;
use oxcer_core::security::policy_engine::{
    evaluate, Operation, PolicyCaller, PolicyDecisionKind, PolicyRequest, PolicyTarget, ToolType,
};
use oxcer_core::shell;

use oxcer::event_log;
use oxcer::router;
use oxcer::router::{
    get_destructive_command_visibility, CommandVisibilityContext, PendingApprovalsStore,
    PendingOperation, RouterError, to_requested_payload,
};
use oxcer::settings::{
    get_effective_fs_policy as settings_get_effective_fs_policy, is_forbidden_workspace_path,
    load as settings_load, log_destructive_setting_change as settings_log_destructive_change,
    save as settings_save, AppSettings, to_workspace_roots, WorkspaceDirectory, EffectiveFsPolicy,
};

/// Helper to get effective FS policy from app state (for config gates).
fn effective_fs_policy_from_app(app: &AppHandle) -> EffectiveFsPolicy {
    app.try_state::<Mutex<AppSettings>>()
        .map(|state| settings_get_effective_fs_policy(&state.lock().expect("settings lock")))
        .unwrap_or_else(|| EffectiveFsPolicy {
            allowed_workspaces: vec![],
            destructive_operations_enabled: false,
        })
}

/// Helper to build the FS context from the running application handle and current settings.
fn build_fs_context(app: &AppHandle) -> fs::AppFsContext {
    let app_config_dir = app
        .path()
        .app_config_dir()
        .expect("app_config_dir should be available");

    let workspace_roots = app
        .try_state::<Mutex<AppSettings>>()
        .map(|state| {
            let guard = state.lock().expect("settings lock");
            to_workspace_roots(&guard.workspace_directories)
        })
        .unwrap_or_default();

    fs::AppFsContext {
        app_config_dir,
        workspace_roots,
    }
}

const WORKSPACE_OUTSIDE_MSG: &str =
    "This path is outside your configured workspaces. Please add a workspace in Settings.";

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>Oxcer Guardrails Dashboard</title>
<style>body{font-family:system-ui;color:#e0e0e0;background:#1a1a1a;margin:0;padding:20px;}
#toast{position:fixed;bottom:20px;right:20px;background:#333;color:#e0e0e0;padding:12px 20px;border-radius:8px;max-width:360px;display:none;}</style>
</head>
<body>
<h1>Oxcer Guardrails Dashboard</h1>
<p style="color:#a0a0a0;">Add workspaces in Settings. Destructive operations require explicit enabling.</p>
<div id="toast"></div>
<script>
(function(){const i=window.__TAURI__?.core?.invoke;if(!i)return;
function t(m){const e=document.getElementById('toast');e.textContent=m;e.style.display='block';setTimeout(()=>e.style.display='none',6000);}
window.__TAURI__?.event?.listen?.('security.destructive_op_executed',e=>t(e.payload?.summary||'')).catch(()=>{});})();
</script>
</body>
</html>"#;

/// Resolve workspace_root to (ctx, workspace_id). No implicit defaults.
/// - Fails if no workspaces are configured.
/// - Fails if workspace_root does not match any registered workspace.
fn ctx_and_workspace_id(
    app: &AppHandle,
    workspace_root: &str,
) -> Result<(fs::AppFsContext, String), RouterError> {
    let ctx = build_fs_context(app);
    if ctx.workspace_roots.is_empty() {
        return Err(RouterError::PolicyDenied {
            reason_code: "NO_WORKSPACES".to_string(),
            message: WORKSPACE_OUTSIDE_MSG.to_string(),
        });
    }
    let path_buf = PathBuf::from(workspace_root);
    let canonical = path_buf.canonicalize().ok();
    let id_opt = ctx.workspace_roots.iter().find_map(|w| {
        if w.path == path_buf {
            Some(w.id.clone())
        } else if let Some(ref can) = canonical {
            w.path.canonicalize().ok().filter(|cw| cw == can).map(|_| w.id.clone())
        } else {
            None
        }
    });
    if let Some(id) = id_opt {
        return Ok((ctx, id));
    }
    Err(RouterError::PolicyDenied {
        reason_code: "WORKSPACE_OUTSIDE_SCOPE".to_string(),
        message: WORKSPACE_OUTSIDE_MSG.to_string(),
    })
}

fn fs_caller_from_policy(c: PolicyCaller) -> fs::FsCaller {
    match c {
        PolicyCaller::Ui => fs::FsCaller::Ui,
        PolicyCaller::AgentOrchestrator => fs::FsCaller::Agent,
        PolicyCaller::InternalSystem => fs::FsCaller::ShellTool,
    }
}

fn shell_caller_from_policy(c: PolicyCaller) -> shell::ShellCaller {
    match c {
        PolicyCaller::Ui => shell::ShellCaller::Ui,
        PolicyCaller::AgentOrchestrator => shell::ShellCaller::Agent,
        PolicyCaller::InternalSystem => shell::ShellCaller::System,
    }
}

/// FS list_dir — Agent MUST use this; never call fs:: directly.
#[tauri::command]
fn cmd_fs_list_dir(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    caller: Option<String>,
) -> Result<Vec<fs::DirEntryMetadata>, RouterError> {
    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    let base = fs::BaseDirKind::Workspace { id: workspace_id.clone() };
    let normalized = fs::normalize_and_resolve(&ctx, &base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
    };
    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Access denied by policy".to_string(),
        });
    }

    fs::fs_list_dir(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace { id: workspace_id },
        &rel_path,
    )
    .map_err(RouterError::from)
}

/// FS read_file — Agent MUST use this; never call fs:: directly.
#[tauri::command]
fn cmd_fs_read_file(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    caller: Option<String>,
) -> Result<fs::FsReadResult, RouterError> {
    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    let base = fs::BaseDirKind::Workspace { id: workspace_id.clone() };
    let normalized = fs::normalize_and_resolve(&ctx, &base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
    };
    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Access denied by policy".to_string(),
        });
    }

    fs::fs_read_file(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace { id: workspace_id },
        &rel_path,
    )
    .map_err(RouterError::from)
}

/// FS write_file — Agent MUST use this; never call fs:: directly.
#[tauri::command]
fn cmd_fs_write_file(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    contents: Vec<u8>,
    caller: Option<String>,
) -> Result<(), RouterError> {
    let policy_caller = router::parse_caller(caller.as_deref());

    let canonical_path = std::path::Path::new(&workspace_root)
        .join(&rel_path)
        .display()
        .to_string();

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Write,
        target: PolicyTarget::FsPath {
            canonical_path: canonical_path.clone(),
        },
    };
    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Write denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Write to {}", rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Write,
            PolicyTarget::FsPath {
                canonical_path: canonical_path.clone(),
            },
            PendingOperation::FsWrite {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                contents: contents.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_write".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    fs::fs_write_file(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace { id: workspace_id },
        &rel_path,
        &contents,
    )
    .map_err(RouterError::from)
}

const DESTRUCTIVE_DISABLED_MSG: &str =
    "Destructive file operations are disabled in Settings.";

/// Emit event when a high-risk FS op completes (for toast feedback).
fn emit_destructive_op_executed(app: &AppHandle, summary: &str) {
    let _ = app.emit(
        "security.destructive_op_executed",
        serde_json::json!({
            "summary": summary,
            "unlocked": true,
        }),
    );
}

/// FS delete — Agent MUST use this. Gated by config.
/// Agent never executes delete immediately; all agent delete requests require explicit user approval.
#[tauri::command]
fn cmd_fs_delete(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    caller: Option<String>,
) -> Result<(), RouterError> {
    let policy = effective_fs_policy_from_app(&app);
    if !policy.destructive_operations_enabled {
        return Err(RouterError::ConfigDisabled {
            message: DESTRUCTIVE_DISABLED_MSG.to_string(),
        });
    }

    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    let base = fs::BaseDirKind::Workspace { id: workspace_id.clone() };
    let normalized = fs::normalize_and_resolve(&ctx, &base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Delete,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
    };

    // Agent must NEVER execute delete without explicit user approval.
    if policy_caller == PolicyCaller::AgentOrchestrator {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Delete {}", rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Delete,
            request.target,
            PendingOperation::FsDelete {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
            },
            "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        if let Ok(app_config_dir) = app.path().app_config_dir() {
            let _ = event_log::append(
                &app_config_dir,
                "destructive_approval.requested",
                Some(&workspace_id),
                Some(&serde_json::json!({
                    "operation": "fs_delete",
                    "request_id": request_id,
                    "rel_path": rel_path,
                    "summary": summary
                })),
            );
        }
        app.emit("security.approval.requested", &payload).ok();
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_delete".to_string(),
            summary,
            reason_code: "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
        });
    }

    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Delete denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Delete {}", rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Delete,
            request.target,
            PendingOperation::FsDelete {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_delete".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    fs::fs_remove_file(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace { id: workspace_id.clone() },
        &rel_path,
    )
    .map_err(RouterError::from)?;
    emit_destructive_op_executed(
        &app,
        &format!("Deleted {} in \"{}/\". (Destructive operations enabled in Settings.)", rel_path, workspace_id),
    );
    Ok(())
}

/// FS rename — Agent MUST use this. Gated by config.
/// Agent never executes rename immediately; all agent rename requests require explicit user approval.
#[tauri::command]
fn cmd_fs_rename(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    new_rel_path: String,
    caller: Option<String>,
) -> Result<(), RouterError> {
    let policy = effective_fs_policy_from_app(&app);
    if !policy.destructive_operations_enabled {
        return Err(RouterError::ConfigDisabled {
            message: DESTRUCTIVE_DISABLED_MSG.to_string(),
        });
    }

    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    let base = fs::BaseDirKind::Workspace { id: workspace_id.clone() };
    let normalized = fs::normalize_and_resolve(&ctx, &base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Rename,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
    };

    if policy_caller == PolicyCaller::AgentOrchestrator {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Rename {} → {}", rel_path, new_rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Rename,
            request.target,
            PendingOperation::FsRename {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                new_rel_path: new_rel_path.clone(),
            },
            "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        if let Ok(app_config_dir) = app.path().app_config_dir() {
            let _ = event_log::append(
                &app_config_dir,
                "destructive_approval.requested",
                Some(&workspace_id),
                Some(&serde_json::json!({
                    "operation": "fs_rename",
                    "request_id": request_id,
                    "rel_path": rel_path,
                    "new_rel_path": new_rel_path,
                    "summary": summary
                })),
            );
        }
        app.emit("security.approval.requested", &payload).ok();
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_rename".to_string(),
            summary,
            reason_code: "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
        });
    }

    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Rename denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Rename {} → {}", rel_path, new_rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Rename,
            request.target,
            PendingOperation::FsRename {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                new_rel_path: new_rel_path.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_rename".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    fs::fs_rename(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace { id: workspace_id.clone() },
        &rel_path,
        &new_rel_path,
    )
    .map_err(RouterError::from)?;
    emit_destructive_op_executed(
        &app,
        &format!("Renamed {} → {} in \"{}/\". (Destructive operations enabled in Settings.)", rel_path, new_rel_path, workspace_id),
    );
    Ok(())
}

/// FS move — Agent MUST use this. Gated by config.
/// Agent never executes move immediately; all agent move requests require explicit user approval.
#[tauri::command]
fn cmd_fs_move(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    dest_workspace_root: String,
    dest_rel_path: String,
    caller: Option<String>,
) -> Result<(), RouterError> {
    let policy = effective_fs_policy_from_app(&app);
    if !policy.destructive_operations_enabled {
        return Err(RouterError::ConfigDisabled {
            message: DESTRUCTIVE_DISABLED_MSG.to_string(),
        });
    }

    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, src_workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
    let (_, dest_workspace_id) = ctx_and_workspace_id(&app, &dest_workspace_root)?;

    let src_base = fs::BaseDirKind::Workspace { id: src_workspace_id.clone() };
    let normalized = fs::normalize_and_resolve(&ctx, &src_base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Move,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
    };

    if policy_caller == PolicyCaller::AgentOrchestrator {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Move {} → {}/{}", rel_path, dest_workspace_root, dest_rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Move,
            request.target,
            PendingOperation::FsMove {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                dest_workspace_root: dest_workspace_root.clone(),
                dest_rel_path: dest_rel_path.clone(),
            },
            "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        if let Ok(app_config_dir) = app.path().app_config_dir() {
            let _ = event_log::append(
                &app_config_dir,
                "destructive_approval.requested",
                Some(&src_workspace_id),
                Some(&serde_json::json!({
                    "operation": "fs_move",
                    "request_id": request_id,
                    "rel_path": rel_path,
                    "dest_workspace_root": dest_workspace_root,
                    "dest_rel_path": dest_rel_path,
                    "summary": summary
                })),
            );
        }
        app.emit("security.approval.requested", &payload).ok();
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_move".to_string(),
            summary,
            reason_code: "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
        });
    }

    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Move denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Move {} → {}/{}", rel_path, dest_workspace_root, dest_rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Move,
            request.target,
            PendingOperation::FsMove {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                dest_workspace_root: dest_workspace_root.clone(),
                dest_rel_path: dest_rel_path.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_move".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    fs::fs_move(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace { id: src_workspace_id.clone() },
        &rel_path,
        fs::BaseDirKind::Workspace { id: dest_workspace_id.clone() },
        &dest_rel_path,
    )
    .map_err(RouterError::from)?;
    emit_destructive_op_executed(
        &app,
        &format!("Moved {} → \"{}/{}\". (Destructive operations enabled in Settings.)", rel_path, dest_workspace_id, dest_rel_path),
    );
    Ok(())
}

/// Shell run — Agent MUST use this; never call shell:: directly.
#[tauri::command]
fn cmd_shell_run(
    app: AppHandle,
    workspace_root: String,
    command_id: String,
    params: serde_json::Value,
    caller: Option<String>,
) -> Result<shell::ShellResult, RouterError> {
    let policy_caller = router::parse_caller(caller.as_deref());

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Shell,
        operation: Operation::Exec,
        target: PolicyTarget::ShellCommand {
            command_id: command_id.clone(),
            normalized_command: None,
        },
    };
    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Command denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Execute command: {}", command_id);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Shell,
            Operation::Exec,
            PolicyTarget::ShellCommand {
                command_id: command_id.clone(),
                normalized_command: None,
            },
            PendingOperation::ShellRun {
                workspace_root: workspace_root.clone(),
                command_id: command_id.clone(),
                params: params.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "shell_run".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    let (fs_ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
    let ctx = shell::ShellContext {
        workspace_roots: fs_ctx.workspace_roots,
        default_workspace_id: workspace_id,
    };
    let catalog = shell::default_catalog();
    shell::shell_run(
        shell_caller_from_policy(policy_caller),
        &ctx,
        &catalog,
        &command_id,
        params,
    )
    .map_err(RouterError::from)
}

/// Execute a pending approval request after user confirms in the HITL modal.
/// On Allow: marks record APPROVED, resumes original command execution.
/// On Deny: marks DENIED, returns error.
/// Destructive (delete/rename/move) requests and decisions are logged to the event log.
#[tauri::command]
fn cmd_approve_and_execute(
    app: AppHandle,
    request_id: String,
    approved: bool,
) -> Result<serde_json::Value, RouterError> {
    let store = app.state::<PendingApprovalsStore>();
    let record = store.take(&request_id).ok_or_else(|| {
        RouterError::PolicyDenied {
            reason_code: "EXPIRED_OR_UNKNOWN".to_string(),
            message: "Approval request expired or not found".to_string(),
        }
    })?;

    let is_destructive = matches!(
        record.operation_payload,
        PendingOperation::FsDelete { .. }
            | PendingOperation::FsRename { .. }
            | PendingOperation::FsMove { .. }
    );
    if is_destructive {
        let op_name = match &record.operation_payload {
            PendingOperation::FsDelete { .. } => "fs_delete",
            PendingOperation::FsRename { .. } => "fs_rename",
            PendingOperation::FsMove { .. } => "fs_move",
            _ => "",
        };
        if let Ok(dir) = app.path().app_config_dir() {
            let event_type = if approved {
                "destructive_approval.approved"
            } else {
                "destructive_approval.denied"
            };
            let _ = event_log::append(
                &dir,
                event_type,
                None,
                Some(&serde_json::json!({
                    "request_id": request_id,
                    "operation": op_name,
                    "summary": record.summary
                })),
            );
        }
    }

    if !approved {
        return Err(RouterError::PolicyDenied {
            reason_code: "USER_DENIED".to_string(),
            message: "User denied the operation".to_string(),
        });
    }

    match record.operation_payload {
        PendingOperation::FsWrite {
            workspace_root,
            rel_path,
            contents,
        } => {
            let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            fs::fs_write_file(
                fs::FsCaller::Agent,
                &ctx,
                fs::BaseDirKind::Workspace { id: workspace_id },
                &rel_path,
                &contents,
            )
            .map_err(RouterError::from)?;
            Ok(serde_json::json!({ "success": true }))
        }
        PendingOperation::FsDelete {
            workspace_root,
            rel_path,
        } => {
            let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            fs::fs_remove_file(
                fs::FsCaller::Agent,
                &ctx,
                fs::BaseDirKind::Workspace { id: workspace_id.clone() },
                &rel_path,
            )
            .map_err(RouterError::from)?;
            emit_destructive_op_executed(
                &app,
                &format!("Deleted {} in \"{}/\". (Destructive operations enabled in Settings.)", rel_path, workspace_id),
            );
            Ok(serde_json::json!({ "success": true }))
        }
        PendingOperation::FsRename {
            workspace_root,
            rel_path,
            new_rel_path,
        } => {
            let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            fs::fs_rename(
                fs::FsCaller::Agent,
                &ctx,
                fs::BaseDirKind::Workspace { id: workspace_id.clone() },
                &rel_path,
                &new_rel_path,
            )
            .map_err(RouterError::from)?;
            emit_destructive_op_executed(
                &app,
                &format!("Renamed {} → {} in \"{}/\". (Destructive operations enabled in Settings.)", rel_path, new_rel_path, workspace_id),
            );
            Ok(serde_json::json!({ "success": true }))
        }
        PendingOperation::FsMove {
            workspace_root,
            rel_path,
            dest_workspace_root,
            dest_rel_path,
        } => {
            let (ctx, src_workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            let (_, dest_workspace_id) = ctx_and_workspace_id(&app, &dest_workspace_root)?;
            fs::fs_move(
                fs::FsCaller::Agent,
                &ctx,
                fs::BaseDirKind::Workspace { id: src_workspace_id },
                &rel_path,
                fs::BaseDirKind::Workspace { id: dest_workspace_id.clone() },
                &dest_rel_path,
            )
            .map_err(RouterError::from)?;
            emit_destructive_op_executed(
                &app,
                &format!("Moved {} → \"{}/{}\". (Destructive operations enabled in Settings.)", rel_path, dest_workspace_id, dest_rel_path),
            );
            Ok(serde_json::json!({ "success": true }))
        }
        PendingOperation::ShellRun {
            workspace_root,
            command_id,
            params,
        } => {
            let (fs_ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            let ctx = shell::ShellContext {
                workspace_roots: fs_ctx.workspace_roots,
                default_workspace_id: workspace_id,
            };
            let catalog = shell::default_catalog();
            let result = shell::shell_run(
                shell::ShellCaller::Agent,
                &ctx,
                &catalog,
                &command_id,
                params,
            )
            .map_err(RouterError::from)?;
            Ok(serde_json::json!({
                "success": true,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "exit_code": result.exit_code,
                "duration_ms": result.duration_ms
            }))
        }
    }
}

// -----------------------------------------------------------------------------
// Settings commands (for Settings screen)
// -----------------------------------------------------------------------------

#[tauri::command]
fn cmd_settings_get(app: AppHandle) -> Result<AppSettings, String> {
    let state = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let settings = state.lock().expect("settings lock").clone();
    Ok(settings)
}

#[tauri::command]
fn cmd_settings_save(app: AppHandle, settings: AppSettings) -> Result<(), String> {
    let app_config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let state = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let prev = state.lock().expect("settings lock").clone();
    let from = prev.advanced.allow_destructive_fs_without_hitl;
    let to = settings.advanced.allow_destructive_fs_without_hitl;
    settings_save(&app_config_dir, &settings)?;
    if from != to {
        let _ = settings_log_destructive_change(&app_config_dir, from, to);
        let event_type = if to {
            "security.destructive_fs.enabled"
        } else {
            "security.destructive_fs.disabled"
        };
        let _ = event_log::append(
            &app_config_dir,
            event_type,
            None,
            Some(&serde_json::json!({ "from": from, "to": to })),
        );
    }
    *state.lock().expect("settings lock") = settings;
    Ok(())
}

/// Opens native directory picker; returns selected path or null if cancelled.
#[tauri::command]
fn cmd_dialog_open_directory(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::FilePath;

    let path: Option<FilePath> = app.dialog().file().blocking_pick_folder();

    let path = match path {
        Some(file_path) => match file_path.into_path() {
            Ok(pb) => Some(pb.display().to_string()),
            Err(_) => None,
        },
        None => None,
    };

    Ok(path)
}

#[tauri::command]
fn cmd_workspace_add(app: AppHandle, path: String) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    if !path_buf.is_dir() {
        return Err("Path is not a directory".to_string());
    }
    if is_forbidden_workspace_path(&path_buf) {
        return Err("This directory cannot be used as a workspace (home or parent of home)".to_string());
    }
    let app_config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let state = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let mut guard = state.lock().expect("settings lock");
    let canonical = path_buf.canonicalize().map_err(|e| e.to_string())?;
    let path_str = canonical.display().to_string();
    if guard.workspace_directories.iter().any(|w| {
        PathBuf::from(&w.path).canonicalize().as_ref().map(|p| p == &canonical).unwrap_or(false)
    }) {
        return Err("This workspace is already added".to_string());
    }
    let name = path_buf.file_name().and_then(|n| n.to_str()).unwrap_or("Workspace").to_string();
    let id = uuid::Uuid::new_v4().to_string();
    guard.workspace_directories.push(WorkspaceDirectory {
        id: id.clone(),
        name: name.clone(),
        path: path_str.clone(),
    });
    settings_save(&app_config_dir, &*guard)?;
    let _ = event_log::append(
        &app_config_dir,
        "workspace_added",
        Some(&id),
        Some(&serde_json::json!({ "name": name, "root_path": path_str })),
    );
    Ok(())
}

#[tauri::command]
fn cmd_workspace_remove(app: AppHandle, id: String) -> Result<(), String> {
    oxcer::commands::workspace_cleanup_on_delete(&app, &id)
}

/// Returns effective FS policy for the Security Policy Engine.
/// - allowed_workspaces: list of root paths
/// - destructive_operations_enabled: from config
#[tauri::command]
fn get_effective_fs_policy(app: AppHandle) -> Result<EffectiveFsPolicy, String> {
    let state = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let settings = state.lock().expect("settings lock").clone();
    Ok(settings_get_effective_fs_policy(&settings))
}

/// Returns config.json as JSON (workspaces, default_model, fs options). SSOT for dashboard.
#[tauri::command]
fn get_config(app: AppHandle) -> Result<serde_json::Value, String> {
    let app_config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let config_path = app_config_dir.join("config.json");
    let config = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(_) => {
            return Ok(serde_json::json!({
                "security": { "destructive_fs": { "enabled": false } },
                "workspaces": [],
                "model": { "default_id": "gemini-2.5-flash" }
            }))
        }
    };
    serde_json::from_str(&config).map_err(|e| e.to_string())
}

/// Returns visibility for destructive commands (delete/rename/move) so UI can hide or show disabled with explanation.
/// context: "main" = command palette (hide when off), "advanced" = Settings advanced (show disabled with message).
#[tauri::command]
fn get_command_visibility(
    app: AppHandle,
    context: CommandVisibilityContext,
) -> Result<std::collections::HashMap<String, oxcer::router::CommandVisibility>, String> {
    let destructive = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?
        .lock()
        .expect("settings lock")
        .advanced
        .allow_destructive_fs_without_hitl;
    Ok(get_destructive_command_visibility(destructive, context))
}

/// Returns available model options for the default-model dropdown (id + display name). Sprint 5 spec.
/// Stored selection is persisted to model.default_id only; Semantic Router / real model routing in a later sprint.
#[tauri::command]
fn cmd_models_list() -> Vec<(String, String)> {
    vec![
        (String::new(), "— Select model —".to_string()),
        ("gemini-2.5-flash".to_string(), "Gemini 2.5 Flash (Default)".to_string()),
        ("gemini-2.5-pro".to_string(), "Gemini 2.5 Pro".to_string()),
        ("gemini-1.5-flash".to_string(), "Gemini 1.5 Flash".to_string()),
        ("gpt-4.1-mini".to_string(), "GPT-4.1 Mini".to_string()),
        ("gpt-4o-mini".to_string(), "GPT-4o Mini".to_string()),
        ("claude-3.5-sonnet-latest".to_string(), "Claude 3.5 Sonnet (Latest)".to_string()),
        ("grok-4.1-fast".to_string(), "Grok 4.1 Fast".to_string()),
        ("grok-3-mini".to_string(), "Grok 3 Mini".to_string()),
    ]
}

/// Returns the Tauri context for the app.
fn app_context() -> tauri::Context<tauri::Wry> {
    #[cfg(all(test, feature = "test"))]
    {
        tauri::test::mock_context(tauri::test::noop_assets())
    }

    #[cfg(not(all(test, feature = "test")))]
    {
        tauri::generate_context!()
    }
}

fn main() {
    let context = app_context();
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(PendingApprovalsStore::new())
        .manage(Mutex::new(AppSettings::default()))
        .setup(|app| {
            let handle = app.handle();
            let app_config_dir = handle.path().app_config_dir().expect("app_config_dir");
            std::fs::create_dir_all(&app_config_dir).expect("failed to create app config directory");
            let loaded = settings_load(&app_config_dir);
            if let Some(state) = handle.try_state::<Mutex<AppSettings>>() {
                *state.lock().expect("settings lock") = loaded;
            }

            // App menu: Oxcer (Quit); in debug builds also View → Toggle Developer Tools
            let quit = PredefinedMenuItem::quit(handle, None)?;
            let app_sub = SubmenuBuilder::new(handle, "Oxcer")
                .item(&quit)
                .build()?;

            #[cfg(debug_assertions)]
            {
                let devtools_item = MenuItem::with_id(
                    handle,
                    "devtools",
                    "Toggle Developer Tools",
                    true,
                    Some("CmdOrCtrl+Shift+I"),
                )?;
                let view_sub =
                    SubmenuBuilder::new(handle, "View").item(&devtools_item).build()?;
                let menu = MenuBuilder::new(handle)
                    .items(&[&app_sub, &view_sub])
                    .build()?;
                app.set_menu(menu)?;
                app.on_menu_event(move |app_handle, event| {
                    if event.id().0.as_str() == "devtools" {
                        if let Some(w) = app_handle.get_webview_window("main") {
                            w.open_devtools();
                        }
                    }
                });
                // Open devtools on startup in debug builds
                if let Some(w) = app.get_webview_window("main") {
                    w.open_devtools();
                }
            }

            #[cfg(not(debug_assertions))]
            {
                let menu = MenuBuilder::new(handle).items(&[&app_sub]).build()?;
                app.set_menu(menu)?;
            }

            Ok(())
        })
        // ONLY invoke commands for FS/Shell — no bypass. Agent Orchestrator
        // must use these; direct fs::/shell:: calls are not exposed.
        .register_uri_scheme_protocol("oxcer", |_ctx, request| {
            let path = request.uri().path();
            let (status, body, content_type) = match path {
                "/main" | "main" | "/" | "" => {
                    (StatusCode::OK, DASHBOARD_HTML.as_bytes().to_vec(), "text/html; charset=utf-8")
                }
                _ => (
                    StatusCode::NOT_FOUND,
                    b"Not Found".to_vec(),
                    "text/plain",
                ),
            };
            Response::builder()
                .status(status)
                .header(CONTENT_TYPE, content_type)
                .body(body)
                .unwrap()
        })
        .invoke_handler(tauri::generate_handler![
            cmd_fs_list_dir,
            cmd_fs_read_file,
            cmd_fs_write_file,
            cmd_fs_delete,
            cmd_fs_rename,
            cmd_fs_move,
            cmd_shell_run,
            cmd_approve_and_execute,
            cmd_settings_get,
            cmd_settings_save,
            cmd_dialog_open_directory,
            cmd_workspace_add,
            cmd_workspace_remove,
            cmd_models_list,
            get_config,
            get_command_visibility,
            get_effective_fs_policy,
        ])
        .run(context)
        .expect("error while running Oxcer Tauri application");
}
