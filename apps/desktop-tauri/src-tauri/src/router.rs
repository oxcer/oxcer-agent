//! Command Router: sits between Tauri invoke and FS/Shell services.
//!
//! The Security Policy Engine is the final authority for all privileged
//! actions. All requests (UI and Agent) pass through this router.
//!
//! ## "Agent = untrusted client" invariant
//!
//! The Agent Orchestrator **never** calls `fs::` or `shell::` directly.
//! It invokes Tauri commands (`cmd_fs_*`, `cmd_shell_run`) with
//! `caller: "agent_orchestrator"`. Those commands build a `PolicyRequest`,
//! call `evaluate()`, and only then dispatch to the underlying tool.
//! Destructive operations (write, delete, exec) from agents return
//! `REQUIRE_APPROVAL` and are gated by the HITL flow.
//!
//! Pseudocode flow:
//! 1. Build PolicyRequest from incoming command (caller, tool_type, operation, target)
//! 2. Call evaluate(request)
//! 3. DENY -> short-circuit, return error, log the decision
//! 4. ALLOW -> execute underlying tool/command
//! 5. REQUIRE_APPROVAL -> trigger HITL approval flow before proceeding

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use oxcer_core::fs;
use oxcer_core::security::policy_engine::{
    Operation, PolicyCaller, PolicyDecision, PolicyDecisionKind, PolicyRequest, PolicyTarget,
    ToolType,
};
use oxcer_core::shell;
use serde::{Deserialize, Serialize};

// -----------------------------------------------------------------------------
// Policy decision logging (aligned with FS/Shell log format)
// -----------------------------------------------------------------------------

#[derive(Serialize)]
struct PolicyLogEntry<'a> {
    timestamp: String,
    caller: &'a str,
    tool_type: &'a str,
    operation: &'a str,
    decision: &'a str,
    reason_code: &'a str,
}

/// Logs policy decision for audit trail. Called on every DENY, and optionally
/// on REQUIRE_APPROVAL and ALLOW for full traceability.
pub fn log_policy_decision(request: &PolicyRequest, decision: &PolicyDecision) {
    let timestamp = {
        let now = std::time::SystemTime::now();
        match now.duration_since(std::time::UNIX_EPOCH) {
            Ok(dur) => format!("{}.{:03}Z", dur.as_secs(), dur.subsec_millis()),
            Err(_) => "0.000Z".to_string(),
        }
    };
    let entry = PolicyLogEntry {
        timestamp,
        caller: match request.caller {
            PolicyCaller::Ui => "ui",
            PolicyCaller::AgentOrchestrator => "agent_orchestrator",
            PolicyCaller::InternalSystem => "internal_system",
        },
        tool_type: match request.tool_type {
            ToolType::Fs => "fs",
            ToolType::Shell => "shell",
            ToolType::Agent => "agent",
            ToolType::Web => "web",
            ToolType::Other => "other",
        },
        operation: match request.operation {
            Operation::Read => "read",
            Operation::Write => "write",
            Operation::Delete => "delete",
            Operation::Rename => "rename",
            Operation::Move => "move",
            Operation::Chmod => "chmod",
            Operation::Exec => "exec",
        },
        decision: match decision.decision {
            PolicyDecisionKind::Allow => "allow",
            PolicyDecisionKind::Deny => "deny",
            PolicyDecisionKind::RequireApproval => "require_approval",
        },
        reason_code: decision.reason_code.as_str(),
    };
    if let Ok(json) = serde_json::to_string(&entry) {
        println!("[policy] {json}");
    }
}

// -----------------------------------------------------------------------------
// Command visibility (Sprint 5: hide vs disabled-with-explanation)
// -----------------------------------------------------------------------------

/// Policy for whether a dangerous command is shown and how.
/// Other frontends (SwiftUI, etc.) can reuse this.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandVisibility {
    /// Do not show in main command palette / menus.
    Hidden,
    /// Show as disabled; on hover/click show explanation (e.g. helper text or modal).
    DisabledWithExplanation { message: String },
    /// Shown and available.
    Available,
}

/// Context for visibility: "main" (command palette / primary UI) vs "advanced" (Settings advanced panel).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandVisibilityContext {
    Main,
    Advanced,
}

/// Destructive FS commands (delete, rename, move). Used by get_destructive_command_visibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestructiveCommand {
    FsDelete,
    FsRename,
    FsMove,
}

const DESTRUCTIVE_DISABLED_MSG: &str =
    "Enable 'Allow destructive file operations' in Settings to use this option.";

/// Returns visibility for destructive commands so UI can hide or show disabled with explanation.
pub fn get_destructive_command_visibility(
    destructive_fs_enabled: bool,
    context: CommandVisibilityContext,
) -> HashMap<String, CommandVisibility> {
    let mut out = HashMap::new();
    let (del, ren, mov) = if destructive_fs_enabled {
        (
            CommandVisibility::Available,
            CommandVisibility::Available,
            CommandVisibility::Available,
        )
    } else {
        match context {
            CommandVisibilityContext::Main => (
                CommandVisibility::Hidden,
                CommandVisibility::Hidden,
                CommandVisibility::Hidden,
            ),
            CommandVisibilityContext::Advanced => (
                CommandVisibility::DisabledWithExplanation {
                    message: DESTRUCTIVE_DISABLED_MSG.to_string(),
                },
                CommandVisibility::DisabledWithExplanation {
                    message: DESTRUCTIVE_DISABLED_MSG.to_string(),
                },
                CommandVisibility::DisabledWithExplanation {
                    message: DESTRUCTIVE_DISABLED_MSG.to_string(),
                },
            ),
        }
    };
    out.insert("fs_delete".to_string(), del);
    out.insert("fs_rename".to_string(), ren);
    out.insert("fs_move".to_string(), mov);
    out
}

// -----------------------------------------------------------------------------
// Caller mapping
// -----------------------------------------------------------------------------

/// Maps string caller from frontend to PolicyCaller.
pub fn parse_caller(s: Option<&str>) -> PolicyCaller {
    match s {
        Some("agent_orchestrator") | Some("AGENT_ORCHESTRATOR") => PolicyCaller::AgentOrchestrator,
        Some("internal_system") | Some("INTERNAL_SYSTEM") => PolicyCaller::InternalSystem,
        _ => PolicyCaller::Ui,
    }
}

// -----------------------------------------------------------------------------
// Approval request record (HITL)
// -----------------------------------------------------------------------------

/// Status of an approval request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

/// Original payload enough to reconstruct the call. Secrets redacted for display.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PendingOperation {
    FsWrite {
        workspace_root: String,
        rel_path: String,
        contents: Vec<u8>,
    },
    FsDelete {
        workspace_root: String,
        rel_path: String,
    },
    FsRename {
        workspace_root: String,
        rel_path: String,
        new_rel_path: String,
    },
    FsMove {
        workspace_root: String,
        rel_path: String,
        dest_workspace_root: String,
        dest_rel_path: String,
    },
    ShellRun {
        workspace_root: String,
        command_id: String,
        params: serde_json::Value,
    },
}

/// Full approval request record with status, timestamps, and actor fields.
#[derive(Clone, Debug)]
pub struct ApprovalRequestRecord {
    pub request_id: String,
    pub caller: PolicyCaller,
    pub tool_type: ToolType,
    pub operation: Operation,
    pub target: PolicyTarget,
    pub operation_payload: PendingOperation,
    pub status: ApprovalStatus,
    pub created_at: Instant,
    pub expires_at: Instant,
    pub approved_by: Option<String>,
    pub approved_at: Option<Instant>,
    pub denied_by: Option<String>,
    pub denied_at: Option<Instant>,
    pub reason_code: String,
    pub summary: String,
}

impl ApprovalRequestRecord {
    fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

/// One affected item for the approval modal impact summary.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AffectedItem {
    pub name: String,
    pub path: String,
}

/// Impact summary for destructive operations: list of affected files/folders and counts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImpactSummary {
    pub item_count: u32,
    pub affected_items: Vec<AffectedItem>,
    /// Human-readable description: "Delete 1 file", "Move 1 file to …", etc.
    pub operation_description: String,
    /// For move: destination path. Absent for delete/rename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_path: Option<String>,
}

/// Sanitized payload for event emission (no secrets in plain text).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalRequestedPayload {
    pub request_id: String,
    pub caller: String,
    pub tool_type: String,
    pub operation: String,
    pub target: String,
    pub target_hint: String,
    pub reason_code: String,
    pub summary: String,
    pub risk_hints: Vec<String>,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    /// Redacted payload for View details (no raw contents, only metadata).
    pub details_redacted: serde_json::Value,
    /// For delete/move/rename: impact summary for the confirmation modal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact_summary: Option<ImpactSummary>,
    /// True for irreversible actions (e.g. hard delete); UI shows danger-zone copy.
    #[serde(default)]
    pub danger_zone: bool,
}

/// In-memory store for approval requests. Configurable timeout (default 5 min).
/// Timeout fails closed (auto-deny on expiry).
///
/// ## Concurrency guarantees
///
/// - **Single execution**: `take(request_id)` atomically removes the record. At most
///   one caller can ever retrieve a given approval; subsequent callers get `None`.
/// - **Idempotent take**: Once taken, further `take(id)` calls return `None`—no double
///   execution of the same approval.
/// - All mutations (`insert`, `take`, `cleanup_expired`) are guarded by an internal
///   `Mutex`, so concurrent access is serialized and race-free.
pub struct PendingApprovalsStore {
    pub inner: Mutex<std::collections::HashMap<String, ApprovalRequestRecord>>,
    ttl: Duration,
}

impl PendingApprovalsStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(std::collections::HashMap::new()),
            ttl: Duration::from_secs(300),
        }
    }

    pub fn with_timeout_secs(secs: u64) -> Self {
        Self {
            inner: Mutex::new(std::collections::HashMap::new()),
            ttl: Duration::from_secs(secs),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_record(
        &self,
        request_id: String,
        caller: PolicyCaller,
        tool_type: ToolType,
        operation: Operation,
        target: PolicyTarget,
        operation_payload: PendingOperation,
        reason_code: String,
        summary: String,
    ) -> ApprovalRequestRecord {
        let now = Instant::now();
        let ttl = self.ttl;
        ApprovalRequestRecord {
            request_id: request_id.clone(),
            caller,
            tool_type,
            operation,
            target,
            operation_payload,
            status: ApprovalStatus::Pending,
            created_at: now,
            expires_at: now + ttl,
            approved_by: None,
            approved_at: None,
            denied_by: None,
            denied_at: None,
            reason_code: reason_code.clone(),
            summary: summary.clone(),
        }
    }
}

/// Builds sanitized payload for event emission (no secrets).
pub fn to_requested_payload(record: &ApprovalRequestRecord) -> ApprovalRequestedPayload {
    let caller_str = match record.caller {
        PolicyCaller::Ui => "user",
        PolicyCaller::AgentOrchestrator => "agent",
        PolicyCaller::InternalSystem => "internal",
    };
    let target_str = match &record.target {
        PolicyTarget::FsPath { canonical_path } => canonical_path.clone(),
        PolicyTarget::ShellCommand {
            command_id,
            normalized_command: _,
        } => command_id.clone(),
        PolicyTarget::Resource {
            resource_id,
            api_name: _,
        } => resource_id.clone(),
    };
    let target_hint = match &record.target {
        PolicyTarget::FsPath { .. } => "path",
        PolicyTarget::ShellCommand { .. } => "command",
        PolicyTarget::Resource { .. } => "resource",
    }
    .to_string();
    let risk_hints = vec![record.reason_code.clone()];
    let (details_redacted, impact_summary, danger_zone) = match &record.operation_payload {
        PendingOperation::FsWrite {
            workspace_root,
            rel_path,
            contents,
        } => (
            serde_json::json!({
                "op": "fs_write",
                "workspace_root": workspace_root,
                "rel_path": rel_path,
                "contents_size_bytes": contents.len(),
                "contents_preview": "[redacted]",
            }),
            None,
            false,
        ),
        PendingOperation::FsDelete {
            workspace_root: _,
            rel_path,
        } => {
            let name = std::path::Path::new(rel_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(rel_path)
                .to_string();
            let impact = ImpactSummary {
                item_count: 1,
                affected_items: vec![AffectedItem {
                    name: name.clone(),
                    path: rel_path.clone(),
                }],
                operation_description: format!("Delete 1 item: {}", rel_path),
                target_path: None,
            };
            (
                serde_json::json!({
                    "op": "fs_delete",
                    "rel_path": rel_path,
                }),
                Some(impact),
                true, // Hard delete is irreversible
            )
        }
        PendingOperation::FsRename {
            workspace_root: _,
            rel_path,
            new_rel_path,
        } => {
            let name = std::path::Path::new(rel_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(rel_path)
                .to_string();
            let impact = ImpactSummary {
                item_count: 1,
                affected_items: vec![AffectedItem {
                    name,
                    path: rel_path.clone(),
                }],
                operation_description: format!("Rename to {}", new_rel_path),
                target_path: Some(new_rel_path.clone()),
            };
            (
                serde_json::json!({
                    "op": "fs_rename",
                    "rel_path": rel_path,
                    "new_rel_path": new_rel_path,
                }),
                Some(impact),
                false,
            )
        }
        PendingOperation::FsMove {
            workspace_root: _,
            rel_path,
            dest_workspace_root: _,
            dest_rel_path,
        } => {
            let name = std::path::Path::new(rel_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(rel_path)
                .to_string();
            let target = dest_rel_path.to_string();
            let impact = ImpactSummary {
                item_count: 1,
                affected_items: vec![AffectedItem {
                    name,
                    path: rel_path.clone(),
                }],
                operation_description: format!("Move 1 item to {}", target),
                target_path: Some(target),
            };
            (
                serde_json::json!({
                    "op": "fs_move",
                    "rel_path": rel_path,
                    "dest_rel_path": dest_rel_path,
                }),
                Some(impact),
                false,
            )
        }
        PendingOperation::ShellRun {
            workspace_root,
            command_id,
            params,
        } => (
            serde_json::json!({
                "op": "shell_run",
                "workspace_root": workspace_root,
                "command_id": command_id,
                "params_keys": params.as_object().map(|o| o.keys().collect::<Vec<_>>()),
                "params": "[redacted]",
            }),
            None,
            false,
        ),
    };
    ApprovalRequestedPayload {
        request_id: record.request_id.clone(),
        caller: caller_str.to_string(),
        tool_type: format!("{:?}", record.tool_type).to_lowercase(),
        operation: format!("{:?}", record.operation).to_lowercase(),
        target: target_str,
        target_hint,
        reason_code: record.reason_code.clone(),
        summary: record.summary.clone(),
        risk_hints,
        created_at_ms: 0,
        expires_at_ms: 300_000,
        details_redacted,
        impact_summary,
        danger_zone,
    }
}

impl PendingApprovalsStore {
    pub fn insert(&self, record: ApprovalRequestRecord) {
        self.cleanup_expired();
        self.inner
            .lock()
            .unwrap()
            .insert(record.request_id.clone(), record);
    }

    pub fn take(&self, request_id: &str) -> Option<ApprovalRequestRecord> {
        self.cleanup_expired();
        let mut guard = self.inner.lock().unwrap();
        let record = guard.remove(request_id)?;
        if record.is_expired(Instant::now()) {
            return None;
        }
        Some(record)
    }

    pub fn get(&self, request_id: &str) -> Option<ApprovalRequestRecord> {
        self.cleanup_expired();
        let guard = self.inner.lock().unwrap();
        guard.get(request_id).cloned()
    }

    fn cleanup_expired(&self) {
        let now = Instant::now();
        self.inner.lock().unwrap().retain(|_, v| !v.is_expired(now));
    }

    /// Cancel all pending approvals that reference this workspace (by root path).
    /// Used when a workspace is deleted so no approval can execute against it.
    pub fn cancel_pending_for_workspace_root(&self, root_path: &str) {
        let mut guard = self.inner.lock().unwrap();
        guard.retain(|_, record| {
            let payload = &record.operation_payload;
            let touches = match payload {
                PendingOperation::FsWrite { workspace_root, .. }
                | PendingOperation::FsDelete { workspace_root, .. }
                | PendingOperation::FsRename { workspace_root, .. }
                | PendingOperation::ShellRun { workspace_root, .. } => *workspace_root == root_path,
                PendingOperation::FsMove {
                    workspace_root,
                    dest_workspace_root,
                    ..
                } => *workspace_root == root_path || *dest_workspace_root == root_path,
            };
            !touches
        });
    }
}

impl Default for PendingApprovalsStore {
    fn default() -> Self {
        Self::new()
    }
}

// -----------------------------------------------------------------------------
// Router error (policy deny / approval required / underlying error)
// -----------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouterError {
    /// User config has disabled destructive operations.
    ConfigDisabled {
        message: String,
    },
    PolicyDenied {
        reason_code: String,
        message: String,
    },
    #[serde(rename = "approval_required")]
    ApprovalRequired {
        request_id: String,
        operation: String,
        summary: String,
        reason_code: String,
    },
    Fs {
        error_kind: String,
        message: String,
    },
    Shell {
        error_kind: String,
        message: String,
    },
}

impl std::fmt::Display for RouterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouterError::ConfigDisabled { message } => write!(f, "{}", message),
            RouterError::PolicyDenied { message, .. } => write!(f, "{}", message),
            RouterError::ApprovalRequired { summary, .. } => write!(f, "{}", summary),
            RouterError::Fs { message, .. } => write!(f, "{}", message),
            RouterError::Shell { message, .. } => write!(f, "{}", message),
        }
    }
}

impl std::error::Error for RouterError {}

impl From<fs::FsError> for RouterError {
    fn from(e: fs::FsError) -> Self {
        RouterError::Fs {
            error_kind: format!("{:?}", e.kind),
            message: e.message,
        }
    }
}

impl From<shell::ShellError> for RouterError {
    fn from(e: shell::ShellError) -> Self {
        RouterError::Shell {
            error_kind: format!("{:?}", e.kind),
            message: e.message,
        }
    }
}

// Tauri v2 provides impl<T: Serialize> From<T> for InvokeError, so commands
// returning Result<_, RouterError> are converted automatically; no custom impl.

// -----------------------------------------------------------------------------
// Router + HITL wiring tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn parse_caller_maps_agent_orchestrator() {
        assert!(matches!(
            parse_caller(Some("agent_orchestrator")),
            PolicyCaller::AgentOrchestrator
        ));
        assert!(matches!(
            parse_caller(Some("AGENT_ORCHESTRATOR")),
            PolicyCaller::AgentOrchestrator
        ));
    }

    #[test]
    fn parse_caller_maps_internal_system() {
        assert!(matches!(
            parse_caller(Some("internal_system")),
            PolicyCaller::InternalSystem
        ));
    }

    #[test]
    fn parse_caller_defaults_to_ui() {
        assert!(matches!(parse_caller(None), PolicyCaller::Ui));
        assert!(matches!(parse_caller(Some("unknown")), PolicyCaller::Ui));
    }

    #[test]
    fn visibility_main_hides_when_destructive_off() {
        let vis = get_destructive_command_visibility(false, CommandVisibilityContext::Main);
        assert_eq!(vis.get("fs_delete"), Some(&CommandVisibility::Hidden));
        assert_eq!(vis.get("fs_move"), Some(&CommandVisibility::Hidden));
    }

    #[test]
    fn visibility_advanced_disabled_with_explanation_when_destructive_off() {
        let vis = get_destructive_command_visibility(false, CommandVisibilityContext::Advanced);
        let del = vis.get("fs_delete").unwrap();
        match del {
            CommandVisibility::DisabledWithExplanation { message } => {
                assert!(message.contains("Allow destructive"));
            }
            _ => panic!("expected DisabledWithExplanation"),
        }
    }

    #[test]
    fn visibility_available_when_destructive_on() {
        let vis = get_destructive_command_visibility(true, CommandVisibilityContext::Main);
        assert_eq!(vis.get("fs_delete"), Some(&CommandVisibility::Available));
        assert_eq!(vis.get("fs_rename"), Some(&CommandVisibility::Available));
    }

    /// DENY / REQUIRE_APPROVAL: pending record lifecycle.
    #[test]
    fn pending_approvals_store_insert_and_take() {
        let store = PendingApprovalsStore::new();
        let record = store.create_record(
            "req-1".to_string(),
            PolicyCaller::Ui,
            ToolType::Fs,
            Operation::Write,
            PolicyTarget::FsPath {
                canonical_path: "/tmp/test".to_string(),
            },
            PendingOperation::FsWrite {
                workspace_root: "/tmp".to_string(),
                rel_path: "test.txt".to_string(),
                contents: vec![1, 2, 3],
            },
            "TEST".to_string(),
            "Test write".to_string(),
        );
        store.insert(record);
        let taken = store.take("req-1");
        assert!(taken.is_some());
        assert_eq!(taken.as_ref().unwrap().request_id, "req-1");
        assert!(matches!(
            taken.as_ref().unwrap().status,
            ApprovalStatus::Pending
        ));
        // Second take returns None (already removed)
        assert!(store.take("req-1").is_none());
    }

    /// Expired: command never executes (take returns None).
    #[test]
    fn pending_approvals_store_expired_returns_none() {
        let store = PendingApprovalsStore::with_timeout_secs(0);
        let record = store.create_record(
            "req-expired".to_string(),
            PolicyCaller::AgentOrchestrator,
            ToolType::Fs,
            Operation::Write,
            PolicyTarget::FsPath {
                canonical_path: "/tmp/test".to_string(),
            },
            PendingOperation::FsWrite {
                workspace_root: "/tmp".to_string(),
                rel_path: "expired.txt".to_string(),
                contents: vec![],
            },
            "TEST".to_string(),
            "Expired".to_string(),
        );
        store.insert(record);
        // With TTL=0, entry expires immediately; cleanup removes it before take
        std::thread::sleep(Duration::from_millis(10));
        let taken = store.take("req-expired");
        assert!(taken.is_none(), "expired request must not be retrievable");
    }

    /// Approved -> command can execute (store yields record for execution).
    #[test]
    fn pending_approvals_store_approved_record_retrievable() {
        let store = PendingApprovalsStore::with_timeout_secs(60);
        let record = store.create_record(
            "req-approve".to_string(),
            PolicyCaller::AgentOrchestrator,
            ToolType::Fs,
            Operation::Write,
            PolicyTarget::FsPath {
                canonical_path: "/tmp/approved".to_string(),
            },
            PendingOperation::FsWrite {
                workspace_root: "/tmp".to_string(),
                rel_path: "approved.txt".to_string(),
                contents: b"approved".to_vec(),
            },
            "AGENT_WRITE_REQUIRES_APPROVAL".to_string(),
            "Approved write".to_string(),
        );
        store.insert(record);
        let taken = store.take("req-approve");
        assert!(
            taken.is_some(),
            "approved request must be retrievable for execution"
        );
        let op = &taken.unwrap().operation_payload;
        match op {
            PendingOperation::FsWrite {
                rel_path, contents, ..
            } => {
                assert_eq!(rel_path, "approved.txt");
                assert_eq!(contents, b"approved");
            }
            _ => panic!("expected FsWrite"),
        }
    }

    /// Concurrent take: at most one caller retrieves the record; others get None.
    /// Ensures single-execution invariant—no double execution under race.
    #[test]
    fn pending_approvals_store_concurrent_take_single_execution() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let store = std::sync::Arc::new(PendingApprovalsStore::with_timeout_secs(60));
        let record = store.create_record(
            "req-concurrent".to_string(),
            PolicyCaller::AgentOrchestrator,
            ToolType::Fs,
            Operation::Write,
            PolicyTarget::FsPath {
                canonical_path: "/tmp/concurrent".to_string(),
            },
            PendingOperation::FsWrite {
                workspace_root: "/tmp".to_string(),
                rel_path: "concurrent.txt".to_string(),
                contents: vec![],
            },
            "TEST".to_string(),
            "Concurrent take test".to_string(),
        );
        store.insert(record);

        let execution_count = std::sync::Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let store_clone = std::sync::Arc::clone(&store);
            let exec_clone = std::sync::Arc::clone(&execution_count);
            handles.push(std::thread::spawn(move || {
                if let Some(rec) = store_clone.take("req-concurrent") {
                    exec_clone.fetch_add(1, Ordering::SeqCst);
                    Some(rec)
                } else {
                    None
                }
            }));
        }

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let retrieved: Vec<_> = results.into_iter().flatten().collect();
        assert_eq!(
            retrieved.len(),
            1,
            "exactly one caller must retrieve the record"
        );
        assert_eq!(
            execution_count.load(Ordering::SeqCst),
            1,
            "execution must occur at most once"
        );
    }
}
