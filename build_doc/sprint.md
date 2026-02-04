# Oxcer sprint log

High-level, guardrails-focused log for future retros. Oxcer is an enterprise fork of OpenClaw with a **guardrails-first** philosophy for safety-critical agent systems.

---

## Sprint 1

**Sprint goal:** Align on guardrails-first philosophy and design direction; produce a design-review prompt for Cursor.

- Clarified Oxcer’s goals and the “Guardrails first” philosophy.
- Agreed on design direction: `GuardrailOrchestrator`, exec approvals, pre/post hooks.
- Wrote a Cursor design-review prompt that:
  - Points to specific artifacts (architecture docs, orchestrator, gateway, exec-related code).
  - Asks for alignment with philosophy, scalability (many tools/nodes/engines), concrete improvements, and a guardrail-centric checklist.
- **TODOs carried over:** Run design review; turn checklist into actionable gaps.

---

## Sprint 2

**Sprint goal:** Code-level guardrails gap analysis; turn philosophy into a concrete checklist and coverage assessment.

- Diagnosis: current structure matches philosophy but is not yet a logical central gate.
- Major gaps and bypass paths:
  - `checkPrompt` / `checkAction` / `checkResult` results are **not enforced** (decisions ignored).
  - Non-exec tools (`write`, `web_fetch`, `nodes`, `message`, etc.) do not use guardrails.
  - `node.invoke`, HTTP `/tools/invoke`, and nodes CLI can bypass guardrails.
  - `guardrailOrchestrator` is optional, so new entry points can omit it.
- Created a 10-item guardrail-centric checklist; assessed coverage: ~1/10 fully met, ~2/10 partially met, rest not met.
- **TODOs carried over:** Enforce check decisions; centralize node/gateway invocations; make orchestrator mandatory where appropriate.

---

## Sprint 3

**Sprint goal:** Guardrail hardening roadmap; preserve philosophy, make high-risk paths fail-closed first.

- Strategy: phased plan—preserve philosophy, harden high-risk paths before expanding coverage.
- Phase 1: Enforce `checkAction` / `checkResult` in the exec tool (handle `deny` / `needs_human`; add `guardrails.mode` / `failClosed` config).
- Phase 2: Add guardrails to gateway `node.invoke` handler so all node executions go through a central gate.
- Phase 3: Introduce `executeWithGuardrails` helper; migrate exec to it first.
- Follow-ups: inject guardrails into HTTP `/tools/invoke`; wrap high-impact tools (web/file/message/nodes); enrich `ProposedAction` metadata; audit logging; Composite `GuardrailOrchestrator`.
- **TODOs carried over:** Implement Phase 1 (exec enforcement + config); then Phase 2 (gateway `node.invoke`); then helper and tool wrapping.

---

## Sprint 4

**Sprint goal:** Exec + node.invoke hardening — make shell/terminal and node execution go through enforced guardrails.

- **Guardrails config**
  - Added `config/guardrails.ts`: `GuardrailMode` (`off` | `warn` | `enforce`), `GuardrailsConfig` (action.mode, result.mode, failClosed), `resolveGuardrailsConfig(cfg)`.
  - Wired into `OpenClawConfig` (types.openclaw), zod schema, and schema UI (FIELD_LABELS / FIELD_DESCRIPTIONS). Defaults: actionMode/resultMode `off`, failClosed `false`.

- **Exec tool enforcement** (`src/agents/bash-tools.exec.ts`)
  - `ExecToolDefaults` extended with `guardrailsConfig?: ResolvedGuardrailsConfig`; `guardrailsConfig` resolved in `pi-tools` from `options?.config` and passed into `createExecTool`.
  - Helpers: `runActionGuardrail(gConfig, orchestrator, action)` (returns `{ forceApproval }` or throws on deny; on error uses failClosed) and `runResultGuardrail(...)` (returns true if result should be redacted).
  - **After checkAction:** If decision `deny` → throw (no exec). If `needs_human` → force approval flow (`requiresAsk = requiresAsk || forceApproval`). Applied once per path (node host, gateway with/without approval, gateway direct).
  - **After checkResult:** If decision `deny` (enforce) or error + failClosed → return redacted result (`[Result redacted by guardrails]`). Applied for node-host direct result and gateway direct result.
  - Removed best-effort `void guardrailOrchestrator?.checkAction/checkResult` calls; all pre-exec checks now go through `runActionGuardrail` with mode semantics.

- **node.invoke gating** (`src/gateway/server-methods/nodes.ts`, `nodes.helpers.ts`)
  - Shared helper `runNodeInvokeGuard(context, cfg, { nodeId, command, params }, respond)`: when `actionMode === "enforce"` runs `guardrailOrchestrator.checkAction`; on deny/needs_human responds with error and returns false; on guard error and failClosed responds and returns false; when warn logs and returns true.
  - `node.invoke` handler uses `runNodeInvokeGuard` before `context.nodeRegistry.invoke`.
  - All internal gateway callers of `context.nodeRegistry.invoke` now run the guard: **exec-approvals** (`exec.approvals.node.get`, `exec.approvals.node.set`) and **browser** (`browser.proxy` via node) call `runNodeInvokeGuard` before invoking.

- **Paths audited**
  - CLI/node tool/canvas tool: already call gateway RPC `node.invoke` → guarded in handler.
  - Gateway-internal: `node.invoke` handler, `exec-approvals`, and `browser` proxy path now use `runNodeInvokeGuard`; no remaining direct `nodeRegistry.invoke` bypass.

- **Result:** Shell execution and node execution always pass through guardrails when config is set to enforce; deny/needs_human and result-deny are enforced; warn mode logs only; failClosed blocks on guard errors.

- **Pre–Sprint 5 prep (cleanup)**
  - **Generic guardrail helpers:** Moved `runActionGuardrail`, `runResultGuardrail`, and `REDACTED_RESULT_TEXT` from `src/agents/bash-tools.exec.ts` into `src/guardrails/action-result.ts`. Signatures are generic (ProposedAction, ResolvedGuardrailsConfig, GuardrailOrchestrator); exec-specific “Exec denied by guardrails” message is applied at the exec call site via a thin wrapper that catches and rethrows.
  - **runNodeInvokeGuard result shape:** Return type changed from `boolean` to `NodeInvokeGuardResult`: `{ proceed: boolean; forceApproval?: boolean; decision?: PolicyDecision }`. Callers (nodes.ts, exec-approvals.ts, browser.ts) now use `guardResult.proceed` to decide whether to invoke; `forceApproval` and `decision` are set for needs_human so node-level approval flows can be wired later. TODOs added at call sites to thread these for Sprint 5.
  - **Audit log hook points:** Added `src/guardrails/audit.ts` with no-op placeholders (dev console.debug only): `recordGuardrailActionDecision(entry)`, `recordGuardrailResultDecision(entry)`, `recordGuardrailNodeInvoke(entry)`, `recordGuardrailNodeInvokeComplete(entry)`. Entry types include actionId, tool, decision, mode, failClosed, timestamps, sessionKey, agentId. Hooks are called from `runActionGuardrail`/`runResultGuardrail` in action-result.ts and from `runNodeInvokeGuard` and after node.invoke in nodes.ts.

---

## Sprint 6 – Audit trail & monitoring hooks

**Sprint goal:** Implement a production-ready, append-only audit log for guardrail decisions and high-impact tool actions, plus minimal monitoring hooks, without over-engineering.

- **JSONL audit store** (`src/guardrails/audit.ts`): Each `recordGuardrail*` writes one JSON object per line to an append-only file (default `$OPENCLAW_STATE_DIR/audit/actions.jsonl`). Events include `eventId` (uuid), `eventType` (`guardrail.action`, `guardrail.result`, `guardrail.node.invoke`, `guardrail.node.invoke.complete`), `timestamp` (RFC3339), and existing fields (actionId, tool, args summary, decision, mode, failClosed, risk, sessionKey, agentId, nodeId, proceed/forceApproval, redacted, errorMessage). Append uses `fs.appendFileSync` with flag `a`; write failures are caught and logged (debug) so audit log issues never crash the process. TODO comment added for future maxSizeBytes/rotation.
- **Configurable audit** (`config/guardrails.ts`): `GuardrailsAuditConfig` with `enabled`, `filePath`, `devConsole`. `resolveGuardrailsAuditConfig(cfg, stateDir)` returns resolved path and flags. Gateway startup calls `setGuardrailAuditConfig(resolveGuardrailsAuditConfig(cfgAtStart, STATE_DIR))` so `recordGuardrail*` read config via `getGuardrailAuditConfig()`. In dev, `audit.enabled: true` and `devConsole: true` allow file + console; in prod, file-only. Schema and FIELD_LABELS/FIELD_DESCRIPTIONS extended for `guardrails.audit.*`.
- **Wiring of audit calls**: Action decisions (including when `checkAction` throws) and result decisions in `action-result.ts`; node.invoke decision and invoke-complete in `nodes.helpers.ts` and `nodes.ts`. `errorMessage` added for node invoke failures and for action/result error paths.
- **Monitoring hook** (`src/guardrails/metrics.ts`): `emitGuardrailMetric(event)` is called from each `recordGuardrail*` after a successful JSONL write. No-op in this sprint (optional console.debug in dev); signature and call sites allow plugging StatsD/Prometheus later. Derived fields for alerting: severity bucket from `riskLevel`/decision (low/medium/high/critical).
- **Tests**: `src/guardrails/audit.test.ts` – with audit enabled, one `recordGuardrailActionDecision` call produces one JSONL line with expected `eventType`, `tool`, `decision`; with audit disabled, no file is written.
- Manual guardrails smoke-test plan defined (exec, node.invoke, web_fetch, write) using warn-mode guardrails and JSONL audit inspection; see `build_doc/guardrails-smoke-test.md`. Intended as the seed for a future automated regression suite.
- The smoke-test doc was validated against the current codebase; commands, expected audit `eventType`s, and key fields were checked. Necessary updates to commands and expectations were made so the doc is current as of 2025-02-03.

**Next steps**

- Forward audit logs to a centralized logging/observability stack (e.g. log shipper or API).
- Add dashboards/queries for denies and needs_human by tool/risk.
- Consider integrity features (e.g. hash chaining) if compliance requires it.

---

## Sprint 7 – NeMo Guardrails integration

**Sprint goal:** Implement a NeMo-backed guardrail engine so that the gateway's `checkAction` can call out to a NeMo Guardrails service and interpret its decision as `allow` | `deny` | `needs_human`.

- **NeMo Guardrails orchestrator** (`src/guardrails/nemo-orchestrator.ts`): Implemented `NemoGuardrailOrchestrator` class that implements `GuardrailOrchestrator` interface. Calls a NeMo Guardrails REST endpoint (`POST {endpoint}/guardrails/check_action` and `/guardrails/check_result`) with action/result data and maps NeMo responses (`allow`, `deny`/`blocked`, `needs_human`/`uncertain`/`needs_review`) to `PolicyDecision`. Includes error handling, dev-mode logging of raw NeMo results, and optional API key authentication support.

- **Configuration** (`src/config/guardrails.ts`): Added `TextGuardrailsConfig` type with `engine` selector (`"noop"` | `"nemo"`) and `nemo` config (`endpoint`, optional `apiKey`). Extended `GuardrailsConfig` with optional `textguardrails` field. Updated zod schema (`src/config/zod-schema.ts`) to validate the new config structure.

- **Orchestrator factory** (`src/guardrails/orchestrator.ts`): Added `createGuardrailOrchestrator(config)` factory function that selects between `NoopGuardrailOrchestrator` (default) and `NemoGuardrailOrchestrator` based on `config.guardrails.textguardrails.engine`. Default behavior: if `engine` is absent or `"noop"`, uses no-op (existing behavior).

- **Gateway wiring** (`src/gateway/server.impl.ts`): Updated gateway startup to use `createGuardrailOrchestrator(cfgAtStart)` instead of hardcoded `NoopGuardrailOrchestrator`. When `engine === "nemo"`, gateway will call NeMo service for all `checkAction`/`checkResult` calls.

- **NeMo service (external, documented)**: Sprint 7 expects a NeMo Guardrails instance running locally, exposing a REST endpoint. Example command (to be fleshed out later): `python -m nemoguardrails.server --config rails/ --port 8080`. Config example added to sprint notes showing how to enable NeMo engine in Oxcer config.

**End state:** Gateway can switch between noop and NeMo guardrails engines via config. When `guardrails.textguardrails.engine = "nemo"`, all action/result checks flow through the NeMo service. Existing guardrails behavior (no external calls) remains the default when `engine` is `"noop"` or absent.

**Next steps**

- Set up and run a local NeMo Guardrails service instance.
- Refine NeMo API contract (request/response format) to match real NeMo Guardrails API.
- Add integration tests for NeMo orchestrator (mock NeMo service).
- Consider gRPC HTTP endpoint support if NeMo exposes gRPC.


---

## Sprint 8 – NeMo Guardrails evaluation & operational hardening

**Sprint goal:** Make the NeMo Guardrails integration robust and evaluable through better observability, evaluation harness, and operational documentation.

- **Enhanced logging & observability** (`src/guardrails/nemo-orchestrator.ts`): Added structured logging around NeMo calls with request summaries (action type, redacted args), decision outcomes (`allow`/`deny`/`needs_human`), reasons, and duration metrics. Logs respect `NODE_ENV=development` for verbose output in dev; non-allow decisions are always logged. Captures NeMo's own log fields (`traceId`, `step`) when available in responses for correlation with NeMo service logs. Logging uses consistent `[NeMo Guardrails]` prefix for easy filtering.

- **Evaluation harness** (`src/guardrails/nemo-eval-runner.ts`): Implemented a batch evaluation script that reads test cases from JSONL files, constructs `ActionProposal`/`Result` objects, calls the guardrails orchestrator (same path as gateway), and collects decisions with reasons. Prints concise summaries to stdout and optionally writes JSONL report files. Supports both action and result test cases. Sample dataset added at `test/guardrails/nemo-eval-dataset.jsonl` with canonical cases (high-risk exec commands, file writes, result checks).

- **npm script** (`package.json`): Added `pnpm guardrails:nemo:eval` command to run the evaluation harness. Supports `--dataset <path>` and `--output <path>` flags for custom datasets and output files.

- **Documentation** (`sprint.md`): Added Sprint 8 section documenting observability improvements, evaluation harness usage, and operational guidance for running NeMo + Oxcer together.

**How to run**

1. **Start NeMo Guardrails service** (external, example command):
   ```bash
   # Example – run NeMo Guardrails locally (to be refined separately)
   python -m nemoguardrails.server --config rails/ --port 8080
   ```

2. **Enable NeMo in Oxcer config** (`~/.openclaw/openclaw.json` or `OPENCLAW_CONFIG_PATH`):
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
       },
       "textguardrails": {
         "engine": "nemo",
         "nemo": {
           "endpoint": "http://localhost:8080"
         }
       }
     }
   }
   ```

3. **Run evaluation harness**:
   ```bash
   # Use default dataset (test/guardrails/nemo-eval-dataset.jsonl)
   pnpm guardrails:nemo:eval

   # Use custom dataset and output file
   pnpm guardrails:nemo:eval --dataset /path/to/custom.jsonl --output /path/to/results.jsonl
   ```

4. **Interpret logs**: With `NODE_ENV=development`, NeMo Guardrails logs include structured data:
   - Request summaries: `[NeMo Guardrails] checkAction: exec` with tool, args, risk level
   - Decision logs: `[NeMo Guardrails] checkAction decision: DENY` with reason, duration, traceId
   - Error logs: `[NeMo Guardrails] checkAction error:` with tool, status, error details

**End state:** NeMo Guardrails integration is observable (structured logs with decision reasons and NeMo trace IDs), evaluable (batch test harness with sample dataset), and operationally clear (documented how to run NeMo + Oxcer together). Sprint 7 provided basic integration; Sprint 8 adds visibility and evaluability.

**Next steps**

- Set up a real NeMo Guardrails service instance with policy rails.
- Refine NeMo API contract to match actual NeMo Guardrails REST/gRPC endpoints.
- Add integration tests with mocked NeMo service responses.
- Consider correlation IDs between Oxcer audit logs and NeMo service logs.
- Expand evaluation dataset with domain-specific test cases.

---

## Sprint 9 – Guardrails UX & human-in-the-loop

**Sprint goal:** Expose guardrails decisions and "needs_human" flows through a simple web UI so non-engineers can see and act on what NeMo/guardrails are doing.

- **Guardrails events store** (`src/guardrails/events-store.ts`): Implemented in-memory store for guardrail events with support for `pending_review`, `resolved`, and `auto_resolved` statuses. Stores events with metadata (id, timestamp, type, decision, reason, summary, tool, args, riskLevel, sessionKey, agentId). Provides methods to add events, list events (with optional status filter), get single event, submit human reviews, and get pending review queue. Max 1000 events kept in memory (oldest trimmed when exceeded). Not durable across restarts (Sprint 9 scope).

- **Gateway API methods** (`src/gateway/server-methods/guardrails.ts`): Added four WebSocket methods:
  - `guardrails.events.list`: List recent guardrail events (optional status/limit filters).
  - `guardrails.events.get`: Get single event by ID.
  - `guardrails.events.review`: Submit human review (approve/reject) for a needs_human event.
  - `guardrails.reviews.pending`: Get pending review queue.
  Methods registered in `server-methods.ts` and added to `server-methods-list.ts`.

- **Events emission** (`src/guardrails/action-result.ts`): Wired `runActionGuardrail` and `runResultGuardrail` to emit events to the store when guardrails are enabled (mode !== "off"). Events automatically get `pending_review` status when decision is `needs_human`, otherwise `resolved`.

- **Human review audit logging** (`src/guardrails/audit.ts`): Added `recordGuardrailHumanReview` function that logs human review decisions to the audit JSONL file. Includes `eventType: "guardrail.human.review"`, original decision, human decision (approve/reject), reviewer ID, optional note, and `label` field (`human_approved` or `human_rejected`) for future analysis/training scripts.

- **UI components** (`ui/src/ui/views/guardrails.ts`, `ui/src/ui/controllers/guardrails.ts`): Created skeleton UI components:
  - Controller: Functions to call gateway methods (`listGuardrailEvents`, `getGuardrailEvent`, `submitGuardrailReview`, `getPendingReviews`).
  - View: Lit-based component showing events table, pending review badge, review card for selected needs_human items, approve/reject buttons.
  - Note: Full integration requires wiring into `app-view-state.ts` and `app.ts` (state management, refresh polling, navigation).

**How to use**

1. **Start gateway** (already configured from previous sprints):
   ```bash
   pnpm oxcer gateway run
   ```

2. **Enable guardrails in config** (if not already done):
   ```json
   {
     "guardrails": {
       "action": { "mode": "warn" },
       "result": { "mode": "warn" },
       "textguardrails": {
         "engine": "nemo",
         "nemo": { "endpoint": "http://localhost:8080" }
       }
     }
   }
   ```

3. **Open Control UI**: Navigate to `http://127.0.0.1:18789/` (or your gateway URL).

4. **Access Guardrails panel**: Navigate to Guardrails view (requires UI integration - see note below).

5. **Review needs_human items**: When a guardrail decision is `needs_human`, it appears in the pending review queue. Click "Review" to see details and approve/reject.

6. **View audit log**: Human reviews are logged to `guardrails.audit.filePath` (default: `$OPENCLAW_STATE_DIR/audit/actions.jsonl`) with `eventType: "guardrail.human.review"` and `label` field for filtering.

**Example flow**

1. Agent attempts high-risk action (e.g., `exec` with `rm -rf /`).
2. NeMo Guardrails returns `needs_human` decision.
3. Event stored with `status: "pending_review"`.
4. Operator sees event in Guardrails UI pending queue.
5. Operator clicks "Review", sees action details (tool, args, reason).
6. Operator clicks "Approve" or "Reject".
7. Review logged to audit file with `label: "human_approved"` or `"human_rejected"`.
8. Event status updated to `resolved`.

**Out of scope for Sprint 9**

- Full RBAC (reviewer ID is placeholder "local-operator").
- Long-term storage (events are in-memory, cleared on restart).
- Complex analytics/dashboards (basic list view only).
- Real-time WebSocket events (UI polls via `guardrails.events.list`).
- Full UI integration (skeleton components created; requires wiring into app state/navigation).

**End state:** Guardrails decisions are visible in a web UI, needs_human items can be reviewed and approved/rejected by operators, and human decisions are logged to the audit trail with labels for future analysis. Sprint 7 provided NeMo integration; Sprint 8 added observability; Sprint 9 adds human-in-the-loop UX.

**Next steps**

- Wire guardrails UI components into app-view-state.ts and app.ts.
- Add navigation route for Guardrails panel.
- Implement polling/refresh for events list.
- Add WebSocket events for real-time updates (`guardrail.event.created`, `guardrail.event.reviewed`).
- Consider persistence layer for events (file-backed or database).
- Add RBAC for reviewer identification.
- Build analytics views (decision trends, reviewer activity).

---

## Sprint 10 – Guardrails data pipeline & tuning prep

**Sprint goal:** Turn JSONL audit logs (guardrail decisions + HITL labels) into reusable datasets and basic reports so we can identify weak spots and prep for future tuning (NeMo policies, RLHF, etc.).

- Normalize guardrails + human review logs into a reusable dataset (JSONL now; Parquet later).
- Add scripts to compute basic safety metrics (block rate, needs_human volume, agreement with humans).
- Tag and group failure cases for future NeMo/guardrail tuning.
- Document how to export and analyze guardrails data from a local run.
- **Out of scope:** actual RLHF training, NeMo retraining, or complex analytics dashboards (planned for later sprints).

### Folder structure

- `data/guardrails/raw/` – raw JSONL audit logs (copies/symlinks; optional).
- `data/guardrails/normalized/` – normalized datasets exported from audit logs.
- `data/guardrails/reports/` – reports/metrics (JSON; optional).

### How to run

1. Run the gateway + guardrails + HITL for a while to collect logs.
2. Export normalized data:

```bash
pnpm guardrails:data:export
```

3. Generate a basic report:

```bash
pnpm guardrails:data:report
```

4. Inspect:
   - `data/guardrails/normalized/`
   - `data/guardrails/reports/`

---

## Sprint 11 – Guardrails policy tuning loop

**Goal:** Use collected guardrails + HITL data to systematically improve NeMo Guardrails policies (config-level tuning, not model retraining).

- Add a `guardrails:policy:suggestions` tool to identify high-FP/FN and high-needs_human categories.
- Generate a NeMo Guardrails tuning plan (Markdown/JSON) from these suggestions.
- Implement a before/after evaluation script to compare two Guardrails config profiles on the same dataset.
- Document the tuning workflow in sprint.md (“export → suggestions → edit NeMo config → compare → deploy”).
- Out of scope: RLHF/PPO training of LLM weights (considered in later sprints).

### How to use

1. Collect data as in Sprint 10:

```bash
pnpm guardrails:data:export
pnpm guardrails:data:report
```

2. Identify tuning targets:

```bash
pnpm guardrails:policy:suggestions
```

3. Generate a NeMo tuning plan:

```bash
pnpm guardrails:nemo:tuning-plan
```

4. Apply changes to NeMo Guardrails config (rails/prompts) based on the plan.

5. Compare policies before and after:

```bash
pnpm guardrails:policy:compare --configA path/to/old-config.json --configB path/to/new-config.json --dataset data/guardrails/normalized/...
```

---

## Sprint 12 – Oxcer local app (v0.1)

**Goal:** Provide a “one-command / one-app” local experience so non-developers can start the Oxcer gateway + Guardrails UI without dealing with terminal + browser setup.

- Add a launcher script (e.g. `pnpm oxcer:app`) that starts the gateway and opens the Control UI automatically.
- Build a minimal desktop app PoC (Electron) on macOS that spawns the gateway and loads the UI in an embedded webview.
- Normalize runtime config/log/audit directory layout under an app-style root (e.g. `~/.oxcer/`).
- Document the difference between “Dev mode” and “App mode” entrypoints.

**Out of scope (for later sprints):**

- Full installers (.dmg/.pkg), auto-updates, and cross-platform packaging.
- Large-scale docs/branding refresh (planned as a separate Docs & Branding sprint).

### How to use (Sprint 12)

- Dev mode (terminal + browser):

```bash
pnpm oxcer gateway run
# then open http://127.0.0.1:18789/__openclaw__/canvas/
```

- App mode (launcher):

```bash
pnpm oxcer:app
# starts the gateway (OXCER_MODE=app) and opens the Control UI automatically
```

- Desktop app PoC (macOS):

```bash
pnpm oxcer:desktop:dev
# launches the Electron window that spawns the gateway and loads the UI
```

- Runtime data layout (app mode):
  - Config: `~/.oxcer/config/`
  - Logs: `~/.oxcer/logs/`
  - Audit: `~/.oxcer/audit/`
  - Dev datasets: `data/guardrails/**`

---

## Sprint 13 – Multi-session / chat room UX

**Goal:** Introduce a first-class session/chat concept so users can manage multiple conversations (and their guardrails history) side by side.

- Define a session model (sessionKey-based chat sessions) in the gateway.
- Add a sidebar/tab UI for multiple sessions (new chat, recent chats, favorites).
- Show a guardrails events/review timeline per session.
- Allow per-session model/profile selection (e.g., “safe mode”, “experimental mode”).

**Out of scope:**

- Long-term persistence/backup of all history (beyond basic local storage).
- Complex access control per session (RBAC is handled in later sprints).

---

## Sprint 14 – Editing / summary / visualization UX

**Goal:** For each chat/session, provide an inspectable, human-friendly view of what the agent planned and what it actually did, with risk/failure highlights.

- Per-session “actions taken in this run” summary view.
- Risk level / failure category highlighting (colors/badges).
- Pre-execution summary screen that surfaces what Guardrails is worried about before running.
- Post-execution report that summarizes actual actions, files, and sites touched.

**Out of scope:**

- Full, persistent Run/Experiment tracking (beyond current session scope).
- Complex analytics dashboards or time series across many sessions.
