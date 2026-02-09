//! Orchestrator failure-path tests: empty plan, step error propagation, agent_request with stub.
//!
//! Covers behaviors that must hold when tools fail or the planner produces no steps.

use oxcer_core::orchestrator::{
    agent_request, agent_step, start_session, AgentConfig, AgentSessionState, AgentTaskInput,
    AgentToolExecutor, StepResult, ToolCallIntent,
};
use oxcer_core::semantic_router::{RouterConfig, RouterInput, Strategy, TaskContext};

/// Empty plan: tools_only task with no workspace_root → plan is empty, first intent is None.
#[test]
fn orchestrator_empty_plan_when_no_workspace_root() {
    let input = RouterInput {
        task_description: "list files".to_string(),
        context: TaskContext::default(),
        config: RouterConfig {
            prefer_tools_only: true,
            ..Default::default()
        },
        capabilities: None,
    };
    let (session, first) = start_session(
        "s1".to_string(),
        input,
        Some("ws1".to_string()),
        None, // no workspace_root
    );
    assert!(session.plan.is_empty());
    assert!(first.is_none());
}

/// Step error: when last_result is Err, next_action returns Complete with error message.
#[test]
fn orchestrator_step_error_propagates_to_complete() {
    let mut session = AgentSessionState::new("s1".to_string(), "What is Rust?".to_string());
    session.state = oxcer_core::orchestrator::TaskState::Executing;
    session.plan = vec![ToolCallIntent::LlmGenerate {
        strategy: Strategy::CheapModel,
        task: "What is Rust?".to_string(),
        system_hint: None,
    }];
    session.step_index = 0;

    let config = AgentConfig::default();
    let input = AgentTaskInput {
        task_description: "What is Rust?".to_string(),
        context: TaskContext::default(),
    };
    let outcome = agent_step(
        input,
        &mut session,
        &config,
        Some(StepResult::Err {
            message: "LLM API error: 503".to_string(),
        }),
    )
    .unwrap();

    match outcome {
        oxcer_core::orchestrator::AgentStepOutcome::Complete(result) => {
            assert!(result.final_answer.as_ref().map_or(false, |s| s.contains("503")));
            assert_eq!(result.tool_traces.len(), 1);
            assert_eq!(result.tool_traces[0].result_summary.as_deref(), Some("LLM API error: 503"));
        }
        _ => panic!("expected Complete with error message"),
    }
}

/// Stub executor: agent_request with executor that returns Err on tools → propagates Err.
struct StubErrExecutor;

impl AgentToolExecutor for StubErrExecutor {
    fn execute_tool(&self, intent: ToolCallIntent) -> Result<oxcer_core::orchestrator::ToolOutcome, String> {
        let _ = intent;
        Err("stub: tool execution not available".to_string())
    }
    fn resolve_approval(&self, _request_id: &str, _approved: bool) -> Result<serde_json::Value, String> {
        Err("stub: approval not available".to_string())
    }
}

#[test]
fn agent_request_fails_when_executor_returns_error() {
    let mut session = AgentSessionState::new("test-session-001".to_string(), "delete foo.txt".to_string());
    let config = AgentConfig {
        default_workspace_id: Some("ws1".to_string()),
        default_workspace_root: Some("/tmp/ws".to_string()),
        router_config: RouterConfig {
            prefer_tools_only: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let input = AgentTaskInput {
        task_description: "delete foo.txt".to_string(),
        context: TaskContext::default(),
    };
    let executor = StubErrExecutor;
    let result = agent_request(input, &mut session, &config, &executor);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("stub"));
}
