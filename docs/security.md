# Security Architecture

> **Read this if** you want to understand the threat model, policy engine, and human-in-the-loop approval design. Also useful before auditing tool execution or data-handling code.

> **Preview — v0.1.0:** The security model described here reflects the design intent for this release. Some mitigations — log-file retention rotation, environment filtering completeness, policy-file hot-reload — are partially implemented and will be hardened in future versions. Do not rely on Oxcer as your sole security boundary for sensitive data in production environments.

Oxcer is **designed on a zero-trust, safe-by-default** principle: the AI agent is treated as an untrusted client. All privileged operations are intended to pass through the policy engine and, where required, receive explicit user approval before execution.

---

## Core Principles

1. **Agent = untrusted client.** The orchestrator can propose tool calls but cannot execute them unilaterally.
2. **Every tool call is policy-evaluated.** There are no side doors; all FS and shell operations go through the same router regardless of whether they originate from the UI or the agent.
3. **Hard never-send rules.** Credentials, private keys, and API tokens are blocked from reaching the LLM — not redacted, blocked.
4. **Default-deny.** An operation with no explicit allow rule is denied.

---

## Policy Engine

The policy engine (`oxcer-core/src/security/policy_engine.rs`) is the final authority for all privileged actions.

```
PolicyRequest { caller, tool_type, operation, target }
        │
        ▼
PolicyDecision: Allow | Deny | RequireApproval
```

**Rule precedence:**

| Priority | Rule | Example | Decision |
|---|---|---|---|
| 1 | Static deny — path blocklist | `~/.ssh`, `~/.aws`, `~/.env*`, Keychains | Deny |
| 1 | Static deny — command blacklist | `rm -rf`, `sudo`, `mkfs`, `dd` | Deny |
| 2 | Risk-based | Agent FS delete / rename / move | RequireApproval |
| 2 | Risk-based | Shell deploy / push / migrate | RequireApproval |
| 3 | Caller-sensitive | Agent FS read | Allow |
| 3 | Caller-sensitive | Agent FS write / exec | RequireApproval |
| 3 | Caller-sensitive | UI FS read / write, shell exec | Allow |
| 4 | Default | No explicit allow | Deny |

Policies are expressed as YAML (`config/policies/default.yaml`). Invalid policy files fall back to the secure default (default-deny) instead of crashing.

---

## Human-in-the-Loop (HITL) Approval

When the policy engine returns `RequireApproval`, the agent loop suspends and presents an approval bubble to the user. The operation is not executed until the user explicitly approves or cancels it.

**Swift side flow:**

1. `AgentRunner` detects a tool intent in `approvalRequiredKinds` (all FS and shell operations).
2. It calls `onApprovalNeeded(requestId, humanReadableSummary)`, which suspends on a `CheckedContinuation<Bool, Never>`.
3. `ConversationSession.pendingApproval` is set; the `ApprovalBubble` appears in the UI.
4. The user presses **Approve** or **Cancel**.
5. The continuation resumes with the user's decision; the loop continues or feeds back an error result.

If the agent task is cancelled (Stop button) while an approval is pending, the continuation is resumed with `false` and the pending approval is cleared before task cancellation.

**Approval-required tool kinds:**

```
fs_list_dir  fs_read_file  fs_write_file  fs_delete  fs_rename  fs_move  shell_run
```

---

## Data Loss Prevention (DLP)

Before any content is sent to the LLM, it passes through two layers:

### 1. Data Sensitivity Classifier

14 regex rules across High and Medium sensitivity levels:

| Level | Pattern IDs | Action |
|---|---|---|
| **High** | AWS keys, API keys, JWTs, PEM private keys, SSH key paths, passwords in env/URLs | Never send to LLM (`NeverSendToLlm`) |
| **Medium** | Keychain paths, IPv4 addresses, long Base64 blobs, `Authorization: Bearer` | Scrub and replace with `[REDACTED: kind]` |
| **Threshold** | ≥ 50% of payload redacted | Block entire payload (`TooMuchSensitiveData`) |

### 2. Prompt Scrubber

`scrub_for_llm_call(raw_payload, options)` applies the classifier to the complete payload (task + file snippets + shell outputs + metadata) and returns either a scrubbed string ready to send or an `Err` that aborts the LLM call.

No code path should send unscrubbed content to any provider. The scrubber is the last line of defence before the LLM is invoked.

### 3. Environment Filtering

Child processes (shell commands) receive a filtered environment. High-risk keys — `AWS_*`, `GITHUB_*`, `OPENAI_API_KEY`, `*_SECRET`, `*_PASSWORD` — are removed before the child process starts.

---

## Logging and Observability

Agent logs are written to `~/Library/Application Support/Oxcer/logs/`:

- `{session_id}.jsonl` — per-session event trace (only scrubbed payloads are stored; raw secrets are never written).
- `telemetry.jsonl` — rolling log of component events (router, security, plugin load).
- `scrubbing.log` — one JSON record per scrubbing operation: original length, redacted length, matched kinds, decision.

Retention: 30 days or 10 MB per file, then rotate (drop oldest).

---

## No Batching

The orchestrator never batches tool calls into an opaque sequence. Each tool intent is evaluated and approved individually. There is no mechanism for the agent to pre-approve a sequence of destructive operations in a single step.
