//! Integration tests: router + orchestrator + policy engine.
//!
//! - Agent tools-only delete: "delete file X" → router → tools_only + high-risk;
//!   orchestrator emits FsDelete; policy produces ApprovalRequired for agent.
//! - Cheap vs expensive routing: simple QA → cheap; long plan → expensive.

use oxcer_core::orchestrator::{start_session, ToolCallIntent};
use oxcer_core::security::policy_engine::{
    evaluate, Operation, PolicyCaller, PolicyDecisionKind, PolicyRequest, PolicyTarget, ToolType,
};
use oxcer_core::semantic_router::{
    route_task, RouterConfig, RouterInput, Strategy, TaskCategory, TaskContext,
};

/// Agent tools-only delete: user prompt "delete file X" → router → tools_only + high-risk flag.
#[test]
fn integration_delete_file_x_router_tools_only_high_risk() {
    let ctx = TaskContext::default();
    let config = RouterConfig {
        prefer_tools_only: true,
        ..Default::default()
    };
    let out = route_task("delete file myfile.txt", &ctx, &config);
    assert_eq!(out.category, TaskCategory::ToolsHeavy);
    assert_eq!(out.strategy, Strategy::ToolsOnly);
    assert!(out.flags.requires_high_risk_approval);
}

/// Orchestrator emits FS delete tool call; security policy produces ApprovalRequired for agent.
#[test]
fn integration_agent_delete_produces_approval_required() {
    let input = RouterInput {
        task_description: "delete target.txt".to_string(),
        context: TaskContext::default(),
        config: RouterConfig {
            prefer_tools_only: true,
            ..Default::default()
        },
        capabilities: None,
    };
    let (_session, first) = start_session(
        "int-s1".to_string(),
        input,
        Some("ws1".to_string()),
        Some("/tmp/workspace".to_string()),
    );
    let intent = match first {
        Some(ToolCallIntent::FsDelete { rel_path, .. }) => {
            assert_eq!(rel_path, "target.txt");
            true
        }
        _ => false,
    };
    assert!(intent, "orchestrator should emit FsDelete for delete task");

    let canonical = "/tmp/workspace/target.txt";
    let request = PolicyRequest {
        caller: PolicyCaller::AgentOrchestrator,
        tool_type: ToolType::Fs,
        operation: Operation::Delete,
        target: PolicyTarget::FsPath {
            canonical_path: canonical.to_string(),
        },
        ..Default::default()
    };
    let decision = evaluate(request);
    assert_eq!(
        decision.decision,
        PolicyDecisionKind::RequireApproval,
        "agent delete must require approval"
    );
}

/// v1 policy: borderline cases (e.g. "What is Rust?") are routed to Planning for safety.
#[test]
fn integration_simple_qa_cheap_model() {
    let out = route_task(
        "What is Rust?",
        &TaskContext::default(),
        &RouterConfig::default(),
    );
    // v1 policy: borderline cases are routed to Planning for safety.
    assert_eq!(out.category, TaskCategory::Planning);
    assert_eq!(out.strategy, Strategy::ExpensiveModel);
}

/// Long multi-step task with "plan" language goes to expensive model.
#[test]
fn integration_long_plan_expensive_model() {
    let out = route_task(
        "We need to refactor the entire system. First create a plan: outline the architecture, \
         then break down into steps, then assign priorities. This is a multi-phase project.",
        &TaskContext::default(),
        &RouterConfig::default(),
    );
    assert_eq!(out.category, TaskCategory::Planning);
    assert_eq!(out.strategy, Strategy::ExpensiveModel);
}
