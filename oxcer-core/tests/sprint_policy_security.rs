//! Security policy gap tests: path blocklist, UI vs Agent caller, default-deny.
//!
//! Complements policy_engine.rs and policy_config.rs unit tests with integration-style
//! scenarios that use load_from_yaml and evaluate_with_config.

use oxcer_core::security::policy_config::{evaluate_with_config, load_from_yaml_result};
use oxcer_core::security::policy_engine::{
    evaluate, Operation, PolicyCaller, PolicyDecisionKind, PolicyRequest, PolicyTarget, ToolType,
};

/// Path blocklist: paths matching ~/.ssh should be denied for both UI and Agent.
/// Uses default policy; path is constructed from HOME for portability.
#[test]
fn policy_path_blocklist_denies_ssh_for_all_callers() {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let ssh_path = format!("{}/.ssh/id_rsa", home.trim_end_matches('/'));
    let ssh_config_path = format!("{}/.ssh/config", home.trim_end_matches('/'));

    let default_policy = include_bytes!("../policies/default.yaml");
    let cfg = load_from_yaml_result(default_policy).expect("default policy valid");

    let req_ui = PolicyRequest {
        caller: PolicyCaller::Ui,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: ssh_path.clone(),
        },
        ..Default::default()
    };
    let dec_ui = evaluate_with_config(&req_ui, &cfg);
    assert_eq!(
        dec_ui.decision,
        PolicyDecisionKind::Deny,
        "UI must not read ~/.ssh"
    );

    let req_agent = PolicyRequest {
        caller: PolicyCaller::AgentOrchestrator,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: ssh_config_path,
        },
        ..Default::default()
    };
    let dec_agent = evaluate_with_config(&req_agent, &cfg);
    assert_eq!(
        dec_agent.decision,
        PolicyDecisionKind::Deny,
        "Agent must not read ~/.ssh"
    );
}

/// Agent FS delete on allowed path → RequireApproval (destructive rule).
#[test]
fn policy_agent_delete_requires_approval() {
    let req = PolicyRequest {
        caller: PolicyCaller::AgentOrchestrator,
        tool_type: ToolType::Fs,
        operation: Operation::Delete,
        target: PolicyTarget::FsPath {
            canonical_path: "/tmp/workspace/foo.txt".to_string(),
        },
        ..Default::default()
    };
    let dec = evaluate(req);
    assert_eq!(
        dec.decision,
        PolicyDecisionKind::RequireApproval,
        "agent delete must require approval"
    );
}

/// UI FS read on non-blocked path → Allow.
#[test]
fn policy_ui_read_allowed_for_workspace_path() {
    let req = PolicyRequest {
        caller: PolicyCaller::Ui,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: "/Users/dev/projects/oxcer/src/main.rs".to_string(),
        },
        ..Default::default()
    };
    let dec = evaluate(req);
    assert_eq!(
        dec.decision,
        PolicyDecisionKind::Allow,
        "UI read on workspace path allowed"
    );
}
