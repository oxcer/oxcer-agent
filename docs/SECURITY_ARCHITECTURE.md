# Security & Guardrails Architecture (Sprint 4)

## Philosophy

**"Agents are untrusted clients."** All high-risk operations must go through a human-in-the-loop (HITL) approval flow by default. Both the UI and AI Agent Orchestrator use the same router + policy + approval pipeline.

## Components

### 1. Security Policy Engine (`oxcer-core/src/security/policy_engine.rs`)

Lives in its own module and acts as the **final authority** for all privileged actions.

- **Entry point:** `evaluate(request: PolicyRequest) -> PolicyDecision`
- **PolicyRequest:** `caller` (UI | AGENT_ORCHESTRATOR | INTERNAL_SYSTEM), `tool_type` (FS | SHELL | AGENT | WEB | OTHER), `operation` (read | write | delete | move | exec), `target` (canonical path, command id, or resource id)
- **PolicyDecision:** `decision` (ALLOW | DENY | REQUIRE_APPROVAL), `reason_code` (stable string for logging/telemetry)

### 2. Command Router (`src-tauri/src/router.rs`)

Sits **in front of** the FS and Shell services. All Tauri commands pass through the router, which:

1. Builds a `PolicyRequest` from the incoming operation
2. Calls `policy_engine::evaluate()`
3. If **DENY** → returns error immediately
4. If **REQUIRE_APPROVAL** → stores pending request, returns `ApprovalRequired` for the frontend
5. If **ALLOW** → proceeds to FS/Shell service

### 3. HITL Approval Flow

- When policy returns `REQUIRE_APPROVAL`, the backend stores the operation in `PendingApprovalsStore` and returns a structured error with `request_id`.
- The Launcher UI detects `kind: 'approval_required'` and shows an approval modal.
- User approves or denies; frontend calls `cmd_approve_and_execute(request_id, approved)`.
- Backend retrieves the pending operation and either executes it or discards it.

### 4. Reason Codes

Stable strings for logging and telemetry:

- `FS_PATH_IN_BLOCKLIST` — Path touches .ssh, .aws, Keychains, Passwords
- `SHELL_COMMAND_BLACKLISTED` — Command in deny list
- `DESTRUCTIVE_FS_REQUIRES_APPROVAL` — Agent delete/move
- `AGENT_WRITE_REQUIRES_APPROVAL` — Agent FS write
- `AGENT_EXEC_REQUIRES_APPROVAL` — Agent shell exec

### 5. Policy Rules (Zero Trust, safe-by-default)

**Rule order of precedence:**

1. **Static deny** — blocklisted paths, blacklisted commands → DENY
2. **Risk-based** — destructive FS (delete/rename/move/chmod), high-risk tools (deploy/push/migrate) → REQUIRE_APPROVAL
3. **Caller-sensitive** — Agent stricter than UI
4. **Default-deny** — no explicit allow → DENY (`DEFAULT_DENY`)

| Rule | Condition | Result |
|------|-----------|--------|
| Static deny | Path in blocklist (~/.ssh, ~/.gnupg, ~/.aws, ~/.env*, etc.) | DENY |
| Static deny | Command in blacklist (rm, sudo, mkfs, dd, etc.) | DENY |
| Risk-based | FS delete/rename/move/chmod | REQUIRE_APPROVAL |
| Risk-based | Shell deploy/push/migrate/release | REQUIRE_APPROVAL |
| Caller | Agent + FS read | ALLOW |
| Caller | Agent + FS write/exec | REQUIRE_APPROVAL |
| Caller | UI + FS read/write, Shell exec | ALLOW |
| Default | No explicit allow | DENY |

## HITL Approval Flow

When policy returns `REQUIRE_APPROVAL`:

1. **Generate request_id** — Stable UUID.
2. **Persist approval record** — `ApprovalRequestRecord` with: request_id, caller, tool_type, operation, target, original payload (secrets redacted for display), status (PENDING | APPROVED | DENIED | EXPIRED), timestamps, actor fields.
3. **Emit event** — `security.approval.requested` with sanitized context (no secrets).
4. **Launcher UI** — Approval Modal with: description, caller type (Agent vs User), target path/command, risk hints; buttons: Allow once, Deny, View details (expands with secrets redacted).
5. **Resolution** — On Allow: mark APPROVED, resume execution. On Deny/timeout: mark DENIED/EXPIRED, return error. Configurable timeout (default 5 min) fails closed (auto-deny).

## "Agent = untrusted client" contract

**Invariant:** The Agent Orchestrator never calls FS/Shell/Web tools directly. All privileged operations MUST go through:

  Command Router → Security Policy Engine → optional HITL Approval → tool

- **Enforcement:** The Tauri launcher (`main.rs`) registers the ONLY invoke commands for FS/Shell (`cmd_fs_list_dir`, `cmd_fs_read_file`, `cmd_fs_write_file`, `cmd_shell_run`). There are no direct `fs::` or `shell::` Tauri commands.
- **Agent path:** Agents invoke these commands with `caller: "agent_orchestrator"`. The policy engine enforces stricter rules (write/delete/exec → REQUIRE_APPROVAL) for agents.
- **Tests:** `agent_destructive_operation_requires_approval` and `agent_exec_requires_approval` verify that approval is enforced.

## Policy definition & configuration

Policies are expressed as data (YAML/JSON) instead of hard-coding.

- **Schema:** `match` (caller, tool_type, operation, path_patterns, command_patterns), `action` (allow | deny | require_approval), optional `notes`, `risk_level`
- **Default policy:** `oxcer-core/policies/default.yaml` encodes path blocklist, command blacklist, risk-based rules
- **Loader:** Validates schema; invalid policy → secure default (default-deny). Fails safely.

## Test suite

### Policy engine (table-driven)

- `(caller, tool_type, operation, target) -> expected decision, reason_code`
- Covers: allow, deny, require_approval, Agent vs UI asymmetry
- `table_driven_policy_decisions` runs all cases

### Router + HITL wiring

- **DENY** → command never executes (policy returns Deny; handler short-circuits)
- **REQUIRE_APPROVAL** → pending approval record created (`PendingApprovalsStore::insert`)
- **Approved** → record retrievable via `take()`, command can execute exactly once
- **Denied/expired** → `take()` returns None (expired with TTL=0); command never executes
