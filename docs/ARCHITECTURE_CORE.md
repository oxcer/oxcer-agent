# Core Modules Refactor Guidelines

Guidelines for maintaining `oxcer-core` modules: security/policy_engine, data_sensitivity, prompt_sanitizer, semantic_router, and orchestrator.

---

## Product Policy Tables

### Prompt sanitizer / data_sensitivity / security

| pattern_id / finding_kind | NeverSend vs ScrubAndAllow | Redaction strategy | Example input | Expected outcome |
|---------------------------|----------------------------|--------------------|---------------|------------------|
| `aws_access_key_id` | **NeverSend** | — | `AKIAIOSFODNN7EXAMPLE` | `Err(NeverSendToLlm)`; LLM call aborted |
| `aws_credentials` | **NeverSend** | — | `aws_secret_access_key=...` | `Err(NeverSendToLlm)` |
| `jwt_or_oauth_token` | **NeverSend** | — | `eyJ...` JWT | `Err(NeverSendToLlm)` |
| `api_key` | **NeverSend** | — | `OPENAI_API_KEY=sk-...` | `Err(NeverSendToLlm)` |
| `ssh_private_key`, `pem_private_key_header`, `ssh_private_key_path` | **NeverSend** | — | PEM block, `~/.ssh/id_rsa` | `Err(NeverSendToLlm)` |
| `password_in_env`, `password_in_url`, `secret_or_password_env` | **NeverSend** | — | `DB_PASSWORD=...`, `user:pass@host` | `Err(NeverSendToLlm)` |
| `ip_address`, `base64_long`, `keychain_or_credential_path`, `authorization_bearer` | **ScrubAndAllow** | Placeholder `[REDACTED: kind]` | `192.168.1.1`, long base64 | Scrubbed string sent to LLM |
| ≥50% redacted | **ScrubAndBlock** | — | Payload mostly secrets | `Err(TooMuchSensitiveData)` |

**Behavior contract:** `sanitize_text` / `sanitize_text_with_options` return redacted content only (no never-send check). `scrub_for_llm_call` and `build_and_scrub_for_llm` enforce never-send and threshold; they return `Err` when credentials are present. When changing rules, update both the policy table and tests.

### Semantic router

| Example prompt | Expected route | Strategy | Rationale |
|----------------|----------------|----------|-----------|
| "What is Rust?" | SimpleQa | CheapModel | Short, ends with ?, no tool/code markers |
| "Fix the bug in main.rs" (with selected_paths) | Code | CheapModel | Code markers or selected paths; short |
| Long code task (≥planning_length_threshold chars) | Planning | ExpensiveModel | Length over threshold → planning |
| "I need a plan and strategy for refactoring" | Planning | ExpensiveModel | Planning keywords |
| "list files", "delete foo.txt" | ToolsHeavy | ToolsOnly (if prefer_tools_only) or CheapModel | Tool verbs |
| "Do something useful" (no markers) | Code | CheapModel | Default fallback |

**Behavior contract:** Routing order: 1) tool verbs → ToolsHeavy, 2) many paths → ToolsHeavy, 3) planning/long → Planning, 4) code markers → Code, 5) simple_qa → SimpleQa, 6) default → Code. When changing thresholds or keywords, update both the policy table and tests.

### Orchestrator (tools-only flows)

| Task | Expected plan shape | Rationale |
|------|---------------------|-----------|
| "list files in workspace" (with prefer_tools_only + workspace_root) | At least one `FsListDir { rel_path: "." }` | Heuristic maps to FsListDir |
| "delete foo.txt" (with prefer_tools_only + workspace_root) | Exactly one `FsDelete { rel_path: "foo.txt" }` | Heuristic maps to FsDelete |
| No heuristic match (tools_only strategy) | Empty plan | Planner returns no steps; session completes with "Done." |

**Behavior contract:** Tools-only plan is built by `build_plan_tools_only`; it requires `prefer_tools_only: true` and a non-empty `workspace_root`. Assert plan contains expected intent (e.g. `plan.iter().any(...)`) rather than assuming step order unless the heuristic guarantees it.

---

## security / policy_engine

- **Defaults & enums:** Use `impl Default` for enums with non-unit variants (e.g. `PolicyTarget::FsPath { canonical_path: String }`). Avoid `#[default]` on variants with fields when it causes compatibility issues; prefer explicit `impl Default` that returns a clear “empty” or “unknown” value.
- **Error handling:** Policy evaluation does not return `Result`; invalid config falls back to secure default (default-deny). Log or surface config parse errors separately if needed.
- **Testing:** Add tests for default values (`PolicyRequest::default()`, `PolicyTarget::default()`) to lock in semantics.

## data_sensitivity

- **Call sites:** Public API takes `&str`. When iterating over `Vec<String>`, pass `&s` (e.g. `classify_and_mask_default(s)` where `s: &String`). When `s` is from `format!(...)`, pass `&s`.
- **Rule definitions:** Keep regex patterns in `r#"..."#` or raw strings to avoid escaping issues (e.g. single quote in character class). Use shared constants (`PEM_KEY_TYPE`, `PATH_BOUNDARY`, `PATTERN_*`) for repeated fragments.
- **Merge logic:** `merge_findings` and `dedup_contained` operate on `Vec<SensitivityFinding>`. Use explicit type annotations on intermediate `Vec`s to avoid inference edge cases. Extract helpers (e.g. `merge_overlapping_spans`) when logic is reusable.
- **Tests:** Parameterized tests in `oxcer-core/tests/data_sensitivity_*.rs` cover each rule (should match / should NOT match) and merge behavior (overlapping findings).
- **Performance:** Rules use `OnceLock<Regex>`. Keep rules deterministic and avoid external I/O in the hot path.

### Performance considerations

| Aspect | Value | Notes |
|--------|-------|-------|
| Regexes per input | 14 (10 high + 4 medium) | Each `classify_and_mask` call runs all rules once |
| Expected input sizes | Short: prompts ~100–2K chars; Long: logs/code blocks ~10K–100K chars | Hot path is every LLM-bound payload |
| Hotspots | `base64_long` on large blobs (128+ char word-boundary scan); PEM block on multi-line keys | Avoid running same regex twice; use `OnceLock` |
| Micro-bench | `data_sensitivity_completes_on_long_input` (cfg(test)) | Asserts `classify_and_mask_default` completes on ~50K char input; run with `cargo test -p oxcer-core data_sensitivity_completes` |

## prompt_sanitizer

- **Test naming:** Do not name a test function the same as a public utility (e.g. `to_workspace_relative_path`). A test named `fn to_workspace_relative_path()` shadows the real function and causes "takes 0 arguments but 2 supplied" when the test calls it. Use distinct names like `to_workspace_relative_path_basic_cases`.

## orchestrator

- **State management:** `SessionState` is mutated in place by `agent_step` and `next_action`. Tests use `let mut session` when passing `&mut session` to these functions. Do not add `mut` where the binding is never mutated (e.g. `start_session` creates and returns a session without mutating it).
- **Imports:** Import only what the module uses. Remove unused symbols (e.g. `TaskCategory` when only `RouterDecision` is needed).
- **Error propagation:** Orchestrator returns `Result<..., String>`. Keep error messages specific enough for debugging; avoid swallowing errors.
- **Testability:** `agent_step` and `agent_request` accept injectable executor. Use stub executors in tests; avoid real network/LLM calls.
- **Pattern matching in tests:** Use `matches!(&session.plan[i], ToolCallIntent::Variant { field, .. } if field == value)` to avoid "cannot move out of index" — match by reference, not by value.
- **Tools-only tests:** Use `prefer_tools_only: true` in RouterConfig so the router returns ToolsOnly; otherwise the plan is LlmGenerate. Assert plan contains expected intent with `plan.iter().any(...)`.

---

## Behavior Contract Summary

When changing routes, policies, or scrubber rules:

1. **Update the policy table** in this document (NeverSend vs ScrubAndAllow, routing thresholds, plan shape).
2. **Update the tests** to match the intended behavior; do not weaken security to satisfy old test expectations.
3. **Run `cargo test -p oxcer-core`** and ensure all tests pass.
