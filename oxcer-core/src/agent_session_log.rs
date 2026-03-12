//! Agent session log: structured log for each agent_request for explainability and evaluation.
//!
//! Persist via event_log or a dedicated JSONL file (e.g. logs/agent_sessions.jsonl) with
//! 30-day/10MB retention. Enables "Why did the agent do X?", per-strategy evaluation,
//! and safety tooling.
//!
//! **Security:** Only scrubbed content is stored. Raw secrets are never written to plain JSON logs.
//! `from_completed_session` scrubs `user_input`, each step's `result_summary`, and the `task` field
//! in llm_generate args via the data_sensitivity classifier.

use serde::{Deserialize, Serialize};

use crate::data_sensitivity;
use crate::semantic_router::RouterDecision;

/// Kind of step in an agent session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum AgentStepKind {
    ModelCall,
    ToolCall,
    ApprovalWait,
    System,
}

/// Log entry for a single model call (e.g. LlmGenerate).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelCallLog {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_summary: Option<String>,
}

/// Max length for result_summary in persisted logs (truncate for storage).
const RESULT_SUMMARY_MAX_LEN: usize = 500;

fn truncate_result(s: &str) -> String {
    if s.len() <= RESULT_SUMMARY_MAX_LEN {
        s.to_string()
    } else {
        format!("{}...", &s[..RESULT_SUMMARY_MAX_LEN])
    }
}

/// Scrubs text so it is safe to store in plain JSON logs. Never persist raw secrets.
fn scrub_for_log(s: &str) -> String {
    data_sensitivity::classify_and_mask_default(s).masked_content
}

/// For llm_generate steps, scrub the "task" field in args so we never store raw payloads.
fn scrub_tool_args_for_log(tool_name: &str, args: &serde_json::Value) -> serde_json::Value {
    if tool_name != "llm_generate" {
        return args.clone();
    }
    let obj = match args.as_object() {
        Some(o) => o.clone(),
        None => return args.clone(),
    };
    let mut out = serde_json::Map::new();
    for (k, v) in obj {
        if k == "task" {
            if let Some(t) = v.as_str() {
                out.insert(k, serde_json::Value::String(scrub_for_log(t)));
            } else {
                out.insert(k, v);
            }
        } else {
            out.insert(k, v);
        }
    }
    serde_json::Value::Object(out)
}

/// Log entry for a single tool call: request, policy, approval, result.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolCallLog {
    pub tool_name: String,
    pub args: serde_json::Value,
    /// allowed | denied | approval_required
    pub policy_decision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    /// approved | denied
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
}

/// One step in the session (model call, tool call, approval wait, or system).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentStepLog {
    pub step_index: u32,
    pub kind: AgentStepKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_call: Option<ModelCallLog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCallLog>,
}

/// Full session log: persisted per agent_request for explainability and evaluation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSessionLog {
    pub session_id: String,
    /// RFC3339 timestamp when the session completed (for retention).
    pub completed_at: String,
    pub workspace_id: String,
    pub user_input: String,
    pub router_decision: RouterDecision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_model: Option<String>,
    pub steps: Vec<AgentStepLog>,
}

impl AgentSessionLog {
    /// Build from session state and task input. Call when session completes (Complete outcome).
    pub fn from_completed_session(
        session_id: &str,
        user_input: &str,
        workspace_id: &str,
        router_decision: &RouterDecision,
        selected_model: Option<&str>,
        tool_traces: &[crate::orchestrator::ToolTrace],
        final_answer: Option<&str>,
    ) -> Self {
        let completed_at = chrono::Utc::now().to_rfc3339();
        let mut steps = Vec::new();

        for (i, trace) in tool_traces.iter().enumerate() {
            let policy_decision = trace
                .policy_decision
                .as_ref()
                .map(|d| match d {
                    crate::orchestrator::PolicyDecisionKind::Allow => "allowed",
                    crate::orchestrator::PolicyDecisionKind::Deny => "denied",
                    crate::orchestrator::PolicyDecisionKind::RequireApproval => "approval_required",
                })
                .unwrap_or("unknown")
                .to_string();
            let approval_outcome = trace
                .approved
                .map(|b| if b { "approved" } else { "denied" }.to_string());

            let result_summary = trace
                .result_summary
                .as_deref()
                .map(|s| truncate_result(&scrub_for_log(s)));
            let tool_call = ToolCallLog {
                tool_name: trace.tool_name.clone(),
                args: scrub_tool_args_for_log(&trace.tool_name, &trace.input),
                policy_decision,
                approval_id: None,
                approval_outcome,
                result_summary,
            };

            steps.push(AgentStepLog {
                step_index: i as u32,
                kind: if trace.tool_name == "llm_generate" {
                    AgentStepKind::ModelCall
                } else {
                    AgentStepKind::ToolCall
                },
                model_call: if trace.tool_name == "llm_generate" {
                    Some(ModelCallLog {
                        model_id: selected_model.map(String::from),
                        response_summary: trace
                            .result_summary
                            .as_deref()
                            .map(|s| truncate_result(&scrub_for_log(s))),
                        ..Default::default()
                    })
                } else {
                    None
                },
                tool_call: Some(tool_call),
            });
        }

        if steps.is_empty() && final_answer.is_some() {
            steps.push(AgentStepLog {
                step_index: 0,
                kind: AgentStepKind::ModelCall,
                model_call: Some(ModelCallLog {
                    response_summary: final_answer.map(|s| scrub_for_log(s)),
                    ..Default::default()
                }),
                tool_call: None,
            });
        }

        Self {
            session_id: session_id.to_string(),
            completed_at,
            workspace_id: workspace_id.to_string(),
            user_input: scrub_for_log(user_input),
            router_decision: router_decision.clone(),
            selected_model: selected_model.map(String::from),
            steps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::ToolTrace;
    use crate::semantic_router::{RouterFlags, Strategy, TaskCategory};

    #[test]
    fn from_completed_session_scrubs_user_input_and_llm_task() {
        let router_decision = RouterDecision {
            category: TaskCategory::SimpleQa,
            strategy: Strategy::CheapModel,
            flags: RouterFlags::default(),
            tool_hints: Some(vec![]),
        };
        let traces = vec![ToolTrace {
            tool_name: "llm_generate".to_string(),
            input: serde_json::json!({
                "task": "Use key AKIAIOSFODNN7EXAMPLE for AWS.",
                "strategy": "CheapModel"
            }),
            policy_decision: Some(crate::orchestrator::PolicyDecisionKind::Allow),
            approved: Some(true),
            result_summary: Some("Done.".to_string()),
        }];
        let log = AgentSessionLog::from_completed_session(
            "s1",
            "My secret key is AKIAIOSFODNN7EXAMPLE",
            "ws1",
            &router_decision,
            None,
            &traces,
            None,
        );
        assert!(!log.user_input.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(log.user_input.contains("[REDACTED:"));
        let args = &log.steps[0].tool_call.as_ref().unwrap().args;
        let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!task.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(task.contains("[REDACTED:"));
    }
}
