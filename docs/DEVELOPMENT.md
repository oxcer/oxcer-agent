# Oxcer development workflow

## Project structure

- **`oxcer-core`** — Pure Rust library: FS Service, Shell Service, Security Policy, Logging. No Tauri dependency. All core logic and unit tests live here; test with `cargo test -p oxcer-core` (or `cd oxcer-core && cargo test`).
- **Core modules refactor guidelines:** See [docs/ARCHITECTURE_CORE.md](ARCHITECTURE_CORE.md) for policy_engine, data_sensitivity, and orchestrator design principles.
- **`apps/desktop-tauri/src-tauri`** — Tauri backend: initializes Tauri context/capabilities and exposes commands (FS, Shell, Security, Settings) that delegate into `oxcer-core`. **Backend only** — no WebView UI. The primary UI is a native macOS Swift app that communicates via IPC/sidecar.
- **Tauri frontend** — Minimal placeholder built to `apps/desktop-tauri/dist/` (no scripts). Legacy WebView UI archived in `reference/legacy_ui/`.
- **`apps/windows-launcher/`** — Planned Windows-native launcher using WinUI 3 + Windows App SDK (C#). Intentionally not part of the build yet; will be wired into the toolchain when implementation starts.

## Default workflow

- **Core logic:** Change code in `oxcer-core`; run tests there first.
  - `cd oxcer-core`
  - `cargo test`
  - No Tauri or window; tests are pure Rust.

- **Tauri app sanity:** After core tests are green, ensure the launcher still builds and runs.
  - From repo root: `pnpm tauri dev` (or run workspace tests; see below).
  - The backend runs with a minimal hidden window; the Swift app will provide the primary UI.

- **Workspace tests:** From repo root, run all packages’ tests:
  - `cargo check --workspace`
  - `cargo test --workspace --features test` (the `test` feature enables Tauri’s mock context for the `oxcer` binary tests).

- **Routine:** Modify core -> run `cargo test` in `oxcer-core` -> then run `pnpm tauri dev` when you need to exercise the backend.

## Tauri context and `cargo test`

- **Do not** call `tauri::generate_context!()` directly from code that is compiled when running `cargo test`.
- `generate_context!()` depends on `OUT_DIR` from the Tauri build script and fails when the test binary is built.
- **Fix:** Use the `app_context()` helper in `apps/desktop-tauri/src-tauri/src/main.rs`:
  - `#[cfg(test)]`: returns `tauri::test::mock_context(tauri::test::noop_assets())` so the test binary compiles.
  - `#[cfg(not(test))]`: returns `tauri::generate_context!()` for real runs.
- In `main()` we call `.run(app_context())` instead of `.run(tauri::generate_context!())`.

## Security and capabilities

- **FS and Shell** go through our own services and **Security Policy** (path blocklist, command deny list); they are not raw filesystem/shell access.
- Oxcer runs with a hidden window; the Swift app is the primary UI and invokes backend commands via IPC/sidecar.
- Capabilities are configured in `tauri.conf.json` under `app.security.capabilities`; the main window uses the `"main"` capability.
- For new features, add tests in `oxcer-core` (in the relevant module: `fs.rs` or `shell.rs`) first; keep Tauri-specific wiring in `apps/desktop-tauri/src-tauri/src/main.rs` and validate with `pnpm tauri dev` when needed.

## Testing and validation (Sprint 6)

- **Unit tests (oxcer-core):**
  - **semantic_router::route_task:** Various prompts/contexts assert category + strategy (simple_qa, tools_heavy, delete file X + high-risk, planning, cheap vs expensive by length, tools_only prefer).
  - **Orchestrator:** Simple QA path (LlmGenerate step), tools-only list/delete (FsListDir/FsDelete intents), state machine (agent_step -> NeedTool then Ok -> Complete).
  - **prompt_sanitizer:** Sensitive paths (ssh, aws, .pem), non-sensitive paths, redaction of JWTs and API-key prefixes, sensitive file placeholder in sanitize_for_llm.
- **Integration tests (oxcer-core/tests/agent_flow_integration.rs):**
  - **Agent tools-only delete:** "delete file X" -> route_task -> ToolsHeavy + ToolsOnly + high-risk; start_session -> FsDelete intent; policy evaluate(AgentOrchestrator, Fs, Delete) -> RequireApproval.
  - **Cheap vs expensive routing:** Simple QA -> CheapModel; long multi-step "plan" task -> ExpensiveModel.

Run: `cargo test -p oxcer-core` (unit + integration).

## Sprint 7 testing strategy (data sensitivity & scrubbing)

- **Classifier (data_sensitivity):** Unit tests for High (AWS keys, API keys, PEM, `.ssh/id_rsa` paths), Medium (IPs, ports, long base64), Low (normal code, workspace path normalization). See `oxcer-core/src/data_sensitivity.rs` `#[cfg(test)]`.
- **Scrubber:** Placeholders inserted and surrounding text preserved; `redacted_length / original_length` threshold triggers block (ScrubbedAndBlocked or NeverSendToLlm). See `oxcer-core/src/prompt_sanitizer.rs` tests.
- **Env filtering:** Synthetic env map -> high-risk keys absent in `filter_env_for_child` output. See `oxcer-core/src/env_filter.rs` tests.
- **Policy data_sensitivity:** Rules with `data_sensitivity: { max_level, require_approval_if }`; high-sensitivity payload -> Denied or ApprovalRequired per config. See `oxcer-core/src/security/policy_config.rs` and `oxcer-core/tests/sprint7_integration.rs`.
- **Integration (sprint7_integration.rs):** Sensitive file content -> scrubbed payload has `[REDACTED: ...]`, no raw secret; too much sensitive data -> scrubber returns Err; policy max_level Medium + High content -> Denied.

**Implementation notes:** Classifier uses regex + `OnceLock` pre-compiled patterns for speed. Policy rules are in YAML (`policies/default.yaml`); data_sensitivity rules can be extended there. Scrubbing is in the core path: all LLM calls go through `scrub_for_llm_call` / `scrub_for_llm_call_audit` (e.g. via `cmd_scrub_payload_for_llm`); no per-provider bypass.

**Data sensitivity rules reference** (see `oxcer-core/src/data_sensitivity.rs`):

| Rule ID | Level | What it detects |
|---------|-------|-----------------|
| `aws_access_key` | High | AWS access key ID (AKIA + 16 alphanumeric) |
| `aws_secret_key_like` | High | `aws_secret_access_key` or `aws_access_key_id` env-style assignment with value |
| `jwt` | High | JWT / OAuth tokens (eyJ... base64url) |
| `pem_block` | High | Full PEM private key block (BEGIN … END) |
| `pem_header` | High | PEM private key header only (truncated key) |
| `ssh_key_path` | High | File paths containing id_rsa, id_ed25519, or id_ecdsa (with optional .pub) |
| `password_equals` | High | PASSWORD=, DB_PASSWORD=, API_SECRET= etc. with value |
| `pass_in_url` | High | Password in URL (user:pass@host) or pass= in query string |
| `env_secret_pass` | High | *SECRET= or *PASSWORD= env vars with value |
| `api_key_secret_val` | High | OPENAI_API_KEY=, GITHUB_TOKEN=, api_key=, etc. with value (16+ chars) |
| `keychain_path` | Medium | Keychain paths (~/Library/Keychains, .keychain, KeePass, 1Password) |
| `ip_port` | Medium | IPv4 address with optional port (e.g. 192.168.1.1:8080) |
| `base64_long` | Medium | Long base64 blob (128+ chars, word-boundary) |
| `auth_bearer` | Medium | Authorization: Bearer &lt;token&gt; header |

**Config-driven data sensitivity rules (skeleton)**

The loader structure is in `oxcer-core/src/data_sensitivity_config.rs`. Rules remain hardcoded in `data_sensitivity::RULES`; the config module provides `load_rules_from_yaml(yaml: &str)` for future migration.

Sample YAML format:

```yaml
# config/data_sensitivity_rules.yaml (example; full migration not yet done)
version: 1
rules:
  - id: aws_access_key
    level: high
    never_send: true
    pattern: "AKIA[0-9A-Z]{16}"
    description: "AWS access key ID"
  - id: ip_port
    level: medium
    never_send: false
    pattern: '\b(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})(:\d{1,5})?\b'
    description: "IPv4 with optional port"
```

- **Loader:** `load_rules_from_yaml(yaml)` -> `Vec<DataSensitivityRuleConfig>`; each rule has `id`, `level`, `pattern`, `never_send`, `description`.
- **Compilation:** Not yet wired; future: compile each `pattern` into `Regex` and cache by `id`.
- **Fallback:** On parse failure, use built-in rules from `data_sensitivity::RULES`.

## Sprint 8 – Observability & Recent Sessions

- **Unit tests (oxcer-core):** `log_event` (serialization, missing metrics, one JSON line per call); LLM cost from token counts and pricing; security/policy log event shape (rule_id, decision). See `oxcer-core/src/telemetry.rs`, `oxcer-core/src/llm_metrics.rs`.
- **Integration (oxcer-core):** `oxcer-core/tests/sprint8_telemetry_integration.rs` — writes a small session trace (semantic_router, security, llm_client with metrics, orchestrator) via `log_event`, then asserts the session JSONL exists, parses as `LogEvent`s, contains expected components, and at least one event has non-zero tokens/latency.
- **Integration (Tauri):** `apps/desktop-tauri/src-tauri/tests/sprint8_recent_sessions_integration.rs` — writes a session file with `oxcer_core::telemetry::log_event`, then calls `telemetry_viewer::list_sessions_from_dir` and `load_session_log_from_dir` and asserts summary and event content.

**Manual QA (Recent Sessions UI):**

1. Run the app and trigger at least one agent session that produces telemetry (e.g. run a task that hits the router, policy, and LLM).
2. Open the Launcher dashboard and switch to the **Recent Sessions** tab.
3. Confirm the list loads and shows the session(s) (session id, timestamps, cost, success, tool/approval/deny counts).
4. Select a session and confirm the timeline table shows rows (time, component, action, decision, metrics).
5. Use the **Component** and **Decision** filters and confirm the table updates (e.g. filter by component `security`, then by decision `allow` or `approval_required`).
6. Use the text search filter and confirm only matching events are shown.
7. Expand an event row and confirm the JSON details panel shows the event payload.

## Sprint 9 – Plugin System

- **Unit tests (oxcer-core):** `oxcer-core/tests/sprint9_plugin_system.rs` — invalid YAML skipped, dangerous plugin requires approval, plugin telemetry (plugin_start/plugin_end), git_status E2E (load -> catalog -> policy -> tool_hints).
- **Capability registry:** `for_category`, `for_tag`, `matching_ids_for_task`; indexed lookups for scale.
- **Manual QA (git_status E2E):** See [docs/PLUGIN_SYSTEM.md](PLUGIN_SYSTEM.md#manual-qa-checklist-git_status-e2e) for the step-by-step checklist.

## Full Test & QA Checklist (Sprints 1–8 + SwiftUI)

### 1. Test Surface Map

#### 1.1 Rust core
| Component | Key behaviors | Failure modes | Best covered by |
|-----------|---------------|---------------|-----------------|
| **Security / policy** | Path blocklist, command blacklist, caller-specific rules (UI vs Agent), data_sensitivity | Wrong deny/allow/approval, blocklist bypass, path expansion bugs | Unit + integration |
| **Semantic router** | Category (SimpleQa, ToolsHeavy, Planning), strategy (Cheap/Expensive/ToolsOnly), flags (requires_high_risk_approval) | Misrouting, missing high-risk flag | Unit |
| **Orchestrator** | start_session -> plan, next_action state machine, agent_request loop | Empty plan, step error not propagated, ApprovalPending loop | Unit + integration |
| **Tools** | FsListDir, FsDelete, LlmGenerate intents; heuristic planner | Wrong intent for task, empty plan when no heuristic | Unit |
| **Telemetry** | log_event -> JSONL, list_sessions_from_dir, load_session_log_from_dir | Session file missing, parse errors, wrong summary fields | Unit + integration |
| **Data sensitivity** | Classify (High/Medium/Low), scrub placeholders, NeverSendToLlm | Secrets leaked, false positives | Unit |
| **Prompt sanitizer** | Redact JWTs, API keys, sensitive paths | Raw secrets in LLM payload | Unit |
| **Env filter** | filter_env_for_child removes high-risk keys | Secrets in child process env | Unit |

#### 1.2 Tauri app / commands
| Component | Key behaviors | Failure modes | Best covered by |
|-----------|---------------|---------------|-----------------|
| **Commands** | cmd_fs_*, cmd_shell_run, cmd_approve_and_execute, cmd_scrub_payload_for_llm | Null/malformed args, policy bypass | Integration |
| **Telemetry viewer** | list_sessions_from_dir, load_session_log_from_dir | Path resolution, missing dir | Integration |
| **Workspace cleanup** | workspace_cleanup_on_delete | State leak, approval cancellation | Integration |
| **Settings** | load/save config.json | Corruption, missing file | Integration |

#### 1.3 FFI layer (oxcer_ffi)
| Component | Key behaviors | Failure modes | Best covered by |
|-----------|---------------|---------------|-----------------|
| **JSON contracts** | oxcer_list_workspaces, oxcer_list_sessions, oxcer_load_session_log, oxcer_agent_request | Invalid input JSON, missing fields, wrong output shape | Unit (Rust) |
| **Memory** | oxcer_string_free, null input handling | Leak, double-free, null deref | Unit (Rust) |
| **Error propagation** | Rust Err -> `{ "ok": false, "error": "..." }` | Panic, wrong error shape | Unit (Rust) |

#### 1.4 SwiftUI OxcerLauncher
| Component | Key behaviors | Failure modes | Best covered by |
|-----------|---------------|---------------|-----------------|
| **Workspace loading** | loadWorkspaces -> OxcerFFI.listWorkspaces | Empty config, broken JSON, wrong path | Unit (XCTest) |
| **Task execution** | runAgentRequest -> OxcerFFI.agentRequest | Empty task, FFI error, UI not updating | Unit (mocked FFI) |
| **Recent Sessions** | loadSessions, loadSessionLog | No logs, corrupted JSONL, large file | Unit (mocked data) |
| **UI state** | isRunning, errorMessage, resultText | Race conditions, stale state | Manual QA |

#### 1.5 Cross-cutting
| Concern | Key behaviors | Failure modes | Best covered by |
|---------|---------------|---------------|-----------------|
| **Config** | config.json schema, workspaces array | Malformed YAML/JSON, missing root_path | Integration |
| **Logs** | logs/{session_id}.jsonl, telemetry.jsonl | Retention, sanitization, parse errors | Integration |
| **Error handling** | Err propagation from Rust -> FFI -> Swift | Swallowed errors, wrong user message | Integration + Manual |
| **Performance** | Large session logs, many workspaces | UI freeze, OOM | Manual QA |

---

### 2. End-to-End Test Plan

#### Fast suite (run on every change)
- [ ] `cargo test -p oxcer-core` — all unit + integration tests in oxcer-core (~30s)
- [ ] `cargo test -p oxcer_ffi` — FFI round-trip and negative tests (~5s)

**Data sensitivity rule tests (fast):** Run all rule tests with `cargo test -p oxcer-core data_sensitivity_`. These are parameterized tests in `oxcer-core/tests/data_sensitivity_*.rs` (ssh_keys, tokens, passwords_env, medium, merge). Always run after modifying patterns.

#### Slower suite (run before merge / release)
- [ ] `cargo test --workspace --features test` — Tauri + oxcer-core + oxcer_ffi (~1 min)
- [ ] Build OxcerLauncher: open `apps/OxcerLauncher/OxcerLauncher.xcodeproj` in Xcode, Product -> Build
- [ ] Run Swift tests: Product -> Test (if XCTest target added)

#### Manual QA checklist (before release)
- [ ] **Launch:** OxcerLauncher opens; no crash; app config dir created
- [ ] **Workspace:** Load workspaces from config.json; select workspace; empty config shows "No workspaces"
- [ ] **Task:** Enter task "What is Rust?"; Run Task; result or stub error shown; no crash
- [ ] **Task (validation):** Empty task -> Run disabled; whitespace-only -> Run disabled
- [ ] **Recent Sessions:** Tab loads; empty state shows "Select a session"; after a session, list shows entries
- [ ] **Recent Sessions (detail):** Select session; timeline shows events; expand row -> JSON details
- [ ] **Recent Sessions (edge):** Corrupted JSONL line -> app does not crash; large log (>1000 events) -> scrolls
- [ ] **FFI error:** Temporarily pass invalid app_config_dir -> error message shown in UI

---

### 3. When Something Is Broken, Start Here

| Symptom | Commands / Steps |
|---------|------------------|
| Rust tests fail | `cd oxcer-core && cargo test --no-fail-fast 2>&1 \| head -80` |
| FFI tests fail | `cargo test -p oxcer_ffi` |
| Tauri tests fail | `cargo test -p oxcer --features test` |
| Build fails | `cargo build --workspace` then `cargo build -p oxcer_ffi --release` |
| OxcerLauncher won't build | Ensure `cargo build -p oxcer_ffi --release` succeeds; check Xcode Build Rust dylib phase |
| App crashes on launch | Run from Xcode; check Console for Rust panic or dylib load failure |
| No workspaces shown | Verify `~/Library/Application Support/Oxcer/config.json` exists and has valid `workspaces` array |
| No recent sessions | Verify `~/Library/Application Support/Oxcer/logs/` exists; run a task that produces telemetry |
| Task runs but no answer | FFI uses stub executor; tools return Err. Use Tauri step API for full execution. |

#### Debug logging
- **Rust:** Set `RUST_LOG=oxcer_core=debug` (or `trace`) when running; logs go to stderr.
- **OxcerLauncher:** Add `print()` or `os_log` in Swift; run from Xcode to see console.
- **Telemetry:** Inspect `logs/{session_id}.jsonl` and `logs/telemetry.jsonl` for event flow.

#### Isolating bugs
1. Reproduce in the smallest scope: unit test in oxcer-core if logic-only.
2. If FFI boundary: add a Rust test that calls the FFI function directly with the failing input.
3. If Swift: create XCTest with mocked OxcerFFI returning controlled data.

#### Recent refactors (core modules)
- **policy_engine:** `PolicyTarget` now uses manual `impl Default` (returns `FsPath { canonical_path: String::new() }`) instead of `#[default]` on a variant with fields. Semantics unchanged; improves compatibility.
- **data_sensitivity:** `merge_findings` refactored to use `merge_overlapping_spans` helper; explicit `Vec<SensitivityFinding>` types to avoid inference issues. Tests in `data_sensitivity_merge.rs`.
- **orchestrator:** Removed unused `TaskCategory` import; `start_session` uses `let session` (no mut). No API changes.

---

## SwiftUI OxcerLauncher testing

### Adding an XCTest target

1. In Xcode: File -> New -> Target -> Unit Testing Bundle.
2. Name it `OxcerLauncherTests`, set Host Application = OxcerLauncher.
3. Add the test files from `apps/OxcerLauncher/OxcerLauncherTests/` to the target.
4. Run tests: Product -> Test (⌘U).

### Test files provided

- **OxcerFFITests.swift** — Integration tests calling real OxcerFFI (requires built dylib):
  - `testListWorkspaces_validConfig_returnsWorkspaces`
  - `testListWorkspaces_emptyConfig_returnsEmpty`
  - `testAgentRequest_invalidPayload_propagatesError`
  - `testListSessions_emptyLogsDir_returnsEmpty`
  - `testLoadSessionLog_nonexistentSession_throws`
- **OxcerSwiftUIViewModelTests.swift** — Codable and data-shape tests (no FFI):
  - `testEmptyWorkspaces_dataShape`
  - `testFirstWorkspaceSelection`
  - `testSessionSummary_decodesFromJSON`

### Swift-side FFI abuse sample

To verify error propagation from invalid payloads:

```swift
// In OxcerFFITests
func testAgentRequest_emptyTask_throws() {
    let payload = AgentRequestPayload(taskDescription: "", ...)
    XCTAssertThrowsError(try OxcerFFI.agentRequest(payload))
}
```

Passing malformed JSON or missing required fields causes Rust to return `{ "ok": false, "error": "..." }`; Swift decodes and throws `OxcerFFIError.rustError`.

### SwiftUI manual QA checklist

- [ ] **Launch:** App opens; no crash; `~/Library/Application Support/Oxcer` exists or is created.
- [ ] **Workspace tab:** Load workspaces; empty config shows "No workspaces (config at…)".
- [ ] **Workspace tab:** Valid config.json with workspaces -> picker shows entries; select one.
- [ ] **Task tab:** Empty task -> Run disabled; whitespace-only -> Run disabled.
- [ ] **Task tab:** Valid task -> Run Task; stub executor path shows error or answer (FFI uses stub).
- [ ] **Recent Sessions:** Tab loads; empty logs -> "Select a session"; Refresh works.
- [ ] **Recent Sessions:** After a session exists -> list shows entry; select -> timeline loads.
- [ ] **Recent Sessions:** Corrupted JSONL line -> app does not crash; bad lines skipped.

---

## Keeping the loop tight

- Fix the **first 1–3** errors from `cargo test` or `cargo build`, then re-run; avoid fixing many errors in one go.
- Do not paste LLM meta tokens (`<think>`, `<|tool_calls_begin|>`, etc.) into `.rs` files; keep design notes in `.md` and only validated Rust in source.
