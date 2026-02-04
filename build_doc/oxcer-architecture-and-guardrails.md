# Oxcer: OpenClaw Architecture Analysis & Enterprise Guardrail Design

This document analyzes the OpenClaw architecture (for an enterprise-grade fork, working name **Oxcer**) and designs integration points for: (1) a security/guardrail layer, (2) a local desktop GUI for non-engineers, and (3) human-in-the-loop approvals with summaries and audit logging. It is based on the published docs and the local codebase; no long-running or external tooling was used.

---

## 1. Architecture Section

### 1.1 Components and Responsibilities

- **Gateway (daemon)**  
  - Single long-lived process. Owns all messaging surfaces (WhatsApp/Baileys, Telegram/grammY, Slack, Discord, Signal, iMessage, WebChat).  
  - Listens on one port (default `127.0.0.1:18789`) for **WebSocket + HTTP** (Control UI, health, tools-invoke, OpenResponses/OpenAI-compat).  
  - Validates inbound frames with JSON Schema (TypeBox → generated schemas).  
  - Emits server-push events: `agent`, `chat`, `presence`, `health`, `heartbeat`, `cron`, `exec.approval.requested`, `exec.approval.resolved`, `node.pair.requested`, etc.  
  - **Does not** run the LLM itself; it routes messages, runs the agent loop in process, and invokes tools (including `callGatewayTool` for `node.invoke`, `exec.approval.request`, etc.).

- **Clients (mac app, CLI, web Control UI)**  
  - Connect over WebSocket with `role: operator` (or equivalent).  
  - First frame must be `connect`; then request/response (`req`/`res`) and events.  
  - Scopes (e.g. `operator.approvals`, `operator.admin`, `operator.pairing`) gate which methods/events they get.  
  - Send: `health`, `status`, `send`, `agent`, `chat`, `exec.approval.request`, `exec.approval.resolve`, `node.invoke`, etc.

- **Nodes (macOS / iOS / Android / headless)**  
  - Connect to the **same** WebSocket server with `role: node`, plus device identity and caps/commands.  
  - Pairing is device-based; approval stored in device pairing store.  
  - Expose commands via `node.invoke`: e.g. `system.run`, `system.which`, `system.execApprovals.get/set`, `canvas.*`, `camera.*`, `screen.record`, `location.get`, `sms.send` (Android).  
  - **Node host** (headless or macOS app) runs on the machine where execution happens; approvals are enforced **on the node host** via `~/.openclaw/exec-approvals.json` (and optionally macOS app UI).

- **Agent runtime (in-process)**  
  - Triggered by Gateway when a client sends `agent` or `chat` (or when OpenResponses/OpenAI HTTP triggers a run).  
  - `agentCommand` runs the agent loop: message → LLM → tool calls → execution → result → back to LLM.  
  - Tools are created by `createOpenClawTools()` (and similar) and include `exec`, `read`, `write`, `nodes`, `browser`, etc.  
  - Tool execution that hits the host (gateway or node) goes through: **exec tool** → `callGatewayTool("exec.approval.request", …)` when approval required → Gateway broadcasts `exec.approval.requested` → client resolves with `exec.approval.resolve` → exec tool then calls `node.invoke` (or local exec on gateway).

- **Control UI (dashboard)**  
  - Served at `/` on the Gateway port (or `gateway.controlUi.basePath`).  
  - Authenticates via `connect.params.auth` (token or password). Stores token in `localStorage`.  
  - Subscribes to `exec.approval.requested` (scope `operator.approvals`) and shows approval UI; sends `exec.approval.resolve`.  
  - Also: config, nodes, exec approvals editing (allowlists, defaults), chat.

- **Canvas host**  
  - Separate port (default 18793) for agent-editable HTML and A2UI; not in the critical path for exec/approvals.

### 1.2 Data Flow: Chat Message → Gateway → LLM → Tool → Node → Client

```
User message (channel or Control UI / chat)
    → Gateway receives via channel ingest or WS req `agent` / `chat`
    → agentCommand(...) runs with sessionKey, channel, etc.
    → LLM called (provider API) with system prompt + history + tools
    → LLM returns tool_calls (e.g. exec with command, host=node)
    → Exec tool (createExecTool) runs in same process:
        - If host=node and approval required: callGatewayTool("exec.approval.request", { command, cwd, host, agentId, ... })
        - Gateway: ExecApprovalManager.create(); broadcast("exec.approval.requested"); waitForDecision()
        - Control UI (or Discord/macOS app) receives event; user Approve/Deny
        - Client sends exec.approval.resolve(id, decision)
        - Gateway: manager.resolve(); broadcast("exec.approval.resolved"); request promise resolves
    → Exec tool then callGatewayTool("node.invoke", { nodeId, command: "system.run", params: { command, cwd, env, approved, approvalDecision, runId } })
    → Gateway looks up node by nodeId, forwards to node’s WS connection
    → Node host (or macOS app) receives invoke; enforces local exec-approvals.json (allowlist/ask); runs command; returns result
    → node.invoke.result sent back to Gateway → returned to exec tool
    → Tool result back to agent loop → LLM gets result → next turn or final reply
    → Reply delivered to user (channel or chat UI) and/or streamed via event:agent
```

Important detail: **side-effectful actions** are triggered **inside the agent process** when the LLM’s tool call is executed. The Gateway’s role is to: (1) run the agent loop, (2) expose `exec.approval.request`/`resolve` and `node.invoke` so the exec tool can pause for human approval and then run on the node, and (3) broadcast approval events to subscribed clients.

### 1.3 Where Terminal/Shell Commands Are Represented

- **Exec tool args** (agent-facing): `command` (string), `workdir`/`cwd`, `env`, `timeout`, `host` (sandbox | gateway | node), `security`, `ask`, `node` (when host=node).  
- **Gateway protocol**:  
  - `exec.approval.request` params: `command`, `cwd`, `host`, `security`, `ask`, `agentId`, `resolvedPath`, `sessionKey`, `timeoutMs`, optional `id`.  
  - `node.invoke` params: `nodeId`, `command: "system.run"`, `params: { command (argv string), rawCommand, cwd, env, timeoutMs, agentId, sessionKey, approved, approvalDecision, runId }`.  
- **Node host / macOS app**: Receives `system.run` with the same semantics; evaluates allowlist and ask/fallback from `~/.openclaw/exec-approvals.json`; runs via shell and returns stdout/stderr/exitCode.

So: the **canonical** representation of “what will run” is the `command` string (and cwd, env) in the exec tool and in `exec.approval.request` / `node.invoke` params. There is no separate “proposed action” schema beyond this today.

### 1.4 Security Boundaries (Current)

- **Auth**: Gateway WS requires `connect` with `auth.token` or `auth.password` (or Tailscale identity when allowed). Device pairing for new device IDs (local can auto-approve).  
- **Channel access**: DM/group policies (pairing, allowlist, open), group allowlists, mention gating.  
- **Tool blast radius**: Tool policy (allow/deny lists per agent), sandbox (Docker workspace, optional read-only), elevated mode (allowFrom), exec approvals (deny/allowlist/full + ask + askFallback).  
- **Exec approvals**: Enforced **on the execution host** (gateway process or node host). Gateway does not execute shell commands itself for host=node; it forwards to the node. For host=gateway, the Oxcer/Gateway process runs exec and can use the same approval flow (request → broadcast → resolve) and local exec-approvals file.  
- **Plugins**: In-process; trusted code. Config allowlists recommended.

### 1.5 Call-Flow Summary (Text Diagram)

```
[Channel or Chat UI] --> message
    --> Gateway (ingest or WS agent/chat)
    --> agentCommand()
        --> LLM (provider)
        --> tool_calls
        --> createExecTool().execute()
            --> if approval needed: callGatewayTool("exec.approval.request")
                --> Gateway: ExecApprovalManager.create(); broadcast("exec.approval.requested")
                --> [Control UI / Discord / macOS] receives event
                --> user clicks Approve/Deny --> exec.approval.resolve(id, decision)
                --> Gateway: manager.resolve() --> request promise resolves
            --> callGatewayTool("node.invoke", system.run params)
                --> Gateway routes to node by nodeId
                --> Node host: allowlist/ask check; run command; return result
            --> node.invoke result --> exec tool result
        --> agent loop continues
    --> reply --> deliver to user / event:agent
```

---

## 2. Guardrail Integration Plan

Goal: reduce risk from over-powered terminal/file permissions and from LLM hallucinations by validating actions and results before they affect real systems. Below are **concrete extension points** and how to fail closed.

### 2.1 Pre-LLM: Policy / Guardrails Proxy in Front of LLM Calls

- **Where**: In the agent loop, **before** sending the user/tool turn to the LLM (or before the first user message in a turn).  
- **Location in code**: The code that builds the messages array and calls the provider (e.g. in the agent/run loop that invokes the LLM).  
- **What to pass**: Full prompt (system + user + assistant + tool results), optional tool schemas, `sessionKey`, `agentId`, `channel`, `accountId`.  
- **Integration**:  
  - Optional wrapper: `guardrailCheckPrompt({ messages, tools, context }) => { allowed, rewritten?, reason? }`.  
  - If a “policy engine” or NeMo Guardrails proxy is used, call it here; if it returns block or rewrite, either block the turn or substitute `rewritten` messages.  
- **Fail closed**: If the guardrail service is unreachable, treat as “deny” (abort turn or use a safe default) unless explicitly configured otherwise. Config key suggestion: `guardrails.prompt.mode: "off" | "warn" | "enforce"` and `guardrails.prompt.failClosed: true`.

### 2.2 Pre-Exec: Pre-Execution Hook for Shell / File / Network Tools

- **Where**: Inside the **exec tool** (and optionally other side-effectful tools: `write`, `apply_patch`, `browser`, `web_fetch`, etc.), **before** calling `exec.approval.request` or actually executing.  
- **What we have**: Tool name, tool args (e.g. `command`, `cwd`, `env`, `host`, `nodeId`), `agentId`, `sessionKey`, channel/metadata from run context.  
- **Proposed “proposed action” object** (to pass to guardrail layer):

```ts
interface ProposedAction {
  id: string;
  tool: string;
  args: Record<string, unknown>;
  summary?: string;        // optional NL summary (e.g. from LLM or template)
  riskLevel?: "low" | "medium" | "high" | "critical";
  sessionKey?: string;
  agentId?: string;
  channel?: string;
  timestamp: number;
}
```

- **Integration**:  
  - Before doing approval or execution: `guardrailCheckAction({ action }) => { allowed, summary?, reason? }`.  
  - If `allowed === false`, throw or return a tool error (no exec, no approval request).  
  - If a human approval flow is desired, the guardrail can return `allowed: "approval_required"` and a `summary`; then the existing exec approval flow can carry this summary.  
- **Fail closed**: If the guardrail service is unreachable, config `guardrails.exec.failClosed: true` → treat as deny (do not run and do not request approval).

### 2.3 Approval Request Enhancement: Add Summary and Risk to Existing Flow

- **Where**: When creating the exec approval record in the Gateway (`exec.approval.request` handler) and when broadcasting `exec.approval.requested`.  
- **Current payload**: `id`, `request: { command, cwd, host, security, ask, agentId, resolvedPath, sessionKey }`, `createdAtMs`, `expiresAtMs`.  
- **Extension**: Add optional `summary` (human-readable string) and `riskLevel` to the request payload. The exec tool (or a guardrail layer) can compute them before calling `exec.approval.request`.  
- **Control UI / desktop app**: Show `summary` and `riskLevel` in the approval dialog; no protocol change to resolve (still `id` + `decision`).

### 2.4 Post-Exec: Post-Execution Validator

- **Where**: After a tool returns (e.g. exec, read, web_fetch), before the result is passed back to the LLM or stored.  
- **What we have**: Tool name, args, result (stdout/stderr/exitCode or response body), `sessionKey`, `agentId`.  
- **Integration**: `guardrailCheckResult({ tool, args, result, context }) => { allowed, redactedResult?, reason? }`.  
- **Fail closed**: If validator unreachable, config `guardrails.result.failClosed: true` → e.g. substitute a generic “Result withheld by policy” and optionally log the real result for audit.

### 2.5 Extension Points Summary

| Point              | Location              | Inputs                          | Output / behavior              | Fail closed              |
|--------------------|-----------------------|----------------------------------|--------------------------------|---------------------------|
| Pre-LLM            | Agent loop, before LLM | messages, tools, context         | allow / block / rewrite        | deny turn if unreachable  |
| Pre-Exec           | Exec (and other tools)| ProposedAction                   | allow / deny / approval_required | deny execution          |
| Approval payload   | exec.approval.request | existing + summary, riskLevel    | same resolve flow               | N/A                       |
| Post-Exec          | After tool return     | tool, args, result, context      | allow / redact / block         | withhold result           |

Implementing these requires: (1) a small guardrail client module (HTTP or in-process), (2) config for endpoints and fail-closed behavior, and (3) wiring in the agent loop and in the exec (and optionally other) tools.

---

## 3. Local Desktop App Plan

Goal: non-technical users can use OpenClaw like a local ChatGPT app: simple install, one LLM API key, no Docker/terminals/config files. The app talks to a local OpenClaw Gateway under the hood.

### 3.1 Start/Stop Local Gateway from the App

- **Current state**: Gateway is started by `pnpm oxcer gateway run` (foreground) or by the macOS menubar app (which spawns and supervises the Gateway). There is no separate LaunchAgent label; “restart via app or scripts/restart-mac.sh”.  
- **Desktop app options**:  
  - **Tauri/Electron**: Bundle the Oxcer CLI (or a minimal Node runner that loads the gateway from the same repo/package). On startup, if no Gateway is reachable at `ws://127.0.0.1:18789`, spawn `oxcer gateway run --bind loopback --port 18789` as a child process (e.g. from repo root `/Users/lucasmac/Documents/GitHub/OpenSource/oxcer`); capture stdout/stderr for logs; on quit, terminate the process.  
  - **Alternative**: Rely on the user to start the Gateway once (e.g. “Start OpenClaw” in the app runs the same spawn logic; “Stop” kills the child). No system-wide service required for the minimal case.  
- **Recommendation**: Single-binary or packaged app that **starts the Gateway as a child process** when “Start” is clicked and stops it on “Quit”, with a clear “Gateway status” indicator (connecting / connected / error). Fallback: “Connect to existing Gateway” with URL + token for power users.

### 3.2 Storing and Using LLM API Keys

- **Current**: Model auth lives in `~/.openclaw/agents/<agentId>/agent/auth-profiles.json`; CLI/config use `pnpm oxcer config set` (or `oxcer config set`) and provider-specific keys.  
- **Desktop app**:  
  - **Simple path**: On first run, show “Add API key” (e.g. Anthropic, OpenAI). Store in the same auth-profiles structure under a default agent (e.g. `main`) so the Gateway’s agent loop can use it. Write to `~/.openclaw/agents/main/agent/auth-profiles.json` (or equivalent) with the same schema the Gateway already reads.  
  - **Isolation**: Use the app’s own config directory only if we want to avoid touching the global `~/.openclaw` (e.g. `~/Library/Application Support/Oxcer` on macOS); then the spawned Gateway must be started with `OPENCLAW_STATE_DIR` or equivalent so it reads that directory.  
- **Security**: Prefer OS keychain for the API key (e.g. Electron safeStorage / Tauri secure storage) and write a temporary or in-memory profile when launching the Gateway, or pass env vars to the child process so the key is not written to disk in plain text.

### 3.3 Connecting the App to the Gateway

- **URL**: Default `ws://127.0.0.1:18789` (loopback).  
- **Auth**: Token from `gateway.auth.token` (or generated during onboarding). The app can generate a token at first run and write it to the Gateway config (if the app owns the config) or use a fixed token that the bundled Gateway is started with.  
- **Scopes**: Connect with `operator` role and scopes `operator.admin`, `operator.approvals`, `operator.pairing` so the app can chat, receive approval events, and manage config/nodes if needed.  
- **Flow**: On load, connect WebSocket → send `connect` with `auth.token` and device identity (optional for local). Then use `chat` or `agent` for sending messages and subscribe to `agent` and `exec.approval.requested` events.

### 3.4 Minimal UI Layout

- **Chat panel**: Single conversation (or session selector). Input box + send; streamed replies via `event:agent`.  
- **Pending approvals panel**: When `exec.approval.requested` is received, show a card with: command, cwd, optional summary/risk, and buttons: **Approve once**, **Always allow**, **Deny**. On action, call `exec.approval.resolve(id, decision)`.  
- **Settings**: Model selector (from Gateway config or provider list), API key (masked) with “Edit” opening keychain or a secure input, “Security level” (e.g. Ask on miss / Always ask / Deny exec).  
- **Gateway status**: Indicator (e.g. “Running” / “Connecting” / “Stopped”) and optional “Start Gateway” / “Stop Gateway” if the app owns the process.

Embedding the existing dashboard: the current Control UI is HTTP at `/` on the Gateway. The desktop app can host a **WebView** that loads `http://127.0.0.1:18789/?token=...` so the full dashboard is available in a tab; the minimal “chat + approvals” view can be a separate tab or the default home to keep the UX simple for non-engineers.

---

## 4. Human-in-the-Loop Approvals (Generic Design)

Extend the existing exec-approval pattern to a **generic approval workflow** for any high-impact action (shell, file, API, infra), with a clear summary and audit log.

### 4.1 Abstraction: “Proposed Action”

Define a common shape that any tool can emit when it needs approval:

```ts
interface ProposedAction {
  id: string;
  tool: string;
  args: Record<string, unknown>;
  summary: string;           // Human-readable: "Run shell command: ls -la /etc"
  riskLevel: "low" | "medium" | "high" | "critical";
  sessionKey?: string;
  agentId?: string;
  channel?: string;
  timestamp: number;
  expiresAt: number;
}
```

- **summary**: Generated by the tool (or a guardrail) from the tool name + args; e.g. “Run: rm -rf /tmp/foo”, “Edit file: /etc/hosts”, “Send message to channel #general”.  
- **riskLevel**: Can be derived from tool name (exec → high), path (e.g. /etc), or a small rules engine.

### 4.2 Transport to the GUI (or Any Client)

- **Option A (extend current protocol)**: Reuse `exec.approval.request` and `exec.approval.requested` but allow an optional generic payload: e.g. `action: ProposedAction` in addition to `request: { command, cwd, ... }`. Clients that only know exec can ignore `action.summary`; new clients display it.  
- **Option B (new event/method)**: Add `action.approval.request` and event `action.approval.requested` with payload `ProposedAction`. The Gateway holds a generic “approval manager” (same pattern as ExecApprovalManager) keyed by `id`.  
- **Recommendation**: Option A for minimal change: add `summary` and `riskLevel` (and optional `actionId` for correlation) to the existing exec approval payload. For a fully generic system, add Option B so non-exec tools (write, apply_patch, web_fetch, etc.) can request approval with the same UX.

### 4.3 How the GUI Sends Back Approve/Reject

- **Current**: Client sends `exec.approval.resolve` with `{ id, decision: "allow-once" | "allow-always" | "deny" }`. Optional comment could be added as a third field.  
- **Generic**: Same for a generic flow: `action.approval.resolve` (or keep `exec.approval.resolve` for exec) with `id`, `decision`, and optional `comment` for the audit log.

### 4.4 How the Gateway Proceeds or Aborts and Logs the Decision

- **Proceed**: The pending tool call is waiting on `manager.waitForDecision()`. When a client calls `resolve(id, decision)`, the manager resolves the promise; the exec tool (or other tool) then continues and runs the command (or returns a “denied” tool result).  
- **Abort**: If decision is `deny` or timeout, the tool returns a structured error or message (“Exec denied (user-denied)”); the agent sees it and can reply to the user.  
- **Audit log**: Today, exec lifecycle is surfaced as system messages (Exec running, Exec finished, Exec denied). For a generic audit log:  
  - Persist each `ProposedAction` and the resolution (decision, resolvedBy, timestamp, optional comment) to a dedicated audit store (e.g. `~/.openclaw/audit/approvals.jsonl` or a table).  
  - Include: actionId, tool, summary, riskLevel, decision, resolvedBy, comment, createdAt, resolvedAt.  
  - The Gateway can write this in the same place that handles `exec.approval.resolve` (and in a future `action.approval.resolve`).

### 4.5 Sequence Diagram (Text)

```
Agent (exec tool)                Gateway                    Control UI / Client
       |                            |                                |
       | exec.approval.request      |                                |
       | (command, cwd, summary?,   |                                |
       |  riskLevel?)               |                                |
       |--------------------------->|                                |
       |                            | create record; broadcast       |
       |                            | "exec.approval.requested"      |
       |                            |------------------------------->|
       |                            |                                | (show dialog:
       |                            |                                |  summary, risk,
       |                            |                                |  Approve/Deny)
       |                            |     exec.approval.resolve      |
       |                            |     (id, decision, comment?)   |
       |                            |<-------------------------------|
       |                            | manager.resolve(id, decision)  |
       |  promise resolves          |                                |
       |<---------------------------|                                |
       | (if allow: node.invoke     |                                |
       |  or host exec)             |                                |
       |                            | (optional: append to           |
       |                            |  audit log)                    |
```

### 4.6 Cursor-Like UX

- **Before execution**: Show a single modal or panel: “The agent wants to run: `rm -rf /tmp/x`. Risk: high. [Approve once] [Always allow] [Deny].”  
- **Generalization**: Same UI for “Edit file …”, “Send message to …”, “Call API …” by using the `summary` and `riskLevel` from the proposed action.  
- **Persistence**: Audit log entry for every proposed action and decision so operators can review later.

---

## 5. References (Docs and Code)

- **Docs (fetched)**:  
  - [Gateway Architecture](https://docs.openclaw.ai/concepts/architecture)  
  - [Nodes](https://docs.openclaw.ai/nodes), [CLI nodes](https://docs.openclaw.ai/cli/nodes)  
  - [Gateway Security](https://docs.openclaw.ai/gateway/security)  
  - [Web Dashboard](https://docs.openclaw.ai/web/dashboard)  
  - [Exec Approvals](https://docs.openclaw.ai/tools/exec-approvals)  
- **Local docs**: `docs/concepts/architecture.md`, `docs/tools/exec-approvals.md`.  
- **Key source**:  
  - Gateway: `src/gateway/server.impl.ts`, `server-methods/agent.ts`, `server-methods/chat.ts`, `server-methods/exec-approval.ts`, `server-methods/exec-approvals.ts`, `server-methods/nodes.ts`, `exec-approval-manager.ts`, `server-broadcast.ts`.  
  - Exec tool & approvals: `src/agents/bash-tools.exec.ts`, `src/infra/exec-approvals.ts`, `src/infra/exec-approval-forwarder.ts`, `src/node-host/runner.ts`.  
  - Protocol: `src/gateway/protocol/schema/exec-approvals.ts`, `src/agents/tools/gateway.ts` (`callGatewayTool`).  
  - UI: `ui/src/ui/app-gateway.ts` (exec.approval.requested), `ui/src/ui/gateway.ts` (scopes).

---

This gives a precise mental model of OpenClaw’s architecture, where to plug in guardrails, how to design a local desktop app, and how to generalize human-in-the-loop approvals with summaries and audit logging for an enterprise fork like Oxcer.
