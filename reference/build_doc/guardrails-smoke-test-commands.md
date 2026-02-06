# Guardrails smoke test – copy-pastable commands

Generated from current code and `build_doc/guardrails-smoke-test.md`. Use this to run the smoke test in your dev environment. **Run all commands from the Oxcer repo root** (e.g. `/Users/lucasmac/Documents/GitHub/OpenSource/oxcer`) using the CLI as `pnpm oxcer ...` (not a globally installed `openclaw`).

**Note:** `executeWithGuardrails.ts` does not exist in this repo. Exec and node.invoke are guarded via `runActionGuardrail`/`runResultGuardrail` and `runNodeInvokeGuard`; HTTP `/tools/invoke` does not use guardrails yet (no audit for C/D).

---

## Section 1: Config snippet and gateway start

### 1.1 Minimal config (guardrails warn + audit)

Put this in `~/.openclaw/openclaw.json` (or a file pointed to by `OPENCLAW_CONFIG_PATH`). Merge with your existing config if needed.

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
  },
  "gateway": {
    "auth": { "token": "YOUR_DEV_TOKEN" }
  }
}
```

- Replace `YOUR_DEV_TOKEN` with a secret (e.g. `dev-token-$(openssl rand -hex 8)`). HTTP calls to the gateway use `Authorization: Bearer YOUR_DEV_TOKEN`.
- Or set `OPENCLAW_GATEWAY_TOKEN=YOUR_DEV_TOKEN` and omit `gateway.auth.token` (auth resolves token from config or env).
- Default gateway port is **18789** (override with `gateway.port` or `OPENCLAW_GATEWAY_PORT`).

### 1.2 Start the gateway (required for audit and for C/D)

Audit config is set only when the gateway starts (`setGuardrailAuditConfig` in `server.impl.ts`). Start the gateway first so the audit file is used.

```bash
export NODE_ENV=development
export OPENCLAW_GATEWAY_TOKEN="YOUR_DEV_TOKEN"   # if not in config
pnpm oxcer gateway run
```

Or with port explicit:

```bash
NODE_ENV=development OPENCLAW_GATEWAY_TOKEN="YOUR_DEV_TOKEN" pnpm oxcer gateway run --port 18789
```

Leave this running. Default bind is loopback; port from config or `OPENCLAW_GATEWAY_PORT` or 18789.

---

## Section 2: Per-scenario commands and behavior

### Scenario A – Exec via agent

**Exact command (run in a second terminal; session must exist or use `--agent main`):**

```bash
NODE_ENV=development pnpm oxcer agent \
  --message "Run the shell command: echo hello" \
  --session-id smoke-test-exec \
  --thinking low
```

If you don’t have a session yet, use one of:

- `--agent main` (replace `--session-id smoke-test-exec`), or  
- `--to +15555550123` (fake E.164), or  
- Create a session first via wizard / `pnpm oxcer agents add` and use its session key.

**Guardrail functions that run:**  
`runActionGuardrail`, `runResultGuardrail` (in `src/agents/bash-tools.exec.ts`).

**Audit:**  
Audit config is **only set when the gateway process starts**. A standalone `pnpm oxcer agent` run does **not** set it, so **no JSONL is written** for this exec. To see exec events in the audit file, trigger an agent run that uses exec **via the gateway** (e.g. channel message or HTTP API that runs the agent on the gateway).  
If you run the command above anyway, guardrails still run in process; you just won’t see `guardrail.action` / `guardrail.result` in the file.

**Audit file (when gateway has set config):**  
`/tmp/oxcer-audit/actions.jsonl` (if you set `guardrails.audit.filePath` as above), or `$OPENCLAW_STATE_DIR/audit/actions.jsonl` (default `~/.openclaw/audit/actions.jsonl`).

**EventTypes to look for (when exec is triggered via gateway):**  
`guardrail.action`, `guardrail.result` with `tool: "exec"`, `decision`, `mode: "warn"`, `failClosed: false`.

---

### Scenario B – node.invoke

**There is no HTTP endpoint for node.invoke;** it is only available via the gateway **WebSocket RPC**. So there is no single curl command.

**Ways to trigger:**

1. **WebSocket client**  
   Connect to `ws://127.0.0.1:18789`, send a request with method `node.invoke` and params e.g. `{ "nodeId": "fake-node-smoke", "command": "ping", "params": {} }`. The guard runs, then the gateway responds with UNAVAILABLE (node not connected). You should still see `guardrail.node.invoke` (and possibly `guardrail.node.invoke.complete` with `success: false`) in the audit file.

2. **Existing E2E helpers**  
   See `src/gateway/server.roles-allowlist-update.e2e.test.ts` (e.g. `rpcReq(ws, "node.invoke", { nodeId, command, params })`) for a pattern.

3. **Real node**  
   Run an agent that uses exec on a **node host** (connected device); that path goes through the gateway’s `node.invoke` handler.

**Example one-off with Node (run from repo root, gateway already running):**

```bash
# Minimal WS RPC example (requires gateway running and token)
node -e "
const WebSocket = require('ws');
const ws = new WebSocket('ws://127.0.0.1:18789', { headers: { 'Authorization': 'Bearer YOUR_DEV_TOKEN' } });
ws.on('open', () => {
  ws.send(JSON.stringify({ id: '1', method: 'node.invoke', params: { nodeId: 'fake-node-smoke', command: 'ping', params: {} } }));
});
ws.on('message', (d) => { console.log(d.toString()); ws.close(); });
" 
```

Replace `YOUR_DEV_TOKEN` with your gateway token.

**Guardrail functions that run:**  
`runNodeInvokeGuard` (in `src/gateway/server-methods/nodes.helpers.ts`, called from `nodes.ts`).

**Audit functions:**  
`recordGuardrailNodeInvoke` (in warn mode, async), `recordGuardrailNodeInvokeComplete` (in `nodes.ts`).

**EventTypes in JSONL:**  
`guardrail.node.invoke` (nodeId, command, proceed, decision), `guardrail.node.invoke.complete` (nodeId, command, success, errorMessage if failed). In warn mode, the order in the file may be `guardrail.node.invoke.complete` before `guardrail.node.invoke`.

**Audit file:**  
Same as Section 1 – `/tmp/oxcer-audit/actions.jsonl` or `~/.openclaw/audit/actions.jsonl`.

---

### Scenario C – web_fetch via HTTP

**Exact curl (gateway must be running; replace PORT and TOKEN):**

```bash
curl -s -X POST "http://127.0.0.1:18789/tools/invoke" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_DEV_TOKEN" \
  -d '{"tool":"web_fetch","args":{"url":"https://example.com"}}'
```

**Guardrail functions that run:**  
None. HTTP `/tools/invoke` calls `tool.execute` directly (`src/gateway/tools-invoke-http.ts`); there is no `executeWithGuardrails` or action/result guardrail on this path.

**Audit:**  
No guardrail events. No new lines in the audit file for this call.

**EventTypes:**  
None (current behavior).

---

### Scenario D – write via HTTP

**Exact curl (gateway must be running; replace TOKEN):**

```bash
curl -s -X POST "http://127.0.0.1:18789/tools/invoke" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer YOUR_DEV_TOKEN" \
  -d '{"tool":"write","args":{"path":"/tmp/oxcer-smoke.txt","content":"smoke"}}'
```

The write tool accepts `path` (or `file_path`) and `content`. The tool may restrict writes to a workspace; if you get a policy/validation error, use a path under your agent workspace.

**Guardrail functions that run:**  
None.

**Audit:**  
No guardrail events.

**EventTypes:**  
None (current behavior).

---

## Section 3: Cheat sheet (paste into your notes)

```text
# Oxcer guardrails smoke test – cheat sheet

# 1. Config (~/.openclaw/openclaw.json or OPENCLAW_CONFIG_PATH)
#    guardrails.action.mode = "warn"
#    guardrails.result.mode = "warn"
#    guardrails.audit.enabled = true
#    guardrails.audit.devConsole = true
#    guardrails.audit.filePath = "/tmp/oxcer-audit/actions.jsonl"   # optional
#    gateway.auth.token = "YOUR_DEV_TOKEN"   # or set OPENCLAW_GATEWAY_TOKEN

# 2. Start gateway (first terminal)
NODE_ENV=development OPENCLAW_GATEWAY_TOKEN="YOUR_DEV_TOKEN" pnpm oxcer gateway run

# 3. Watch audit file (second terminal)
tail -n 20 -f /tmp/oxcer-audit/actions.jsonl
# Or if not using filePath override:
tail -n 20 -f ~/.openclaw/audit/actions.jsonl

# 4. Scenario A – Exec (guardrails run; audit only if agent runs via gateway)
NODE_ENV=development pnpm oxcer agent --message "Run the shell command: echo hello" --session-id smoke-test-exec --thinking low

# 5. Scenario B – node.invoke (WS only; use WS client or test helper; guard + audit)
#    See Section 2 Scenario B for Node one-liner or E2E pattern.

# 6. Scenario C – web_fetch (no guardrails/audit on HTTP path)
curl -s -X POST "http://127.0.0.1:18789/tools/invoke" -H "Content-Type: application/json" -H "Authorization: Bearer YOUR_DEV_TOKEN" -d '{"tool":"web_fetch","args":{"url":"https://example.com"}}'

# 7. Scenario D – write (no guardrails/audit on HTTP path)
curl -s -X POST "http://127.0.0.1:18789/tools/invoke" -H "Content-Type: application/json" -H "Authorization: Bearer YOUR_DEV_TOKEN" -d '{"tool":"write","args":{"path":"/tmp/oxcer-smoke.txt","content":"smoke"}}'
```

**Expected audit file path:**  
- With override: `/tmp/oxcer-audit/actions.jsonl`  
- Default: `~/.openclaw/audit/actions.jsonl` (or `$OPENCLAW_STATE_DIR/audit/actions.jsonl`).

**Summary:**  
- A (exec): guardrails run in agent process; audit file only if agent is run via gateway.  
- B (node.invoke): guard + audit when gateway handles node.invoke (WS).  
- C, D: no guardrails/audit on current HTTP `/tools/invoke` path.
