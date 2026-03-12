//! Execution loop: applies tool results, triggers plan expansion, emits next intent.
//! Also contains the narration sanitizer, precondition guards, and all unit tests.

use super::planning::{
    do_expand_plan, start_session,
    FS_RESULT_PLACEHOLDER, FILE_CONTENTS_PLACEHOLDER, MOST_RECENT_FILE_PLACEHOLDER,
};
use super::types::{
    AgentConfig, AgentSessionState, AgentStepOutcome, AgentTaskInput, AgentTaskResult,
    OrchestratorAction, SessionKind, SessionState, StepResult, TaskState,
    ToolCallIntent, build_tool_trace, format_tool_call, intent_tool_name,
};

// -----------------------------------------------------------------------------
// Context-window budget constants
// -----------------------------------------------------------------------------

/// Maximum characters of file content injected into an `LlmGenerate.task` via
/// `{{FS_RESULT}}`.  Characters beyond this limit are dropped and a truncation
/// notice is appended so the model (and the user) know the file was partial.
///
/// ~4 000 characters ≈ ~1 000 tokens, which leaves comfortable headroom for the
/// prompt frame, system hint, and generation budget inside an 8 192-token context.
const FS_RESULT_MAX_CHARS: usize = 4_000;

/// Suffix appended to truncated file content before LLM injection.
const FS_RESULT_TRUNCATION_NOTICE: &str =
    "\n\n[Note: file content was truncated to fit the model's context window.]";

// -----------------------------------------------------------------------------
// LlmGenerate output sanitizer
// -----------------------------------------------------------------------------

/// Phrases that indicate the model narrated internal tool calls instead of
/// writing the requested prose.  All patterns are matched case-insensitively.
///
/// Add new entries here to extend the filter without touching any other code.
const NARRATION_PATTERNS: &[&str] = &[
    "fs_list_dir",
    "fs_read_file",
    "fs_write_file",
    "i'll use ",
    "i will use ",
    "i will now call ",
    "first, i'll ",
    "next, i'll ",
    "now, i'll ",
];

/// Returned by [`sanitize_llm_generate_output`] when a forbidden narration
/// pattern is detected in the raw model output.
#[derive(Debug)]
pub(crate) struct SanitizedError {
    pub message: String,
}

/// Accepts a raw LlmGenerate response and returns `Ok(text)` when the text is
/// clean prose, or `Err(SanitizedError)` when it contains tool-narration
/// patterns that indicate the model described tool calls instead of summarising.
///
/// The check is case-insensitive and purely substring-based — deterministic,
/// zero-allocation on the happy path, and trivially extendable via
/// `NARRATION_PATTERNS`.
fn sanitize_llm_generate_output(raw: &str) -> Result<String, SanitizedError> {
    let lower = raw.to_lowercase();
    for pattern in NARRATION_PATTERNS {
        if lower.contains(*pattern) {
            return Err(SanitizedError {
                message: "The model tried to describe internal tool calls instead of \
                          summarizing the provided document contents. \
                          Please retry or adjust the prompt."
                    .to_string(),
            });
        }
    }
    Ok(raw.to_string())
}

// -----------------------------------------------------------------------------
// LlmGenerate precondition guard
// -----------------------------------------------------------------------------

/// Returns `Some(error_message)` when the `LlmGenerate` step at `step_idx`
/// must NOT be emitted because its data preconditions have not been satisfied.
///
/// # Guard A — `{{FILE_CONTENTS}}` with empty accumulator
/// If the planned task string contains `FILE_CONTENTS_PLACEHOLDER` but
/// `session.content_accumulator` is empty, no `FsReadFile` result has arrived
/// yet. Emitting the `LlmGenerate` would give the model an empty "FILE CONTENTS"
/// section and invite it to hallucinate file summaries in natural language.
/// We surface a clear error instead.
///
/// # Guard B — `{{FS_RESULT}}` with no accumulated response
/// If the task still contains `FS_RESULT_PLACEHOLDER` and `accumulated_response`
/// is `None`, the preceding filesystem tool has not returned real data.
///
/// Note: this function inspects the ORIGINAL plan entry (before placeholder
/// substitution), so the checks are accurate even when called before the
/// substitution block runs.
fn check_llm_generate_precondition(
    session: &SessionState,
    step_idx: usize,
) -> Option<String> {
    let ToolCallIntent::LlmGenerate { task, .. } = session.plan.get(step_idx)? else {
        return None; // not an LlmGenerate step; no precondition to check
    };

    // Guard A: FILE_CONTENTS requires at least one successful FsReadFile result.
    if task.contains(FILE_CONTENTS_PLACEHOLDER) && session.content_accumulator.is_empty() {
        return Some(
            "Cannot generate summary: no file contents were loaded. \
             Check that the target files exist in the specified directory and retry."
                .to_string(),
        );
    }

    // Guard B: FS_RESULT requires that the preceding filesystem tool produced
    // a real text result. After any successful tool call, `accumulated_response`
    // is set to either the `text` field (good: meaningful prose) or a JSON
    // serialisation of the full payload (not useful for summarisation). Detect
    // the "JSON blob fallback" case by checking whether the string starts with
    // `{` — if so, the model never received readable file content.
    if task.contains(FS_RESULT_PLACEHOLDER) {
        let has_real_text = session
            .accumulated_response
            .as_deref()
            .map(|s| !s.is_empty() && !s.starts_with('{'))
            .unwrap_or(false);
        if !has_real_text {
            return Some(
                "Cannot generate answer: the filesystem tool did not return readable text. \
                 Check that the target file exists and is a plain-text file."
                    .to_string(),
            );
        }
    }

    None
}

// -----------------------------------------------------------------------------
// Next action (state machine step)
// -----------------------------------------------------------------------------

/// Advances the session: apply last tool result (if any) and return next action.
/// Runner calls this after executing a ToolCallIntent or after user approval.
/// Records tool traces and approvals_requested for logging / UI.
pub fn next_action(
    mut session: SessionState,
    last_result: Option<StepResult>,
) -> Result<OrchestratorAction, String> {
    // Apply last result if present: record trace (except for ApprovalPending), then update state
    if let Some(res) = &last_result {
        match res {
            StepResult::ApprovalPending { request_id } => {
                session.approvals_requested.push(request_id.clone());
                return Ok(OrchestratorAction::AwaitingApproval {
                    request_id: request_id.clone(),
                    session,
                });
            }
            _ => {
                if session.step_index < session.plan.len() {
                    let intent = &session.plan[session.step_index];
                    let trace = build_tool_trace(intent, res);
                    session.tool_traces.push(trace);
                }
            }
        }
        match res {
            StepResult::ApprovalPending { .. } => unreachable!(),
            StepResult::Err { message } => {
                session.state = TaskState::Complete;
                session.accumulated_response = Some(format!("Error: {}", message));
                session
                    .intermediate_observations
                    .push(format!("Step failed: {}", message));
                return Ok(OrchestratorAction::Complete {
                    response: session.accumulated_response.clone().unwrap_or_default(),
                    session,
                });
            }
            StepResult::Ok { payload } => {
                // Record confirmed workspace root from the first successful FS tool.
                if session.confirmed_root.is_none() {
                    match session.plan.get(session.step_index) {
                        Some(ToolCallIntent::FsListDir { workspace_root, .. })
                        | Some(ToolCallIntent::FsReadFile { workspace_root, .. }) => {
                            session.confirmed_root = Some(workspace_root.clone());
                        }
                        _ => {}
                    }
                }
                // Capture mtime-sorted filenames for {{MOST_RECENT_FILE}} resolution.
                // Swift encodes the field as "sortedByModified" (camelCase).
                if let Some(arr) = payload.get("sortedByModified").and_then(|v| v.as_array()) {
                    session.last_dir_listing_sorted = arr
                        .iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect();
                }
                // Accumulate FsReadFile text content for {{FILE_CONTENTS}} substitution
                // in multi-file summarise workflows.
                if matches!(
                    session.plan.get(session.step_index),
                    Some(ToolCallIntent::FsReadFile { .. })
                ) {
                    if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
                        session.content_accumulator.push(text.to_string());
                    }
                }
                if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
                    // For Task-session LlmGenerate steps, run the narration sanitizer
                    // before storing the output.  Chat sessions are exempt: a response
                    // like "First, I'll explain..." is valid prose for Q&A but looks
                    // like tool narration to the sanitizer.
                    if session.kind == SessionKind::Task
                        && matches!(
                            session.plan.get(session.step_index),
                            Some(ToolCallIntent::LlmGenerate { .. })
                        )
                    {
                        match sanitize_llm_generate_output(text) {
                            Ok(cleaned) => {
                                session.accumulated_response = Some(cleaned);
                            }
                            Err(e) => {
                                session.state = TaskState::Complete;
                                session.accumulated_response = Some(e.message.clone());
                                session
                                    .intermediate_observations
                                    .push(format!("LlmGenerate sanitizer blocked narration: {}", e.message));
                                return Ok(OrchestratorAction::Complete {
                                    response: e.message,
                                    session,
                                });
                            }
                        }
                    } else {
                        session.accumulated_response = Some(text.to_string());
                    }
                } else if let Some(s) = serde_json::to_string(payload).ok() {
                    session.accumulated_response = Some(s);
                }
                session.step_index += 1;
            }
        }
    }

    // Trigger dynamic plan expansion once, immediately after FsListDir completes.
    // `pending_expansion` is taken (not cloned) so it is never applied twice.
    if let Some(expansion) = session.pending_expansion.take() {
        let just_done = session.step_index.saturating_sub(1);
        if matches!(
            session.plan.get(just_done),
            Some(ToolCallIntent::FsListDir { .. })
        ) {
            do_expand_plan(&mut session, expansion);
            // ── Expanded-plan log ─────────────────────────────────────────────
            // Emit the full plan after FsListDir-driven expansion so we can see
            // every FsReadFile / FsMove / FsCreateDir that was spliced in.
            {
                let tag = session.debug_tag.as_deref().unwrap_or("");
                let steps: Vec<String> = session.plan.iter().map(format_tool_call).collect();
                let steps_str = steps
                    .iter()
                    .enumerate()
                    .map(|(i, s)| format!("[{i}] {s}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                agent_event!(DEBUG, session.session_id, "plan_expanded",
                    tag = tag,
                    plan_len = session.plan.len(),
                    plan = %steps_str,
                );
            }
            // ─────────────────────────────────────────────────────────────────
        } else {
            // FsListDir hasn't run yet — put the expansion back.
            session.pending_expansion = Some(expansion);
        }
    }

    // Initial step (no last_result): emit first intent
    if last_result.is_none() && !session.plan.is_empty() {
        let intent = session.plan[0].clone();
        // ── Step log (initial) ────────────────────────────────────────────────
        {
            let tag = session.debug_tag.as_deref().unwrap_or("");
            agent_event!(DEBUG, session.session_id, "plan_step",
                tag = tag,
                step_index = 0usize,
                intent = %format_tool_call(&intent),
            );
        }
        // ─────────────────────────────────────────────────────────────────────
        return Ok(OrchestratorAction::ToolCall {
            intent,
            session,
        });
    }

    // More steps?
    if session.step_index < session.plan.len() {
        // Guard: refuse to emit an LlmGenerate whose data preconditions haven't
        // been met ({{FILE_CONTENTS}} without reads, or {{FS_RESULT}} that is a
        // JSON blob).  Only applies to Task sessions — Chat sessions have no FS
        // tools so their placeholders are never present, and the guard would be
        // a no-op anyway; more importantly, running it for Chat avoids any risk
        // of a filesystem-specific error message appearing for a Q&A response.
        if session.kind == SessionKind::Task {
            if let Some(err) = check_llm_generate_precondition(&session, session.step_index) {
                session.state = TaskState::Complete;
                session.accumulated_response = Some(err.clone());
                return Ok(OrchestratorAction::Complete {
                    response: err,
                    session,
                });
            }
        }

        let mut intent = session.plan[session.step_index].clone();

        // Resolve {{MOST_RECENT_FILE}} in FsReadFile.rel_path using the mtime-sorted
        // filenames captured from the preceding FsListDir payload.
        //
        // Guard: if the directory was empty (or contained no readable files), there
        // is nothing to read.  Return an early Complete with a user-visible message
        // rather than forwarding a bogus path like "(no files found)" to the executor.
        if let ToolCallIntent::FsReadFile { ref rel_path, .. } = intent {
            if rel_path.contains(MOST_RECENT_FILE_PLACEHOLDER) {
                match session.last_dir_listing_sorted.first().cloned() {
                    Some(resolved) => {
                        if let ToolCallIntent::FsReadFile { ref mut rel_path, .. } = intent {
                            *rel_path = rel_path.replace(MOST_RECENT_FILE_PLACEHOLDER, &resolved);
                        }
                    }
                    None => {
                        session.state = TaskState::Complete;
                        let msg = "No recent files found in the target directory.".to_string();
                        session.accumulated_response = Some(msg.clone());
                        return Ok(OrchestratorAction::Complete {
                            response: msg,
                            session,
                        });
                    }
                }
            }
        }

        // Substitute {{FILE_CONTENTS}} with the accumulated multi-file read results.
        // Done before {{FS_RESULT}} so both placeholders can coexist in a single task string.
        if let ToolCallIntent::LlmGenerate { ref mut task, .. } = intent {
            if task.contains(FILE_CONTENTS_PLACEHOLDER) {
                let contents = if session.content_accumulator.is_empty() {
                    session
                        .accumulated_response
                        .as_deref()
                        .unwrap_or("(no content)")
                        .to_string()
                } else {
                    session.content_accumulator.join("\n\n---\n\n")
                };
                *task = task.replace(FILE_CONTENTS_PLACEHOLDER, &contents);
            }
        }

        // If the next intent is an LlmGenerate whose task contains the FS result
        // placeholder, substitute it with the real accumulated tool output before
        // emitting — this prevents the model from hallucinating filesystem content.
        //
        // The content is capped at `FS_RESULT_MAX_CHARS` before injection.  Any
        // file larger than that would overflow the local model's context window,
        // producing a confused or silently truncated summary.  The truncation notice
        // is appended so the model (and the user reading the output) can see why
        // the summary may be incomplete.
        if let ToolCallIntent::LlmGenerate { ref mut task, .. } = intent {
            if task.contains(FS_RESULT_PLACEHOLDER) {
                let raw = session
                    .accumulated_response
                    .as_deref()
                    .unwrap_or("(no tool result available)");
                let capped: String = if raw.chars().count() > FS_RESULT_MAX_CHARS {
                    let truncated: String = raw.chars().take(FS_RESULT_MAX_CHARS).collect();
                    format!("{truncated}{FS_RESULT_TRUNCATION_NOTICE}")
                } else {
                    raw.to_string()
                };
                *task = task.replace(FS_RESULT_PLACEHOLDER, &capped);
            }
        }

        // ── Step log (post-substitution) ──────────────────────────────────────
        // Logged after all placeholder substitutions so the description reflects
        // what the runner actually receives.  LlmGenerate.task is truncated to
        // 120 chars inside format_tool_call to keep logs readable.
        {
            let tag = session.debug_tag.as_deref().unwrap_or("");
            agent_event!(DEBUG, session.session_id, "plan_step",
                tag = tag,
                step_index = session.step_index,
                intent = %format_tool_call(&intent),
            );
        }
        // ─────────────────────────────────────────────────────────────────────

        return Ok(OrchestratorAction::ToolCall {
            intent,
            session,
        });
    }

    // No more steps
    session.state = TaskState::Complete;
    let response = session
        .accumulated_response
        .clone()
        .unwrap_or_else(|| "Done.".to_string());
    Ok(OrchestratorAction::Complete {
        response,
        session,
    })
}

fn build_agent_task_result(session: &SessionState) -> AgentTaskResult {
    AgentTaskResult {
        final_answer: session.accumulated_response.clone(),
        tool_traces: session.tool_traces.clone(),
    }
}

/// Executor for tool runs: used by `agent_request` to run tools and resolve approvals.
/// Implement this to drive the agent to completion (e.g. Tauri commands with caller "agent_orchestrator").
pub trait AgentToolExecutor {
    /// Execute one tool intent. Returns outcome or error.
    fn execute_tool(&self, intent: ToolCallIntent) -> Result<super::types::ToolOutcome, String>;
    /// Block until approval is resolved and return the execution result (or error if denied).
    fn resolve_approval(&self, request_id: &str, approved: bool) -> Result<serde_json::Value, String>;
}

/// Agent step (API): one step of the orchestrator. Update session in place; return outcome.
/// First call: pass `last_result: None`; orchestrator runs `route_task`, builds plan, returns `NeedTool` or `Complete`.
/// Subsequent calls: pass the previous step's result; returns next `NeedTool`, `AwaitingApproval`, or `Complete`.
pub fn agent_step(
    input: AgentTaskInput,
    session: &mut AgentSessionState,
    config: &AgentConfig,
    last_result: Option<StepResult>,
) -> Result<AgentStepOutcome, String> {
    // ── Tracing: entry ────────────────────────────────────────────────────────
    let last_result_tag = match &last_result {
        None => "none",
        Some(StepResult::Ok { .. }) => "ok",
        Some(StepResult::Err { .. }) => "err",
        Some(StepResult::ApprovalPending { .. }) => "approval_pending",
    };
    agent_event!(DEBUG, session.session_id, "agent_step_enter",
        state = ?session.state,
        step_index = session.step_index,
        plan_len = session.plan.len(),
        last_result = last_result_tag,
    );
    // ─────────────────────────────────────────────────────────────────────────

    if session.state == TaskState::Initial && last_result.is_none() {
        let router_input = crate::semantic_router::RouterInput {
            task_description: input.task_description,
            context: input.context,
            config: config.router_config.clone(),
            capabilities: None,
        };
        let (new_session, first_intent) = start_session(
            session.session_id.clone(),
            router_input,
            config.default_workspace_id.clone(),
            config.default_workspace_root.clone(),
        );
        *session = new_session;
        // ── Tracing: init outcome ─────────────────────────────────────────────
        let first_desc = first_intent.as_ref()
            .map(|i| intent_tool_name(i))
            .unwrap_or_else(|| "none".to_string());
        agent_event!(INFO, session.session_id, "agent_step_init",
            first_intent = %first_desc,
            plan_len = session.plan.len(),
        );
        // ─────────────────────────────────────────────────────────────────────
        return match first_intent {
            Some(intent) => Ok(AgentStepOutcome::NeedTool {
                intent,
                session: session.clone(),
            }),
            None => Ok(AgentStepOutcome::Complete(build_agent_task_result(session))),
        };
    }
    if let Some(result) = last_result {
        let action = next_action(session.clone(), Some(result))?;
        match action {
            OrchestratorAction::Complete { session: s, .. } => {
                *session = s;
                agent_event!(INFO, session.session_id, "agent_step_done", outcome = "complete",);
                Ok(AgentStepOutcome::Complete(build_agent_task_result(session)))
            }
            OrchestratorAction::ToolCall { intent, session: s } => {
                *session = s;
                agent_event!(INFO, session.session_id, "agent_step_done",
                    outcome = "need_tool",
                    intent = %intent_tool_name(&intent),
                    step_index = session.step_index,
                    plan_len = session.plan.len(),
                );
                Ok(AgentStepOutcome::NeedTool {
                    intent,
                    session: session.clone(),
                })
            }
            OrchestratorAction::AwaitingApproval { request_id, session: s } => {
                *session = s;
                agent_event!(INFO, session.session_id, "agent_step_done",
                    outcome = "awaiting_approval",
                    request_id = %request_id,
                );
                Ok(AgentStepOutcome::AwaitingApproval {
                    request_id,
                    session: session.clone(),
                })
            }
        }
    } else {
        tracing::error!(
            session_id = %session.session_id,
            event = "agent_step_error",
            state = ?session.state,
            "last_result required when session already executing"
        );
        Err("last_result required when session is already executing".to_string())
    }
}

/// Run the agent to completion using the given executor. Sync: blocks on tool execution and approval.
/// For frontend-driven execution (no blocking executor), use `agent_step` in a loop instead.
pub fn agent_request(
    input: AgentTaskInput,
    session: &mut AgentSessionState,
    config: &AgentConfig,
    executor: &impl AgentToolExecutor,
) -> Result<AgentTaskResult, String> {
    let mut last_result: Option<StepResult> = None;
    loop {
        let outcome = agent_step(input.clone(), session, config, last_result.take())?;
        match outcome {
            AgentStepOutcome::Complete(result) => return Ok(result),
            AgentStepOutcome::NeedTool { intent, .. } => {
                let outcome = executor.execute_tool(intent)?;
                last_result = Some(outcome.into_step_result());
            }
            AgentStepOutcome::AwaitingApproval { request_id, .. } => {
                let resolved = executor.resolve_approval(&request_id, true)?;
                last_result = Some(StepResult::Ok {
                    payload: resolved,
                });
            }
        }
    }
}

/// Run from a fresh session: route, build plan, return first action (or complete if no steps).
pub fn run_first_step(
    session_id: String,
    input: crate::semantic_router::RouterInput,
    default_workspace_id: Option<String>,
    default_workspace_root: Option<String>,
) -> Result<OrchestratorAction, String> {
    let (session, first_intent) = start_session(
        session_id,
        input,
        default_workspace_id,
        default_workspace_root,
    );

    if let Some(intent) = first_intent {
        return Ok(OrchestratorAction::ToolCall {
            intent,
            session,
        });
    }

    // Empty plan (e.g. tools_only with no heuristic steps)
    next_action(session, None)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::{
        start_session, do_expand_plan,
        ExpansionKind, SessionKind, SessionState, TaskState, ToolCallIntent, StepResult,
        OrchestratorAction, AgentStepOutcome, AgentConfig, AgentTaskInput,
        SUMMARIZER_SYSTEM_HINT, FS_RESULT_PLACEHOLDER, FILE_CONTENTS_PLACEHOLDER,
        MOST_RECENT_FILE_PLACEHOLDER,
    };
    use crate::semantic_router::{RouterConfig, RouterInput, TaskContext};

    #[test]
    fn start_session_creates_plan_with_llm_step() {
        let input = RouterInput {
            task_description: "What is Rust?".to_string(),
            context: TaskContext::default(),
            config: Default::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            None,
            None,
        );
        assert_eq!(session.state, TaskState::Executing);
        assert_eq!(session.plan.len(), 1);
        assert!(matches!(&session.plan[0], ToolCallIntent::LlmGenerate { .. }));
        assert!(first.is_some());
    }

    #[test]
    fn start_session_tools_only_list_files() {
        let input = RouterInput {
            task_description: "list files in workspace".to_string(),
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
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(session.state, TaskState::Executing);
        assert!(
            session.plan.iter().any(|step| {
                matches!(step, ToolCallIntent::FsListDir { rel_path, .. } if rel_path == ".")
            }),
            "tools-only list files plan should include FsListDir(\".\")"
        );
        assert!(first.is_some());
    }

    #[test]
    fn next_action_after_ok_completes() {
        let input = RouterInput {
            task_description: "Hello".to_string(),
            context: TaskContext::default(),
            config: Default::default(),
            capabilities: None,
        };
        let (mut session, _) = start_session("s1".to_string(), input, None, None);
        session.step_index = 1; // one step done
        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": "Rust is a systems language." }),
            }),
        )
        .unwrap();
        match action {
            OrchestratorAction::Complete { response, .. } => {
                assert!(response.contains("Rust"));
            }
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn agent_step_first_call_returns_need_tool() {
        let mut session = SessionState::new("s1".to_string(), "What is Rust?".to_string());
        let config = AgentConfig::default();
        let input = AgentTaskInput {
            task_description: "What is Rust?".to_string(),
            context: TaskContext::default(),
        };
        let outcome = agent_step(input, &mut session, &config, None).unwrap();
        match outcome {
            AgentStepOutcome::NeedTool { intent, .. } => {
                assert!(matches!(intent, ToolCallIntent::LlmGenerate { .. }));
            }
            _ => panic!("expected NeedTool"),
        }
    }

    #[test]
    fn agent_step_complete_builds_task_result() {
        let mut session = SessionState::new("s1".to_string(), "Hello".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![ToolCallIntent::LlmGenerate {
            strategy: crate::semantic_router::Strategy::CheapModel,
            task: "Hello".to_string(),
            system_hint: Some(SUMMARIZER_SYSTEM_HINT.to_string()),
        }];
        session.step_index = 0; // about to process first (and only) step result
        let config = AgentConfig::default();
        let input = AgentTaskInput {
            task_description: "Hello".to_string(),
            context: TaskContext::default(),
        };
        let outcome = agent_step(
            input,
            &mut session,
            &config,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": "Hi there." }),
            }),
        )
        .unwrap();
        match outcome {
            AgentStepOutcome::Complete(result) => {
                assert_eq!(result.final_answer.as_deref(), Some("Hi there."));
                assert_eq!(result.tool_traces.len(), 1);
                assert_eq!(result.tool_traces[0].tool_name, "llm_generate");
            }
            _ => panic!("expected Complete"),
        }
    }

    /// Tools-only: "delete file X" -> orchestrator emits single FsDelete intent.
    #[test]
    fn start_session_tools_only_delete_emits_fs_delete() {
        let input = RouterInput {
            task_description: "delete foo.txt".to_string(),
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
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(session.state, TaskState::Executing);
        assert_eq!(session.plan.len(), 1);
        match &session.plan[0] {
            ToolCallIntent::FsDelete { rel_path, workspace_id, .. } => {
                assert_eq!(rel_path, "foo.txt");
                assert_eq!(workspace_id, "ws1");
            }
            _ => panic!("expected FsDelete intent"),
        }
        assert!(first.is_some());
    }

    /// Implicit FS request "summarize my desktop folder" -> two-step plan:
    /// [FsListDir(Desktop), LlmGenerate(with {{FS_RESULT}})]
    #[test]
    fn start_session_implicit_fs_produces_two_step_plan() {
        let input = RouterInput {
            task_description: "Please summarize my desktop folder".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(session.plan.len(), 2, "should have FsListDir + LlmGenerate");
        assert!(
            matches!(&session.plan[0], ToolCallIntent::FsListDir { .. }),
            "step 0 should be FsListDir"
        );
        assert!(
            matches!(&session.plan[1], ToolCallIntent::LlmGenerate { .. }),
            "step 1 should be LlmGenerate"
        );
        // LlmGenerate task must contain the placeholder (not yet substituted)
        if let ToolCallIntent::LlmGenerate { task, .. } = &session.plan[1] {
            assert!(
                task.contains(FS_RESULT_PLACEHOLDER),
                "LlmGenerate task must contain placeholder before substitution"
            );
        }
        // First emitted intent is FsListDir
        assert!(matches!(first, Some(ToolCallIntent::FsListDir { .. })));
    }

    /// After FsListDir returns real entries, next_action substitutes {{FS_RESULT}}
    /// in the LlmGenerate task before emitting it.
    #[test]
    fn next_action_substitutes_fs_result_placeholder() {
        let fs_step = ToolCallIntent::FsListDir {
            workspace_id: "ws1".to_string(),
            workspace_root: "/Users/test/Desktop".to_string(),
            rel_path: ".".to_string(),
        };
        let llm_task_template = format!(
            "The user asked: \"summarize my desktop\"\n\nTool result:\n{}\n\nSummarise.",
            FS_RESULT_PLACEHOLDER
        );
        let llm_step = ToolCallIntent::LlmGenerate {
            strategy: crate::semantic_router::Strategy::CheapModel,
            task: llm_task_template,
            system_hint: None,
        };

        let mut session = SessionState::new("s1".to_string(), "summarize my desktop".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![fs_step, llm_step];
        session.step_index = 0;

        // Simulate FsListDir returning real entries
        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": "file1.txt\nphoto.png\nREADME.md" }),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::ToolCall { intent, .. } => match intent {
                ToolCallIntent::LlmGenerate { task, .. } => {
                    assert!(
                        !task.contains(FS_RESULT_PLACEHOLDER),
                        "placeholder must be substituted before emitting"
                    );
                    assert!(
                        task.contains("file1.txt"),
                        "real fs result must appear in the task"
                    );
                    assert!(task.contains("photo.png"));
                    assert!(task.contains("README.md"));
                }
                _ => panic!("expected LlmGenerate intent"),
            },
            _ => panic!("expected ToolCall, got Complete or AwaitingApproval"),
        }
    }

    /// "Summarize /tmp/paper.md" → plan must be [FsReadFile, LlmGenerate] (explicit path detected).
    #[test]
    fn start_session_file_read_with_explicit_path_builds_read_plan() {
        let input = RouterInput {
            task_description: "Summarize /tmp/paper.md".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp".to_string()),
        );
        assert_eq!(session.plan.len(), 2, "should have FsReadFile + LlmGenerate");
        assert!(
            matches!(&session.plan[0], ToolCallIntent::FsReadFile { .. }),
            "step 0 should be FsReadFile, got {:?}",
            &session.plan[0]
        );
        assert!(
            matches!(&session.plan[1], ToolCallIntent::LlmGenerate { .. }),
            "step 1 should be LlmGenerate"
        );
        if let ToolCallIntent::LlmGenerate { task, .. } = &session.plan[1] {
            assert!(
                task.contains(FS_RESULT_PLACEHOLDER),
                "LlmGenerate task must contain FS_RESULT placeholder before substitution"
            );
        }
        assert!(matches!(first, Some(ToolCallIntent::FsReadFile { .. })));
    }

    /// "Summarize the file I just saved in Downloads" → 3-step plan:
    /// [FsListDir, FsReadFile({{MOST_RECENT_FILE}}), LlmGenerate]
    #[test]
    fn start_session_most_recent_file_builds_three_step_plan() {
        let input = RouterInput {
            task_description: "Summarize the file I just saved in my Downloads folder".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(session.plan.len(), 3, "should have FsListDir + FsReadFile + LlmGenerate");
        assert!(
            matches!(&session.plan[0], ToolCallIntent::FsListDir { .. }),
            "step 0 should be FsListDir"
        );
        assert!(
            matches!(&session.plan[1], ToolCallIntent::FsReadFile { rel_path, .. }
                if rel_path == MOST_RECENT_FILE_PLACEHOLDER),
            "step 1 rel_path must be the MOST_RECENT_FILE_PLACEHOLDER"
        );
        assert!(
            matches!(&session.plan[2], ToolCallIntent::LlmGenerate { .. }),
            "step 2 should be LlmGenerate"
        );
        assert!(matches!(first, Some(ToolCallIntent::FsListDir { .. })));
    }

    /// After FsListDir returns sortedByModified, next_action resolves
    /// {{MOST_RECENT_FILE}} in the following FsReadFile step.
    #[test]
    fn next_action_resolves_most_recent_file_placeholder() {
        let mut session =
            SessionState::new("s1".to_string(), "summarize most recent file".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![
            ToolCallIntent::FsListDir {
                workspace_id: "ws1".to_string(),
                workspace_root: "/Users/test/Downloads".to_string(),
                rel_path: ".".to_string(),
            },
            ToolCallIntent::FsReadFile {
                workspace_id: "ws1".to_string(),
                workspace_root: "/Users/test/Downloads".to_string(),
                rel_path: MOST_RECENT_FILE_PLACEHOLDER.to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: format!("Summarize: {}", FS_RESULT_PLACEHOLDER),
                system_hint: None,
            },
        ];
        session.step_index = 0;

        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({
                    "entries": ["old_doc.pdf", "new_report.pdf"],
                    "sortedByModified": ["new_report.pdf", "old_doc.pdf"],
                    "text": "new_report.pdf\nold_doc.pdf"
                }),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::ToolCall { intent, session: s } => {
                match intent {
                    ToolCallIntent::FsReadFile { rel_path, .. } => {
                        assert_eq!(
                            rel_path, "new_report.pdf",
                            "most recently modified file must be picked"
                        );
                    }
                    _ => panic!("expected FsReadFile intent"),
                }
                assert_eq!(
                    s.last_dir_listing_sorted.first().map(String::as_str),
                    Some("new_report.pdf"),
                    "session.last_dir_listing_sorted must hold the sorted list"
                );
                assert_eq!(
                    s.confirmed_root.as_deref(),
                    Some("/Users/test/Downloads"),
                    "confirmed_root must be recorded from FsListDir"
                );
            }
            _ => panic!("expected ToolCall"),
        }
    }

    /// "Summarize the paper on climate change" (no explicit path) → plan starts with FsListDir.
    #[test]
    fn start_session_file_read_without_path_falls_back_to_list() {
        let input = RouterInput {
            task_description: "Summarize the paper on climate change".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert!(
            session.plan.len() >= 1,
            "plan must have at least one step"
        );
        assert!(
            matches!(&session.plan[0], ToolCallIntent::FsListDir { .. }),
            "step 0 should be FsListDir when no explicit path, got {:?}",
            &session.plan[0]
        );
        assert!(matches!(first, Some(ToolCallIntent::FsListDir { .. })));
    }

    /// When FsReadFile returns Err, the orchestrator must return "Error: ..." and NOT
    /// proceed to the LlmGenerate step — preventing a fabricated file summary.
    #[test]
    fn fs_read_file_error_returns_error_not_hallucination() {
        let mut session = SessionState::new("s1".to_string(), "Summarize paper.md".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![
            ToolCallIntent::FsReadFile {
                workspace_id: "ws1".to_string(),
                workspace_root: "/tmp".to_string(),
                rel_path: "paper.md".to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: format!("Summarize: {}", FS_RESULT_PLACEHOLDER),
                system_hint: Some(SUMMARIZER_SYSTEM_HINT.to_string()),
            },
        ];
        session.step_index = 0;

        let action = next_action(
            session,
            Some(StepResult::Err {
                message: "No such file or directory: paper.md".to_string(),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::Complete { response, .. } => {
                assert!(
                    response.starts_with("Error:"),
                    "error result must begin with 'Error:' not a fabricated summary: {:?}",
                    response
                );
            }
            _ => panic!("expected Complete with error response"),
        }
    }

    /// State machine: tools-only delete — first step returns NeedTool(FsDelete), then Ok result -> Complete.
    #[test]
    fn orchestrator_tools_only_delete_state_machine() {
        let mut session = SessionState::new("s1".to_string(), "delete bar.txt".to_string());
        let config = AgentConfig {
            default_workspace_id: Some("w1".to_string()),
            default_workspace_root: Some("/tmp/proj".to_string()),
            router_config: RouterConfig {
                prefer_tools_only: true,
                ..Default::default()
            },
            ..AgentConfig::default()
        };
        let input = AgentTaskInput {
            task_description: "delete bar.txt".to_string(),
            context: TaskContext::default(),
        };

        let out1 = agent_step(input.clone(), &mut session, &config, None).unwrap();
        let intent = match &out1 {
            AgentStepOutcome::NeedTool { intent, .. } => intent.clone(),
            _ => panic!("expected NeedTool, got {:?}", out1),
        };
        match &intent {
            ToolCallIntent::FsDelete { rel_path, .. } => assert_eq!(rel_path, "bar.txt"),
            _ => panic!("expected FsDelete"),
        }

        let out2 = agent_step(
            input,
            &mut session,
            &config,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "deleted": true }),
            }),
        )
        .unwrap();
        match out2 {
            AgentStepOutcome::Complete(result) => {
                assert_eq!(result.tool_traces.len(), 1);
                assert_eq!(result.tool_traces[0].tool_name, "fs_delete");
            }
            _ => panic!("expected Complete"),
        }
    }

    // ─── Demo workflow tests ─────────────────────────────────────────────────

    /// Workflow 1: "Summarize Test1_doc.md in Downloads" →
    /// plan = [FsReadFile(Downloads/Test1_doc.md), LlmGenerate] (no listing needed)
    #[test]
    fn start_session_file_in_known_dir_uses_dir_as_root() {
        let input = RouterInput {
            task_description: "Summarize Test1_doc.md in my Downloads folder".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(
            session.plan.len(),
            2,
            "expected [FsReadFile, LlmGenerate], got {:?}",
            session.plan
        );
        match &session.plan[0] {
            ToolCallIntent::FsReadFile { workspace_root, rel_path, .. } => {
                assert!(
                    workspace_root.contains("Downloads"),
                    "workspace_root should be the Downloads directory, got {:?}",
                    workspace_root
                );
                assert_eq!(rel_path, "Test1_doc.md", "rel_path must be the bare filename");
            }
            other => panic!("step 0 should be FsReadFile, got {:?}", other),
        }
        assert!(matches!(&session.plan[1], ToolCallIntent::LlmGenerate { .. }));
        assert!(matches!(first, Some(ToolCallIntent::FsReadFile { .. })));
        assert!(session.pending_expansion.is_none(), "no expansion for single-file workflow");
    }

    /// Workflow 2: "Summarize the 20 Test2_doc reports in Downloads into one overview" →
    /// sentinel plan = [FsListDir, LlmGenerate({{FILE_CONTENTS}})]
    /// + pending_expansion = ReadAndSummarize { file_filter: Some("Test2_doc") }
    ///
    /// IGNORED for v0.1: Workflow 2 is disabled in start_session pending
    /// end-to-end context-budget validation.  Run with `--include-ignored`
    /// after re-enabling the match arm in planning.rs.  See ROADMAP.md.
    #[test]
    #[ignore = "Workflow 2 disabled for v0.1 — re-enable match arm in planning.rs first"]
    fn start_session_multi_file_summarize_builds_sentinel() {
        let input = RouterInput {
            task_description:
                "Summarize the 20 Test2_doc reports in Downloads into one overview".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(
            session.plan.len(),
            2,
            "sentinel plan should have [FsListDir, LlmGenerate], got {:?}",
            session.plan
        );
        assert!(matches!(&session.plan[0], ToolCallIntent::FsListDir { .. }), "step 0 = FsListDir");
        match &session.plan[1] {
            ToolCallIntent::LlmGenerate { task, .. } => {
                assert!(
                    task.contains(FILE_CONTENTS_PLACEHOLDER),
                    "LlmGenerate task must contain FILE_CONTENTS_PLACEHOLDER"
                );
            }
            other => panic!("step 1 should be LlmGenerate, got {:?}", other),
        }
        assert!(matches!(first, Some(ToolCallIntent::FsListDir { .. })));
        match &session.pending_expansion {
            Some(ExpansionKind::ReadAndSummarize { file_filter }) => {
                assert!(
                    file_filter.as_deref() == Some("Test2_doc"),
                    "file_filter should be 'Test2_doc', got {:?}",
                    file_filter
                );
            }
            other => panic!("expected ReadAndSummarize expansion, got {:?}", other),
        }
    }

    /// Workflow 3: move task → sentinel plan = [FsListDir, LlmGenerate]
    /// + pending_expansion = MoveToDir { dest_rel_dir: "Test_folder", ... }
    ///
    /// IGNORED for v0.1: Workflow 3 is disabled in start_session pending
    /// end-to-end validation with real files.  Run with `--include-ignored`
    /// after re-enabling the match arm in planning.rs.  See ROADMAP.md.
    #[test]
    #[ignore = "Workflow 3 disabled for v0.1 — re-enable match arm in planning.rs first"]
    fn start_session_move_to_dir_builds_sentinel() {
        let input = RouterInput {
            task_description:
                "Move those 20 Test2_doc files from my Downloads folder into a new folder called Test_folder on my Desktop"
                    .to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let (session, first) = start_session(
            "s1".to_string(),
            input,
            Some("ws1".to_string()),
            Some("/tmp/ws".to_string()),
        );
        assert_eq!(
            session.plan.len(),
            2,
            "sentinel plan should have [FsListDir, LlmGenerate], got {:?}",
            session.plan
        );
        assert!(matches!(&session.plan[0], ToolCallIntent::FsListDir { workspace_root, .. }
            if workspace_root.contains("Downloads")),
            "FsListDir must target Downloads"
        );
        assert!(matches!(first, Some(ToolCallIntent::FsListDir { .. })));
        match &session.pending_expansion {
            Some(ExpansionKind::MoveToDir { dest_rel_dir, dest_workspace_root, .. }) => {
                assert_eq!(dest_rel_dir, "Test_folder", "dest folder name preserved");
                assert!(
                    dest_workspace_root.contains("Desktop"),
                    "destination root must be Desktop, got {:?}",
                    dest_workspace_root
                );
            }
            other => panic!("expected MoveToDir expansion, got {:?}", other),
        }
    }

    /// `do_expand_plan` with ReadAndSummarize: inserts N×FsReadFile before the sentinel LlmGenerate.
    #[test]
    fn do_expand_plan_read_and_summarize_inserts_reads() {
        let ws_root = "/Users/test/Downloads".to_string();
        let llm_task = format!("Overview: {}", FILE_CONTENTS_PLACEHOLDER);

        let mut session = SessionState::new("s1".to_string(), "summarize reports".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![
            ToolCallIntent::FsListDir {
                workspace_id: "ws1".to_string(),
                workspace_root: ws_root.clone(),
                rel_path: ".".to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: llm_task,
                system_hint: None,
            },
        ];
        session.step_index = 1;
        session.confirmed_root = Some(ws_root.clone());
        session.last_dir_listing_sorted =
            vec!["a.md".to_string(), "b.txt".to_string(), ".hidden".to_string(), "image.png".to_string()];

        let expansion = ExpansionKind::ReadAndSummarize { file_filter: None };
        do_expand_plan(&mut session, expansion);

        // Readable files: a.md and b.txt (not .hidden or image.png).
        // Expected plan: [FsListDir, FsReadFile(a.md), FsReadFile(b.txt), LlmGenerate]
        assert_eq!(
            session.plan.len(),
            4,
            "plan should be [FsListDir, FsReadFile×2, LlmGenerate], got {:?}",
            session.plan
        );
        assert!(matches!(&session.plan[0], ToolCallIntent::FsListDir { .. }));
        match &session.plan[1] {
            ToolCallIntent::FsReadFile { rel_path, workspace_root: wr, .. } => {
                assert_eq!(rel_path, "a.md");
                assert_eq!(wr, &ws_root);
            }
            other => panic!("plan[1] should be FsReadFile(a.md), got {:?}", other),
        }
        match &session.plan[2] {
            ToolCallIntent::FsReadFile { rel_path, .. } => assert_eq!(rel_path, "b.txt"),
            other => panic!("plan[2] should be FsReadFile(b.txt), got {:?}", other),
        }
        assert!(matches!(&session.plan[3], ToolCallIntent::LlmGenerate { .. }));
    }

    /// `do_expand_plan` with MoveToDir: inserts [FsCreateDir, FsMove×N] before sentinel LlmGenerate.
    #[test]
    fn do_expand_plan_move_to_dir_inserts_create_and_moves() {
        let src_root = "/Users/test/Downloads".to_string();
        let dest_root = "/Users/test/Desktop".to_string();

        let mut session = SessionState::new("s1".to_string(), "move files".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![
            ToolCallIntent::FsListDir {
                workspace_id: "ws1".to_string(),
                workspace_root: src_root.clone(),
                rel_path: ".".to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: "Confirm move".to_string(),
                system_hint: None,
            },
        ];
        session.step_index = 1;
        session.confirmed_root = Some(src_root.clone());
        session.last_dir_listing_sorted =
            vec!["file1.md".to_string(), "file2.md".to_string(), "other.png".to_string()];

        let expansion = ExpansionKind::MoveToDir {
            dest_workspace_id: "ws1".to_string(),
            dest_workspace_root: dest_root.clone(),
            dest_rel_dir: "MyFolder".to_string(),
            file_filter: Some("file".to_string()),
        };
        do_expand_plan(&mut session, expansion);

        // file_filter "file" matches file1.md and file2.md (not other.png).
        assert_eq!(
            session.plan.len(),
            5,
            "expected [FsListDir, FsCreateDir, FsMove×2, LlmGenerate], got {:?}",
            session.plan
        );
        assert!(matches!(&session.plan[0], ToolCallIntent::FsListDir { .. }));
        match &session.plan[1] {
            ToolCallIntent::FsCreateDir { workspace_root: wr, rel_path, .. } => {
                assert_eq!(wr, &dest_root);
                assert_eq!(rel_path, "MyFolder");
            }
            other => panic!("plan[1] should be FsCreateDir, got {:?}", other),
        }
        match &session.plan[2] {
            ToolCallIntent::FsMove { workspace_root: sr, rel_path, dest_workspace_root: dr, dest_rel_path, .. } => {
                assert_eq!(sr, &src_root);
                assert_eq!(rel_path, "file1.md");
                assert_eq!(dr, &dest_root);
                assert_eq!(dest_rel_path, "MyFolder/file1.md");
            }
            other => panic!("plan[2] should be FsMove(file1.md), got {:?}", other),
        }
        match &session.plan[3] {
            ToolCallIntent::FsMove { rel_path, dest_rel_path, .. } => {
                assert_eq!(rel_path, "file2.md");
                assert_eq!(dest_rel_path, "MyFolder/file2.md");
            }
            other => panic!("plan[3] should be FsMove(file2.md), got {:?}", other),
        }
        assert!(matches!(&session.plan[4], ToolCallIntent::LlmGenerate { .. }));
    }

    /// After two FsReadFile results, `content_accumulator` has 2 entries and the final
    /// LlmGenerate gets `{{FILE_CONTENTS}}` substituted before being emitted.
    #[test]
    fn next_action_accumulates_reads_and_substitutes_file_contents() {
        let ws_root = "/Users/test/Downloads".to_string();

        let mut session = SessionState::new("s1".to_string(), "summarize files".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![
            ToolCallIntent::FsListDir {
                workspace_id: "ws1".to_string(),
                workspace_root: ws_root.clone(),
                rel_path: ".".to_string(),
            },
            ToolCallIntent::FsReadFile {
                workspace_id: "ws1".to_string(),
                workspace_root: ws_root.clone(),
                rel_path: "a.md".to_string(),
            },
            ToolCallIntent::FsReadFile {
                workspace_id: "ws1".to_string(),
                workspace_root: ws_root.clone(),
                rel_path: "b.md".to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: format!("Overview:\n{}", FILE_CONTENTS_PLACEHOLDER),
                system_hint: None,
            },
        ];
        session.step_index = 1; // FsListDir already done; next is FsReadFile(a.md)

        // Simulate FsReadFile(a.md) result
        let action1 = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": "Content of file A" }),
            }),
        )
        .unwrap();

        let session2 = match action1 {
            OrchestratorAction::ToolCall { session: s, .. } => s,
            other => panic!("expected ToolCall after first read, got {:?}", other),
        };
        assert_eq!(session2.content_accumulator.len(), 1, "one file read so far");
        assert_eq!(session2.content_accumulator[0], "Content of file A");

        // Simulate FsReadFile(b.md) result
        let action2 = next_action(
            session2,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": "Content of file B" }),
            }),
        )
        .unwrap();

        match action2 {
            OrchestratorAction::ToolCall { intent, session: s } => {
                assert_eq!(s.content_accumulator.len(), 2, "two files read");
                match intent {
                    ToolCallIntent::LlmGenerate { task, .. } => {
                        assert!(
                            !task.contains(FILE_CONTENTS_PLACEHOLDER),
                            "placeholder must be substituted"
                        );
                        assert!(task.contains("Content of file A"), "file A content must appear");
                        assert!(task.contains("Content of file B"), "file B content must appear");
                        assert!(task.contains("---"), "separator between files must appear");
                    }
                    other => panic!("expected LlmGenerate, got {:?}", other),
                }
            }
            other => panic!("expected ToolCall with LlmGenerate, got {:?}", other),
        }
    }

    // ─── check_llm_generate_precondition tests ───────────────────────────────

    /// Guard A fires: LlmGenerate with {{FILE_CONTENTS}} is blocked when
    /// content_accumulator is empty (no FsReadFile has succeeded yet).
    #[test]
    fn llm_generate_guard_blocks_when_no_reads_completed() {
        let mut session = SessionState::new("s1".to_string(), "summarize files".to_string());
        session.state = TaskState::Executing;
        session.kind = SessionKind::Task;
        session.plan = vec![
            ToolCallIntent::FsListDir {
                workspace_id: "ws1".to_string(),
                workspace_root: "/tmp".to_string(),
                rel_path: ".".to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: format!("Overview: {}", FILE_CONTENTS_PLACEHOLDER),
                system_hint: None,
            },
        ];
        session.step_index = 0;

        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({
                    "entries": [],
                    "sortedByModified": [],
                    "text": ""
                }),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::Complete { response, .. } => {
                assert!(
                    response.to_lowercase().contains("no file contents")
                        || response.to_lowercase().contains("cannot generate summary"),
                    "guard A must produce a clear error, got: {:?}",
                    response
                );
            }
            OrchestratorAction::ToolCall { intent, .. } => {
                panic!(
                    "guard A should have blocked LlmGenerate, but got ToolCall({:?})",
                    intent
                );
            }
            _ => panic!("expected Complete with guard error"),
        }
    }

    /// Guard A passes: LlmGenerate with {{FILE_CONTENTS}} is emitted normally
    /// when content_accumulator has at least one entry.
    #[test]
    fn llm_generate_guard_passes_when_reads_completed() {
        let mut session = SessionState::new("s1".to_string(), "summarize files".to_string());
        session.state = TaskState::Executing;
        session.plan = vec![
            ToolCallIntent::FsReadFile {
                workspace_id: "ws1".to_string(),
                workspace_root: "/tmp".to_string(),
                rel_path: "a.md".to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: format!("Overview: {}", FILE_CONTENTS_PLACEHOLDER),
                system_hint: None,
            },
        ];
        session.step_index = 0;

        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": "Real file content." }),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::ToolCall { intent, .. } => {
                match intent {
                    ToolCallIntent::LlmGenerate { task, .. } => {
                        assert!(
                            !task.contains(FILE_CONTENTS_PLACEHOLDER),
                            "placeholder must be substituted"
                        );
                        assert!(
                            task.contains("Real file content."),
                            "real content must appear in the task"
                        );
                    }
                    other => panic!("expected LlmGenerate, got {:?}", other),
                }
            }
            other => panic!("guard should not have fired; expected ToolCall, got {:?}", other),
        }
    }

    /// Guard B fires: LlmGenerate with {{FS_RESULT}} is blocked when the preceding
    /// filesystem tool returned only a JSON blob (no "text" field).
    #[test]
    fn llm_generate_guard_blocks_when_fs_result_is_json_blob() {
        let mut session = SessionState::new("s1".to_string(), "list folder".to_string());
        session.state = TaskState::Executing;
        session.kind = SessionKind::Task;
        session.plan = vec![
            ToolCallIntent::FsListDir {
                workspace_id: "ws1".to_string(),
                workspace_root: "/tmp".to_string(),
                rel_path: ".".to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: format!("Summarize: {}", FS_RESULT_PLACEHOLDER),
                system_hint: None,
            },
        ];
        session.step_index = 0;

        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "entries": [] }),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::Complete { response, .. } => {
                assert!(
                    response.to_lowercase().contains("cannot generate")
                        || response.to_lowercase().contains("readable text"),
                    "guard B must produce a clear error, got: {:?}",
                    response
                );
            }
            OrchestratorAction::ToolCall { intent, .. } => {
                panic!(
                    "guard B should have blocked LlmGenerate, but got ToolCall({:?})",
                    intent
                );
            }
            _ => panic!("expected Complete with guard error"),
        }
    }

    // ── sanitize_llm_generate_output ─────────────────────────────────────────

    /// Clean prose passes without modification.
    #[test]
    fn sanitize_output_passes_clean_prose() {
        let text = "The repository contains twenty design reports covering the \
                    orchestrator, the FSM, the approval gate, and the UniFFI boundary.";
        let result = sanitize_llm_generate_output(text);
        assert!(result.is_ok(), "clean prose should pass, got: {:?}", result);
        assert_eq!(result.unwrap(), text);
    }

    /// Any of the forbidden tool name substrings triggers rejection.
    #[test]
    fn sanitize_output_rejects_tool_name() {
        for forbidden in &["fs_list_dir", "fs_read_file", "fs_write_file"] {
            let text = format!("I used {} to list the folder contents.", forbidden);
            let result = sanitize_llm_generate_output(&text);
            assert!(
                result.is_err(),
                "expected rejection for pattern {:?}, but got Ok",
                forbidden
            );
            assert!(
                result
                    .unwrap_err()
                    .message
                    .contains("describe internal tool calls"),
                "error message should describe the problem"
            );
        }
    }

    // ── FS_RESULT content-budget cap ─────────────────────────────────────────

    /// When file content exceeds FS_RESULT_MAX_CHARS, the injected text is truncated
    /// and the truncation notice is appended.  The literal placeholder must not appear.
    #[test]
    fn next_action_fs_result_truncated_at_limit() {
        let mut session = SessionState::new("s1".to_string(), "summarize big.txt".to_string());
        session.state = TaskState::Executing;
        session.kind = SessionKind::Task;
        session.plan = vec![
            ToolCallIntent::FsReadFile {
                workspace_id: "ws1".to_string(),
                workspace_root: "/tmp".to_string(),
                rel_path: "big.txt".to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: format!("Summarize: {}", FS_RESULT_PLACEHOLDER),
                system_hint: None,
            },
        ];
        session.step_index = 0;

        // Produce a fake file content that is clearly over the 4 000-char limit.
        let big_content = "x".repeat(FS_RESULT_MAX_CHARS + 500);

        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "text": big_content }),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::ToolCall { intent, .. } => match intent {
                ToolCallIntent::LlmGenerate { task, .. } => {
                    assert!(
                        !task.contains(FS_RESULT_PLACEHOLDER),
                        "placeholder must be substituted"
                    );
                    assert!(
                        task.contains(FS_RESULT_TRUNCATION_NOTICE),
                        "truncation notice must appear in the task"
                    );
                    // Injected content should be ≤ FS_RESULT_MAX_CHARS chars of the original
                    // plus the notice — not the full big_content.
                    assert!(
                        task.len() < big_content.len(),
                        "task should be shorter than the original content"
                    );
                }
                other => panic!("expected LlmGenerate, got {:?}", other),
            },
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    // ── MOST_RECENT_FILE empty-listing guard ─────────────────────────────────

    /// When last_dir_listing_sorted is empty and the plan contains a FsReadFile
    /// with MOST_RECENT_FILE_PLACEHOLDER, next_action must return Complete with a
    /// user-visible error instead of forwarding a bogus path to the executor.
    #[test]
    fn next_action_most_recent_file_empty_listing_returns_error() {
        let mut session = SessionState::new("s1".to_string(), "summarize most recent".to_string());
        session.state = TaskState::Executing;
        session.kind = SessionKind::Task;
        session.plan = vec![
            ToolCallIntent::FsListDir {
                workspace_id: "ws1".to_string(),
                workspace_root: "/tmp/downloads".to_string(),
                rel_path: ".".to_string(),
            },
            ToolCallIntent::FsReadFile {
                workspace_id: "ws1".to_string(),
                workspace_root: "/tmp/downloads".to_string(),
                rel_path: MOST_RECENT_FILE_PLACEHOLDER.to_string(),
            },
            ToolCallIntent::LlmGenerate {
                strategy: crate::semantic_router::Strategy::CheapModel,
                task: format!("Summarize: {}", FS_RESULT_PLACEHOLDER),
                system_hint: None,
            },
        ];
        session.step_index = 0;

        // Simulate FsListDir returning an empty directory (no sortedByModified entries).
        let action = next_action(
            session,
            Some(StepResult::Ok {
                payload: serde_json::json!({ "entries": [], "sortedByModified": [] }),
            }),
        )
        .unwrap();

        match action {
            OrchestratorAction::Complete { response, .. } => {
                assert!(
                    response.to_lowercase().contains("no recent files"),
                    "error must mention 'no recent files', got: {:?}",
                    response
                );
            }
            OrchestratorAction::ToolCall { intent, .. } => {
                panic!(
                    "guard should have fired; got ToolCall({:?}) instead of Complete",
                    intent
                );
            }
            _ => panic!("expected Complete with guard error"),
        }
    }

    /// Narration phrases are rejected regardless of capitalisation.
    #[test]
    fn sanitize_output_rejects_narration_phrase() {
        let cases = [
            "I'll use the tool to read the file.",
            "I WILL USE fs_read_file next.",
            "I will now call the filesystem handler.",
            "First, I'll list the directory.",
            "Next, I'll read each document.",
            "Now, I'll summarise the results.",
        ];
        for text in cases {
            let result = sanitize_llm_generate_output(text);
            assert!(
                result.is_err(),
                "expected rejection for {:?}, but got Ok",
                text
            );
        }
    }
}
