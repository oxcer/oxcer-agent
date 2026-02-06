# NeMo Guardrails Integration Design for Oxcer/ClawGuard

This document analyzes the NeMo Guardrails codebase and defines a minimal integration so that an enterprise OpenClaw fork (Oxcer/ClawGuard) can use Guardrails as a **policy engine** for: (1) safe tool/terminal execution, (2) hallucination and unsafe-content filtering, and (3) human-in-the-loop approvals.

---

## 1. Short Overview: What NeMo Guardrails Does

### 1.1 LLM input/output filtering

- **Input rails** run on the **user message** before the LLM sees it. They are defined in `config.rails.input.flows` (list of Colang flow names). Each flow can:
  - Call actions (e.g. `SelfCheckInputAction`, `ContentSafetyCheckInputAction`) that return allow/block.
  - On block: `bot refuse to respond` + `abort`, or raise an input rail exception.
- **Output rails** run on the **bot message** after the LLM responds. They are defined in `config.rails.output.flows`. Same pattern: actions decide allow/block; on block: refusal + `abort`.
- **Data flow**: Messages are converted to an internal **event list** (e.g. `UserMessage`, `BotMessage`). The Colang runtime runs flows that react to these events; context passed to actions includes `user_message`, `bot_message`, `last_user_message`, `last_bot_message`, and the triggering `event`.

### 1.2 Hallucination and content safety

- **Self-check hallucination**: Implemented in `library/self_check/` and `library/hallucination/`. The flow `self check hallucination` runs when `$check_hallucination == True`; it calls `SelfCheckHallucinationAction()`, which uses multiple LLM completions and an agreement check. On failure it can send `SelfCheckHallucinationRailException` or `bot inform answer unknown` and `abort`.
- **Content policy**: The `library/content_safety/` flows (e.g. `content safety check input $model`, `content safety check output $model`) call actions that return `{ "allowed": bool, "policy_violations": [...] }`. Same allow/refuse/abort pattern.
- **Self-check input/output**: `SelfCheckInputAction` and `SelfCheckOutputAction` use an LLM with a configurable prompt (e.g. “Should the user/bot message be blocked?”) and return a boolean.

### 1.3 Tool/action validation

- **Tool output rails** run on **proposed tool calls** before execution. They are triggered by a `BotToolCalls` event; the flow receives `$tool_calls` from `$event.tool_calls`. Config: `config.rails.tool_output.flows`.
- **Tool input rails** run on **tool results** (after execution, before the LLM sees them). Triggered by `UserToolMessages`; each tool message is processed by the configured flows. Config: `config.rails.tool_input.flows`.
- **Allow/deny**: A tool output flow can call an action (e.g. validate tool name/args). If it decides to block, it does `bot refuse tool execution` (or a custom refusal message) and `abort`. Then no `StartToolCallBotAction` is emitted, so the response has **no** `tool_calls` and the refusal text is the bot content. If the flow does not abort, `StartToolCallBotAction(tool_calls=$tool_calls)` is emitted and the response includes `tool_calls`.

---

## 2. Concrete List of Code Pieces to Reuse

### 2.1 Python classes and APIs

| Component | Location | Use |
|----------|----------|-----|
| `LLMRails` | `nemoguardrails.rails.llm.llmrails.LLMRails` | Main entry: load config, run input/output/tool rails. |
| `RailsConfig` | `nemoguardrails.rails.llm.config.RailsConfig` | Load from YAML/dict; holds `rails.input`, `rails.output`, `rails.tool_output`, `rails.tool_input`. |
| `GenerationOptions` | `nemoguardrails.rails.llm.options.GenerationOptions` | Control which rails run (e.g. `rails.input`, `rails.tool_output`) and logging. |
| `GenerationResponse` | `nemoguardrails.rails.llm.options.GenerationResponse` | `response` (message list or content), `tool_calls`, `log` (activated_rails, internal_events), `output_data` (if requested). |
| Tool output config | `nemoguardrails.rails.llm.config.ToolOutputRails` | `flows: List[str]` — names of Colang flows that validate tool calls. |

### 2.2 Colang patterns to reuse or adapt

| Pattern | Where | Purpose |
|---------|--------|---------|
| `self check input` | `library/self_check/input_check/flows.co` | Allow/block user message via LLM. |
| `self check output` | `library/self_check/output_check/flows.co` | Allow/block bot message via LLM. |
| `self check hallucination` | `library/hallucination/flows.co` | Detect hallucination; block or warn. |
| `self check facts` | `library/self_check/facts/flows.co` | Fact-check vs. retrieved chunks. |
| `content safety check input/output $model` | `library/content_safety/flows.co` | Policy-based input/output safety. |
| Tool output rail | `tests/test_tool_output_rails.py` (define subflow + bot refuse + abort) | Validate `$tool_calls`; allow or refuse. |

### 2.3 Config and examples to copy/adapt

- **Default LLM flow wiring**: `nemoguardrails/rails/llm/llm_flows.co` — defines when input, output, tool output, and tool input rails run.
- **Rails config shape**: In `config.yml`, under `rails`:
  - `input.flows`, `output.flows`, `tool_output.flows`, `tool_input.flows`.
- **Passthrough + tools**: Docs `docs/integration/tools-integration.md`: use `passthrough: true`, register tools with the LLM; tool calls go through tool output rails when configured.
- **Tool call format**: Same as LangChain/OpenAI: list of `{"name", "args", "id", "type": "tool_call"}` (or with `function.name` / `function.arguments` in some paths).

---

## 3. Integration Design for Oxcer/ClawGuard

### 3.1 Assumptions

- **OpenClaw** owns: GUI, human-approval UI, audit logging, and actual tool/terminal execution.
- **NeMo Guardrails** is used as a **synchronous policy service**: you send it messages and/or a proposed action; it returns allow/deny/needs_human (or equivalent) and optional explanation.

### 3.2 ActionProposal schema (OpenClaw → Guardrails)

For every side-effectful action (shell, file, network, etc.), OpenClaw builds an **ActionProposal** and sends it to Guardrails for a decision. Minimal JSON schema:

```json
{
  "id": "unique-request-id",
  "tool_name": "run_shell",
  "args": { "command": "ls -la", "cwd": "/tmp" },
  "context": {
    "conversation_turn": 3,
    "last_user_message": "list files in /tmp",
    "last_bot_message": ""
  }
}
```

- **id**: For audit and idempotency.
- **tool_name**: Maps to Guardrails “tool” name (e.g. `run_shell`, `write_file`).
- **args**: Tool arguments as a JSON object (Guardrails already validates `tool_calls[].args`).
- **context**: Optional; can be passed as user message or context so Guardrails has conversation context for policy (e.g. self-check prompts).

You can extend this with `risk_hint` (e.g. `"high"`), `category` (e.g. `"shell"`, `"network"`), or `required_approval` if you want to force human-in-the-loop for certain categories.

### 3.3 How to send the proposal to Guardrails and get a decision

**Option A — Use Guardrails as a full message pipeline (recommended for consistency):**

1. Build a message list that ends with an assistant message containing **only** the proposed tool call (no text, or a short description):
   - `messages = [ ..., { "role": "assistant", "content": "", "tool_calls": [ <one tool call from ActionProposal> ] } ]`
2. Call `rails.generate(messages=messages, options=GenerationOptions(...))`.
3. Interpret the result:
   - **Allow**: `result.tool_calls` is non-empty and matches the proposed call (or you only sent one); proceed to execute (or to human approval if you add that layer).
   - **Deny**: `result.tool_calls` is empty and `result.response` (or first message content) contains the refusal text.
   - **Needs human**: Use a convention (see below).

**Option B — Tool-output-only path:**

1. You still need a minimal message list so the runtime can run (e.g. one user message plus one assistant message with `tool_calls`).
2. Same `generate(...)` call; same interpretation of `tool_calls` vs refusal content.

**Response shape you can rely on:**

- `allow`: `GenerationResponse.response` = list with one assistant message, `tool_calls` = list with the allowed tool call(s).
- `deny`: `GenerationResponse.response` = list with one assistant message whose `content` is the refusal string; `tool_calls` = `None` or `[]`.
- `needs_human`: Not natively supported. Two options:
  - **Convention**: In your Colang tool output flow, when the policy says “needs human”, do `bot say "[[GUARDRAILS:NEEDS_HUMAN]]"` (optional: reason) and `abort`. OpenClaw checks `content.startswith("[[GUARDRAILS:NEEDS_HUMAN]]")` and shows the approval UI; do not execute until the user approves.
  - **Logging**: Enable `options.log.activated_rails` and optionally `internal_events`; have your custom action set `additional_info` on the activated rail (if the API allows) or use a special refusal prefix so OpenClaw can parse reason + risk from content.

### 3.4 Routing LLM input/output through Guardrails

- **Input**: Configure `rails.input.flows` (e.g. `self check input`, content safety). Every user message is run through these before the LLM. If you use Guardrails only for tool checks, you can set `options.rails.input = False` when you only want to validate an action.
- **Output**: Configure `rails.output.flows` (e.g. `self check output`, `self check hallucination`, content safety). Every bot message is run through these before returning to the user. So you can route all LLM output through Guardrails and get either the (possibly rewritten) message or a refusal.
- **Pipeline**: OpenClaw can call Guardrails in two ways:
  1. **Full pipeline**: Send user message → Guardrails generates (or you use your own LLM and only use Guardrails for input/output rails). Guardrails returns the safe response or refusal.
  2. **Policy-only for tools**: Your stack does LLM and tool execution; before executing any tool, you call Guardrails with a synthetic message list ending in that tool call and use only `tool_output` rails to get allow/deny/needs_human.

---

## 4. Minimal Code Sketch

Below is a minimal Python sketch that:

1. Builds an `LLMRails` instance with tool output rails (and optionally input/output).
2. For an action proposal, builds a message list ending with that tool call.
3. Calls Guardrails and interprets the result as allow / deny / escalate (needs_human by convention).

See **`examples/clawguard_nemo_integration_sketch.py`** for the full runnable sketch (defines `ActionProposal`, `PolicyDecision`, `check_action_policy()`, and a minimal Colang tool-output flow).

---

## 5. Escalation (human-in-the-loop) Colang pattern

In your tool output flow you can branch on risk and either allow, refuse, or “escalate”:

```colang
define subflow validate action proposal
  $allowed = execute your_validate_action(tool_calls=$tool_calls, context=$context)

  if $allowed == "deny"
    bot refuse tool execution
    abort
  else if $allowed == "needs_human"
    bot say "[[GUARDRAILS:NEEDS_HUMAN]] This action requires approval."
    abort
  # else allow: do not abort; StartToolCallBotAction will be created by default flow
```

Your custom action `your_validate_action` can return a string or a dict (e.g. `{"decision": "allow"|"deny"|"needs_human", "reason": "..."}`) and you map that to the three branches. OpenClaw then:

- On **allow**: execute the tool (or your own approval flow if you always require one).
- On **deny**: show the refusal message and do not execute.
- On **needs_human**: show the message, show approval UI, and only execute after user approval; optionally log to audit.

This keeps Guardrails as a pure policy engine and leaves UI and execution to OpenClaw.
