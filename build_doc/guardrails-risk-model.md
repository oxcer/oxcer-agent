# Oxcer guardrails risk model

## 1. Purpose

This document defines what “high-impact” and “high-risk” mean in Oxcer so that design, implementation, and policy decisions around guardrails stay aligned. It is used when designing the `GuardrailOrchestrator`, wiring tools through it, and defining config and audit behaviour. Scope is **agent tool use**: shell, file, network, messaging, browser, node, and business APIs (payments, accounts, workflows).

---

## 2. Risk categories (conceptual)

### Technical high-impact actions

- **Shell / command execution** — Host commands, processes, scripts. Examples: `sh -c "..."`, running installers, cron jobs.
- **File writes/edits** — Code, config, credentials, logs. Examples: editing `~/.ssh/authorized_keys`, writing to `/etc`, app config files.
- **Network calls** — HTTP/HTTPS, internal APIs, SaaS APIs. Examples: `web_fetch`, REST calls to admin endpoints, third-party integrations.
- **Messaging and communication** — Email, chat, notifications. Examples: sending Slack/Teams messages, email, SMS, push notifications.
- **Browser / node / device control** — Browser automation, screen/camera, SMS, location. [sysaid](https://www.sysaid.com/blog/generative-ai/agentic-ai-browsers-risk-rules)

### Business high-impact actions

- **Money movement** — Payments, refunds, payouts, pricing changes.
- **Account and access** — Role changes, account creation/deletion, permission updates.
- **Workflow state changes** — Closing tickets, approving deployments, changing contract/claim status. [bigid](https://bigid.com/blog/agentic-ai-guardrails/)

### Information and compliance risk

- **Access to sensitive or regulated data** — PII/PHI/PCI, internal secrets, trade secrets.
- **Actions with regulatory, legal, or reputational impact** — Data exfiltration, public statements, automated legal/financial decisions. [mckinsey](https://www.mckinsey.com/capabilities/risk-and-resilience/our-insights/deploying-agentic-ai-with-safety-and-security-a-playbook-for-technology-leaders)

---

## 3. How we encode risk in Oxcer

Risk is represented in code via `ProposedAction` (and related types) passed into the guardrail pipeline.

### Category

- **`category`**: `"shell" | "file" | "network" | "messaging" | "browser" | "node" | "other"`.
- Shell, file, network, messaging, browser, and node are treated as **high-impact categories** by default. [developer.nvidia](https://developer.nvidia.com/blog/practical-security-guidance-for-sandboxing-agentic-workflows-and-managing-execution-risk/)

### Resource identifiers

- **`resourceIdentifiers`**: key-value map describing *what* is being touched.
- Examples: `path`, `url`, `channelId`, `nodeId`, `accountId`, `workflowId`.
- Used for policy (allowlists/denylists), audit, and dashboards.

### Risk level and score

- **`riskLevel`**: `"low" | "medium" | "high" | "critical"`.
- Optional **`riskScore`** (numeric) for ordering and thresholds.
- Heuristics: shell on a prod node writing under `/etc/` plus an external `web_fetch` → high/critical; read-only request to a public URL → low/medium.

### Accountability

- **`sessionKey`**, **`agentId`**, **`channel`**: tie the action to a user/session/agent for accountability and audit.

### Pseudo-TypeScript examples

**Shell command (high risk):**

```ts
{
  category: "shell",
  resourceIdentifiers: { nodeId: "prod-gw-01", command: "rm -rf /tmp/cache" },
  riskLevel: "high",
  riskScore: 85,
  sessionKey: "sess_abc",
  agentId: "agent_1",
  channel: "slack:DM"
}
```

**File write to sensitive path:**

```ts
{
  category: "file",
  resourceIdentifiers: { path: "/etc/nginx/nginx.conf", operation: "write" },
  riskLevel: "critical",
  riskScore: 95,
  sessionKey: "sess_xyz",
  agentId: "agent_1"
}
```

**Network call to internal admin API:**

```ts
{
  category: "network",
  resourceIdentifiers: { url: "https://admin.internal/api/users/delete", method: "POST" },
  riskLevel: "high",
  sessionKey: "sess_abc",
  agentId: "agent_1"
}
```

---

## 4. Guardrail policy for high-impact actions

- **Central gate:** All high-impact actions **must** go through `checkAction` with a fully-populated `ProposedAction`; no bypass (exec, `node.invoke`, HTTP `/tools/invoke`, nodes CLI, or other tools).
- **Config modes:** `GuardrailsConfig` supports `off` / `warn` / `enforce` and `failClosed`. Mode determines whether we only log or actually block/approve.
- **Enforce behaviour:** When `mode = "enforce"`:
  - **`deny`** → action does not execute (no shell/file/network/messaging/node effect).
  - **`needs_human`** → action is routed to a human-approval flow (e.g. exec approvals), not executed directly. [lumenova](https://www.lumenova.ai/blog/ai-agent-guardrails-action-4-use-cases-managing-ai-risk/)
- **Result handling:** `checkResult` decisions are enforced for high-impact tools; when result is denied, outputs are redacted and a safe placeholder is returned.
- **Audit and metrics:** Every high-impact action and guardrail decision emits a structured audit entry (e.g. JSONL) and a metrics event for monitoring and anomaly detection. [arxiv](https://arxiv.org/html/2601.20727v1)
- **Fail-closed:** When guardrails are unavailable or misconfigured, `failClosed` ensures we do not execute high-impact actions by default.

---

## 5. Examples

### Example 1: Shell command on a node (harmless vs dangerous)

**Scenario A — Harmless:** `echo "hello"` on a dev node.

- **Encoding:** `category: "shell"`, `resourceIdentifiers: { nodeId: "dev-01", command: "echo \"hello\"" }`, `riskLevel: "low"`.
- **`mode = "warn"`:** Log only; action proceeds.
- **`mode = "enforce"`:** Typically allowed (low risk); no block, no approval required.
- Audit event is always emitted; can drive dashboards and baselines. [galileo](https://galileo.ai/blog/ai-agent-guardrails-framework)

**Scenario B — Dangerous:** `rm -rf /tmp` (or worse, a path that could affect system) on a prod node.

- **Encoding:** `category: "shell"`, `resourceIdentifiers: { nodeId: "prod-gw-01", command: "rm -rf /tmp" }`, `riskLevel: "high"` (or `critical` if target path is sensitive).
- **`mode = "warn"`:** Log only; action still proceeds (operator can react via alerts).
- **`mode = "enforce"`:** Policy can **deny** or **needs_human**; if deny, command never runs; if needs_human, it goes to exec-approval flow.
- Audit event records proposed action, decision, and outcome.

---

### Example 2: File write to a sensitive path

**Scenario:** Agent tries to write to `~/.openclaw/credentials/token.json`.

- **Encoding:** `category: "file"`, `resourceIdentifiers: { path: "/home/user/.openclaw/credentials/token.json", operation: "write" }`, `riskLevel: "critical"`.
- **`mode = "warn"`:** Log only; write proceeds.
- **`mode = "enforce"`:** Policy should **deny** or **needs_human**; if deny, write is blocked and caller gets a safe error/placeholder.
- Audit event captures path, risk level, and guardrail decision.

---

### Example 3: Network call to internal admin endpoint or SaaS API

**Scenario:** `POST https://admin.internal/api/roles/assign` or `POST https://api.stripe.com/v1/refunds`.

- **Encoding:** `category: "network"`, `resourceIdentifiers: { url: "https://admin.internal/api/roles/assign", method: "POST" }`, `riskLevel: "high"` (or `critical` for money/refunds).
- **`mode = "warn"`:** Log only; request proceeds.
- **`mode = "enforce"`:** Policy can **deny** or **needs_human**; refunds and role changes typically require human approval.
- Audit and metrics feed monitoring and anomaly detection.

---

All examples above generate audit events and can be used for dashboards, alerts, and compliance reviews.
