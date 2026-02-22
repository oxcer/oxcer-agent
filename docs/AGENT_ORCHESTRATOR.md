# Agent Orchestrator & Semantic Router (Sprint 6)

## Goal

Cost-aware, security-first Agent Orchestrator on top of the existing tools-only skeleton. A **Semantic Router** decides between cheap model, expensive model, or tools-only; all tool execution goes through the Security Policy Engine and approval flow.

## Three layers

### 1. Semantic Router v1 (`oxcer-core/src/semantic_router.rs`)

- **Public API:**  
  `route_task(task_description: &str, context: &TaskContext, config: &RouterConfig) -> RouterDecision`
- **TaskContext:** `workspace_id: Option<String>`, `selected_paths: Vec<String>`, `risk_hints: bool` (e.g. user said "delete", "rm -rf", "rename", "move", "chmod").
- **RouterDecision:** `category: TaskCategory`, `strategy: Strategy`, `flags: RouterFlags`.
- **TaskCategory:** `SimpleQa` | `Code` | `Planning` | `ToolsHeavy`.
- **Strategy:** `CheapModel` | `ExpensiveModel` | `ToolsOnly`.
- **RouterFlags:** `requires_high_risk_approval: bool`, `allow_model_tools_mix: bool`.
- **First pass (deterministic heuristics, no LLM):**
  - Short task, no file/tool verbs -> `SimpleQa` + `CheapModel`.
  - Code markers (`fn`, `class`, `import`, file extensions) -> `Code`; length then picks `CheapModel` vs `ExpensiveModel`.
  - "plan", "steps", "strategy" -> `Planning` + `ExpensiveModel`.
  - Explicit tool verbs ("list files", "open file", "delete", "rename", "move", "shell command", "run script") -> `ToolsHeavy` and `flags.requires_high_risk_approval = true`.
- **Second pass (optional, part of v1 API):**  
  `route_task_with_classifier(task, context, config, classifier)` — for borderline cases (default heuristic fallback), the given closure is invoked so a small LLM can return a JSON classification `{ "category": "...", "strategy": "..." }` with a small token budget. The orchestrator consumes `RouterDecision`.

### 2. Agent Orchestrator (`oxcer-core/src/orchestrator.rs`)

#### 2.1 Orchestrator API

- **AgentTaskInput:** `task_description: String`, `context: TaskContext` (workspace id, selected files, etc.).
- **AgentTaskResult:** `final_answer: Option<String>` (human-readable answer), `tool_traces: Vec<ToolTrace>` (for logging / UI).
- **ToolTrace:** `tool_name`, `input` (JSON), `policy_decision` (Allow / Deny / RequireApproval), `approved`, `result_summary`.
- **AgentSessionState:** tracks list of steps executed (`tool_traces`), approvals requested (`approvals_requested`), intermediate observations (`intermediate_observations`), plus plan, step index, router decision, accumulated response.
- **agent_step(input, session, config, last_result?)** -> **AgentStepOutcome:** `Complete(AgentTaskResult)` | `NeedTool { intent, session }` | `AwaitingApproval { request_id, session }`. Sync; use in a loop for frontend-driven execution.
- **agent_request(input, session, config, executor)** -> **AgentTaskResult**. Sync; runs to completion using an `AgentToolExecutor` (execute_tool, resolve_approval). For frontend-driven execution without a blocking executor, use `agent_step` in a loop instead.

#### 2.2 Execution strategies (inside agent_step / start_session)

1. Call **route_task(task_description, &context, &router_config)** -> **RouterDecision**.
2. **Strategy::ToolsOnly:** No LLM. Deterministic planner:
   - “list files (in workspace)” / “list dir” / “ls” -> single **FsListDir** (rel_path `"."`).
   - “delete X” / “remove X” / “rm X” -> single **FsDelete**; always goes through Security Policy Engine and existing approval flow (safest baseline when high-risk verbs are detected).
   - Requires `default_workspace_id` and `default_workspace_root` in `AgentConfig` to build intents.
3. **Strategy::CheapModel:** Low-cost model for simple QA and lightweight code edits. Plan = single **LlmGenerate** step; build prompt (system + user + minimal context), call model once (no tools), return answer; log decision and append to tool_traces.
4. **Strategy::ExpensiveModel:** For planning and complex code: plan = single **LlmGenerate** step (two-phase pattern can be added: planning call with JSON plan, then deterministic execution of tool steps; see code TODOs).
5. At each stage, **AgentSessionState** is updated and tool_traces / approvals_requested / intermediate_observations are appended for the in-memory log that mirrors what can be persisted.

### 3. Execution / Policy Layer (existing)

- Every tool call produced by the orchestrator is executed by the **frontend** (Swift or Web) by invoking the same Tauri commands used by the UI: `cmd_fs_list_dir`, `cmd_fs_read_file`, `cmd_fs_write_file`, `cmd_fs_delete`, `cmd_fs_rename`, `cmd_fs_move`, `cmd_shell_run`, with **`caller: "agent_orchestrator"`**.
- Those commands go through: **Command Router -> Security Policy Engine -> optional HITL Approval -> tool.** So all agent tool executions are policy-checked and approval-gated the same way as user actions.

## Backend API: `cmd_agent_step`

- **Command:** `cmd_agent_step(session_id, task, input: RouterInput, last_result?: StepResult)` -> `OrchestratorAction`.
- **First call:** `last_result` omitted. Backend runs router + builds plan, stores session, returns `ToolCall(intent)` or `Complete` or `AwaitingApproval`.
- **Subsequent calls:** Frontend passes `last_result` (from executing the previous intent via `cmd_fs_*` / `cmd_shell_run` or from `cmd_approve_and_execute`). Backend advances the session and returns the next action.
- **Session store:** In-memory `AgentSessionStore` keyed by `session_id`; session is saved when returning `ToolCall` or `AwaitingApproval`, so the frontend can resume after executing the tool or after user approval.

## Frontend flow (Swift / Web)

1. Call `cmd_agent_step(session_id, task, input, null)`.
2. If response is **`Complete`** -> show `response` and stop.
3. If response is **`ToolCall(intent)`**:
   - Map `intent` to the right command (e.g. `FsReadFile` -> `cmd_fs_read_file(workspace_root, rel_path, caller: "agent_orchestrator")`).
   - If command returns **Ok(payload)** -> call `cmd_agent_step(session_id, task, input, StepResult::Ok { payload })`.
   - If command returns **Err(ApprovalRequired { request_id })** -> show approval modal; on approve call `cmd_approve_and_execute(request_id, true)`, then call `cmd_agent_step(session_id, task, input, StepResult::Ok { payload })` with the execution result.
4. If response is **`AwaitingApproval { request_id }`** -> same as above: show modal, then approve and resume with the result.

## Model backends (Sprint 6)

- **Remote APIs only:** OpenAI, Gemini, Anthropic, Grok (no local LLM in Sprint 6).
- **Strategy -> model:** `cheap_model` / `expensive_model` produce a `LlmGenerate` intent; the frontend (or a future backend step) chooses the concrete provider/model from settings (e.g. `model.default_id`) and calls the corresponding API. The orchestrator does not call the LLM itself in the current wiring; it only emits `LlmGenerate` as a tool intent so the runner can integrate LLM calls in a later step.

## Agent as untrusted client

- All tool calls use **caller = PolicyCaller::AgentOrchestrator** through the Command Router. The Security Policy Engine applies a more conservative rule set for the agent (path blocklist, no direct HOME credential access, write/exec require approval).
- **No batching:** The orchestrator never batches tool calls into an opaque "macro". Each tool intent is executed separately; every call is evaluated by the policy engine and can trigger approval. High-risk sequences are never merged into a single request.

## Sensitive data protection and prompt scrubbing pipeline

- **Pre-prompt sanitizer** (`oxcer-core/src/prompt_sanitizer.rs`) and **data_sensitivity** classifier run on every LLM-bound input (see Security Architecture).
- **Central scrubbing pipeline (every LLM call):** The runner (frontend or backend) that executes `LlmGenerate` must:
  1. Build a combined **raw payload** from: task description, selected file snippets, shell outputs, previous tool outputs, any metadata to send (e.g. `prompt_sanitizer::LlmPayloadParts` + `build_raw_payload`, or equivalent).
  2. Before sending to the provider: run `scrub_for_llm_call(&raw_payload, &options)` (or `build_and_scrub_for_llm(&parts, &options)`). Use the returned **scrubbed** string to reconstruct the prompt/messages that go over the wire. Never use the raw payload for network calls.
  3. **Threshold:** If the pipeline returns `Err(ScrubbingError::TooMuchSensitiveData)` (i.e. ≥50% of the payload was redacted), do **not** call the LLM. Return an error to the Orchestrator (e.g. `StepResult::Err { message: "Context contains too much sensitive data; LLM call skipped. Try tools-only or manually inspect." }`). The Orchestrator can then surface this and optionally fall back to a tools-only strategy for that request.
- Prefer a **single central scrubber**; optional pre-scrubbing at tool boundaries (FS read, Shell output, Log read) is allowed but not required for correctness.

## Observability and logging (Agent session log)

For each completed agent run we persist an **AgentSessionLog** to `logs/agent_sessions.jsonl` (one JSON object per line):

- **AgentSessionLog:** `session_id`, `completed_at` (RFC3339), `workspace_id`, `user_input`, `router_decision`, `selected_model`, `steps`.
- **AgentStepLog:** `step_index`, `kind` (ModelCall | ToolCall | ApprovalWait | System), optional `model_call`, optional `tool_call`.
- **ToolCallLog:** `tool_name`, `args`, `policy_decision` (allowed/denied/approval_required), `approval_id`, `approval_outcome`, `result_summary` (truncated).

Retention: same as `events.log` — 30 days or 10MB, then rotate (drop oldest). Enables "Why did the agent do X?", per-strategy evaluation (cheap vs expensive vs tools-only), and safety tooling.

## Non-goals (Sprint 6)

- No multi-agent graphs or long-horizon planning.
- No advanced memory store (only per-session context).
- Local LLM backend out of scope.
