# Guardrails smoke test plan (Oxcer)

Single short test pass to validate guardrails in critical paths, config toggles, and audit trail before introducing NeMo or external policy engines.

**Summary of scenarios**

| Scenario | Trigger | Guardrail path | Audit events (current) |
|----------|---------|----------------|------------------------|
| A – Exec | `pnpm oxcer agent --message "Run: echo hello" ...` | `runActionGuardrail` → `runResultGuardrail` (in exec tool) | `guardrail.action`, `guardrail.result` (only if audit config is set, e.g. gateway started) |
| B – node.invoke | Gateway RPC `node.invoke` (e.g. test client or exec on node host) | `runNodeInvokeGuard` in gateway | `guardrail.node.invoke`, `guardrail.node.invoke.complete` |
| C – web_fetch | `POST /tools/invoke` with `tool: "web_fetch"` | None (HTTP path does not use guardrails yet) | None |
| D – write | `POST /tools/invoke` with `tool: "write"` | None (not wrapped yet) | None |

---

## 1. Dev config for the test

Use a local config that enables guardrails in **warn** mode and audit with dev console. Either merge the fragment below into your main config (e.g. `~/.openclaw/openclaw.json`) or put it in a dedicated file and point `OPENCLAW_CONFIG_PATH` at it.

**Minimal JSON fragment:**

```json
{
  "guardrails": {
    "action": { "mode": "warn" },
    "result": { "mode": "warn" },
    "failClosed": false,
    "audit": {
      "enabled": true,
      "devConsole": true,
      "filePath": "/tmp/oxcer-audit/actions.jsonl"
    }
  }
}
```

- `guardrails.action.mode` / `guardrails.result.mode`: `"warn"` so checks run but do not block (no deny/needs_human enforcement).
- `guardrails.failClosed`: `false` so guard check errors do not deny the action.
- `guardrails.audit.enabled`: `true` so events are written to JSONL.
- `guardrails.audit.devConsole`: `true` so each event is also logged with `console.debug` when `NODE_ENV=development`.
- `guardrails.audit.filePath`: optional; omit to use default `$OPENCLAW_STATE_DIR/audit/actions.jsonl`, or set to a temp path (e.g. `/tmp/oxcer-audit/actions.jsonl`) for easy inspection.

**To see dev console output:** run gateway/agent with `NODE_ENV=development` (e.g. `NODE_ENV=development pnpm oxcer gateway run` or `NODE_ENV=development pnpm oxcer agent ...`). Run from the Oxcer repo root (e.g. `/Users/lucasmac/Documents/GitHub/OpenSource/oxcer`).

---

## 2. Smoke-test scenarios

Four scenarios covering exec, node.invoke, a network tool (web_fetch), and a file tool (write). Current code: **exec** and **node.invoke** are guarded and produce audit events; **web_fetch** and **write** when invoked via HTTP `/tools/invoke` are not yet wrapped with guardrails (Sprint 5), so those scenarios document expected behavior once that path is implemented.

### Scenario A – Exec (shell)

| Item | Description |
|------|-------------|
| **Command / call** | Agent run that triggers the exec tool, e.g.: `pnpm oxcer agent --message "Run the shell command: echo hello" --session-id smoke-test --thinking low` (you must pass one of `--to <E.164>`, `--session-id`, `--sessionKey`, or `--agent` to choose a session). Run from repo root. |
| **Expected functional behavior** | Exec runs `echo hello`; agent returns the command output. |
| **Guardrail behavior** | `runActionGuardrail` and `runResultGuardrail` run (in `src/agents/bash-tools.exec.ts`); `checkAction` and `checkResult` are invoked. With default noop policy, decision is typically `allow`. In warn mode, deny/needs_human would not block. |
| **Result redaction** | Only if result guardrail returns redact (e.g. deny in enforce mode); not expected with allow. |
| **Audit events** | `guardrail.action` (tool `exec`, decision, mode, failClosed, actionId, args); then `guardrail.result` (tool `exec`, decision, redacted: false). **Note:** Audit config is only set when the gateway starts (`setGuardrailAuditConfig` in `server.impl.ts`). A standalone `pnpm oxcer agent` CLI run does not set audit config, so no JSONL is written. To see exec audit events, run the gateway first (with guardrails config and `NODE_ENV=development`), then trigger an agent run that uses exec via a gateway-mediated flow (e.g. channel message or HTTP API that runs the agent on the gateway). |
| **Dev console** | When audit config is set and `NODE_ENV=development`: `[guardrail:action] <actionId> exec allow warn` and `[guardrail:result] exec allow false warn`; `[guardrail:metric] guardrail.action exec allow low` (or similar). |

### Scenario B – node.invoke

| Item | Description |
|------|-------------|
| **Command / call** | Trigger gateway `node.invoke` RPC. Options: (1) Run an agent that uses exec on a **node host** (connected device), or (2) With gateway running, send a single `node.invoke` (e.g. test client or curl to a WebSocket gateway endpoint). For a quick check without a real node: start gateway, then use a test script or WS client to call `node.invoke` with a non-existent `nodeId`; the guard runs before the “not connected” response. |
| **Expected functional behavior** | With valid node: command runs on node. With fake nodeId: gateway responds with UNAVAILABLE / NOT_CONNECTED. |
| **Guardrail behavior** | `runNodeInvokeGuard` runs `checkAction` for the node command. In warn mode, decision is logged only; proceed is true (unless deny in enforce). |
| **Audit events** | `guardrail.node.invoke` (nodeId, command, proceed, decision); `guardrail.node.invoke.complete` (nodeId, command, success: true/false, errorMessage if failed). In warn mode, `recordGuardrailNodeInvoke` is called asynchronously, so in the JSONL file `guardrail.node.invoke.complete` may appear before `guardrail.node.invoke`. |
| **Dev console** | `[guardrail:node.invoke] <nodeId> <command> true allow` and `[guardrail:node.invoke.complete] <nodeId> <command> true/false`; metric line for node.invoke. |

### Scenario C – Network tool (web_fetch)

| Item | Description |
|------|-------------|
| **Command / call** | HTTP: `curl -s -X POST http://127.0.0.1:PORT/tools/invoke -H "Content-Type: application/json" -H "Authorization: Bearer TOKEN" -d '{"tool":"web_fetch","args":{"url":"https://example.com"}}'` (replace PORT and TOKEN). The `web_fetch` tool requires `url` in args. Or agent: `pnpm oxcer agent --message "Fetch the content of https://example.com" --session-id smoke-test --thinking low` (pass `--to`, `--session-id`, `--sessionKey`, or `--agent`). |
| **Expected functional behavior** | Tool returns fetched content (or error). |
| **Guardrail behavior** | **Current:** HTTP `/tools/invoke` does not use `executeWithGuardrails`; no action/result guardrail calls. **After Sprint 5:** `checkAction` and `checkResult` should run; expect `allow` with default policy. |
| **Audit events** | **Current:** None for this path. **After guardrails on HTTP:** `guardrail.action` (tool `web_fetch`), then `guardrail.result`. |
| **Dev console** | **Current:** no guardrail logs for this call. **After:** same pattern as exec (action + result + metric). |

### Scenario D – File tool (write)

| Item | Description |
|------|-------------|
| **Command / call** | HTTP: `curl -s -X POST http://127.0.0.1:PORT/tools/invoke -H "Content-Type: application/json" -H "Authorization: Bearer TOKEN" -d '{"tool":"write","args":{"path":"/tmp/oxcer-smoke.txt","content":"smoke"}}'` (the write tool accepts `path` or `file_path` and `content`). Or agent: `pnpm oxcer agent --message "Write the word smoke to the file /tmp/oxcer-smoke.txt" --session-id smoke-test --thinking low` (ensure workspace allows that path or use a path under workspace). |
| **Expected functional behavior** | File is written (or policy/validation error). |
| **Guardrail behavior** | **Current:** No guardrails on write (neither agent path nor HTTP path uses `executeWithGuardrails`). **After Sprint 5:** action/result checks and audit expected. |
| **Audit events** | **Current:** None. **After:** `guardrail.action` (tool `write`), `guardrail.result`. |
| **Dev console** | **Current:** none. **After:** action + result + metric. |

---

## 3. How to run and inspect each scenario

### Scenario A – Exec

- **Run:**  
  `NODE_ENV=development pnpm oxcer agent --message "Run the shell command: echo hello" --session-id smoke-test --thinking low`  
  (Pass one of `--to`, `--session-id`, `--sessionKey`, or `--agent`. Create session first if needed.)
- **Audit file:** Default `$OPENCLAW_STATE_DIR/audit/actions.jsonl`, or the path set in `guardrails.audit.filePath` (e.g. `/tmp/oxcer-audit/actions.jsonl`). **Note:** Audit config is only set at gateway startup; a standalone CLI agent run does not set it, so no JSONL will be written for exec unless the agent is triggered via the gateway (e.g. start gateway, then trigger agent through channel or HTTP).
- **Inspect:**  
  `tail -n 10 "$OPENCLAW_STATE_DIR/audit/actions.jsonl"`  
  or  
  `tail -n 10 /tmp/oxcer-audit/actions.jsonl`
- **Fields to check:** `eventType` (`guardrail.action`, `guardrail.result`), `tool` (`exec`), `decision` (e.g. `allow`), `mode` (`warn`), `failClosed` (false), `sessionKey`/`agentId` if present.

### Scenario B – node.invoke

- **Run:** Start gateway with guardrails config and `NODE_ENV=development`. Trigger `node.invoke` (e.g. client that sends `{ method: "node.invoke", params: { nodeId: "test-node", command: "ping", params: {} } }`). For a minimal check, use a non-existent nodeId to get guard + then UNAVAILABLE.
- **Audit file:** Same as above.
- **Inspect:**  
  `tail -n 10 "$OPENCLAW_STATE_DIR/audit/actions.jsonl"`
- **Fields to check:** `eventType` (`guardrail.node.invoke`, `guardrail.node.invoke.complete`), `nodeId`, `command`, `proceed`, `decision`, `success`, `errorMessage` (if failed).

### Scenario C – web_fetch (HTTP)

- **Run:** Gateway must be running with auth. Then:  
  `curl -s -X POST "http://127.0.0.1:${PORT}/tools/invoke" -H "Content-Type: application/json" -H "Authorization: Bearer ${TOKEN}" -d '{"tool":"web_fetch","args":{"url":"https://example.com"}}'`
- **Inspect:** Same audit file. **Current:** no new guardrail lines. **After Sprint 5:** look for `guardrail.action` and `guardrail.result` with `tool: "web_fetch"`.

### Scenario D – write (HTTP)

- **Run:**  
  `curl -s -X POST "http://127.0.0.1:${PORT}/tools/invoke" -H "Content-Type: application/json" -H "Authorization: Bearer ${TOKEN}" -d '{"tool":"write","args":{"path":"/tmp/oxcer-smoke.txt","content":"smoke"}}'`
- **Inspect:** Same audit file. **Current:** no guardrail events for write. **After Sprint 5:** `guardrail.action` / `guardrail.result` with `tool: "write"`.

### Quick audit checklist (any scenario)

- **Location:** `$OPENCLAW_STATE_DIR/audit/actions.jsonl` or `guardrails.audit.filePath`.
- **Last N lines:** `tail -n 10 <path>` or `tail -n 20`.
- **Key fields:** `eventType`, `tool` (or `command` for node), `decision`, `mode`, `failClosed`, `sessionKey`, `agentId`, `timestamp` (RFC3339), `eventId` (UUID). For `guardrail.node.invoke.complete`, key fields are `nodeId`, `command`, `success`, `errorMessage` (if failed); no `mode`/`failClosed`.

---

## 4. Simulated run (code paths and audit events)

For each scenario, which guardrail and audit functions run, and what the JSONL lines look like.

| Scenario | Guardrail functions | Audit functions | Example JSONL (eventType and main fields) |
|----------|---------------------|-----------------|------------------------------------------|
| **A – Exec** | `runActionGuardrail`, `runResultGuardrail` (in `bash-tools.exec.ts`) | `recordGuardrailActionDecision`, `recordGuardrailResultDecision` | `guardrail.action`: `tool: "exec"`, `decision`, `mode`, `failClosed`, `actionId`, `args`. `guardrail.result`: `tool: "exec"`, `decision`, `redacted: false`, `mode`. |
| **B – node.invoke** | `runNodeInvokeGuard` (in `nodes.helpers.ts`, called from `nodes.ts` handler) | `recordGuardrailNodeInvoke` (warn: async), `recordGuardrailNodeInvokeComplete` (in `nodes.ts`) | `guardrail.node.invoke`: `nodeId`, `command`, `proceed`, `decision`. `guardrail.node.invoke.complete`: `nodeId`, `command`, `success`, `errorMessage` (if failed). |
| **C – web_fetch** | None (HTTP `/tools/invoke` calls `tool.execute` directly; no `executeWithGuardrails`) | None | — |
| **D – write** | None | None | — |

Event shapes match `src/guardrails/audit.ts`: `GuardrailActionAuditEntry`, `GuardrailResultAuditEntry`, `GuardrailNodeInvokeAuditEntry`, `GuardrailNodeInvokeCompleteEntry`. Each written line includes `eventId` (UUID), `eventType`, and `timestamp` (RFC3339).

---

## References

- Milvus: [How do you test the effectiveness of LLM guardrails](https://milvus.io/ai-quick-reference/how-do-you-test-the-effectiveness-of-llm-guardrails)
- ArXiv: [Guardrails and evaluation](https://arxiv.org/html/2601.20727v1)
