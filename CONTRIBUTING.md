# Contributing to Oxcer

> **Read this before opening a PR.** Covers project structure, the FFI workflow, disabled code paths, experimental features, testing, and code style.

---

## Table of Contents

1. [Project Structure](#project-structure)
2. [FFI Workflow](#ffi-workflow)
3. [Disabled Workflows](#disabled-workflows)
4. [Experimental Features](#experimental-features)
5. [Filing Issues](#filing-issues)
6. [Contributing Code](#contributing-code)
7. [Testing](#testing)
8. [Code Style](#code-style)
9. [Network Access Policy](#network-access-policy)
10. [Commit Conventions](#commit-conventions)
11. [License](#license)

---

## Project Structure

Three layers. Each has a distinct responsibility boundary.

```
oxcer-core/          # Pure Rust library — agent orchestrator, LLM engine, security, tools
oxcer_ffi/           # Rust → Swift FFI bridge (UniFFI 0.28, attribute macros, no .udl)
apps/
  OxcerLauncher/     # macOS SwiftUI app — the only production UI target
  desktop-tauri/     # Cross-platform Tauri shell — backend-only stub, no production UI
  windows-launcher/  # Planned WinUI 3 launcher — stub only
plugins/             # YAML plugin definitions
config/              # Policies and defaults
docs/                # Architecture, development, security docs, whitepaper.pdf
scripts/             # regen-ffi.sh, check-ffi-freshness.sh, dev helpers
demo/                # Test files for Workflow 1 (Test1_doc.md) and Workflow 2 (Test2_doc*.md)
```

### oxcer-core

The agent loop (`orchestrator/`), semantic router, LLM engine abstraction (`llm/`), file tools (`tools/`), data sensitivity scanner (`data_sensitivity/`), and memory/logging modules. No Swift dependencies. No platform APIs. Fully testable with `cargo test`.

Key subdirectories:

| Path | Contents |
|------|----------|
| `oxcer-core/src/orchestrator/` | `planning.rs` (plan builder), `execution.rs` (step loop), `types.rs` (intent/state types) |
| `oxcer-core/src/llm/` | `LlmEngine` trait, `LocalPhi3Engine`, `CloudLlmEngine`, `HttpLlmEngine`, `HybridEngine` |
| `oxcer-core/src/llm/local_phi3/` | `LlamaCppPhiRuntime` (llama.cpp + Metal), `PhiRuntime` trait |
| `oxcer-core/src/semantic_router.rs` | Keyword-based router — no LLM, no network |
| `oxcer-core/src/data_sensitivity/` | DLP scanner — redacts credentials before any LLM call |
| `oxcer-core/src/memory.rs` | Markdown-backed fact store — implemented, not yet wired to production loop |
| `oxcer-core/src/db.rs` | SQLite episodic store — implemented, not yet wired to production loop |
| `oxcer-core/src/agent_session_log.rs` | Per-session structured log — implemented, no callers yet |

### oxcer_ffi

UniFFI 0.28 bridge. The Rust side is `oxcer_ffi/src/lib.rs`. The generated Swift side is `apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift` and `apps/OxcerLauncher/OxcerLauncher/oxcer_ffiFFI.h`.

`ffi_agent_step` is the main per-step entry point. Session state is an opaque JSON blob. Changes to any `#[uniffi::export]` item require regenerating the Swift bindings (see [FFI Workflow](#ffi-workflow)).

### OxcerLauncher

SwiftUI app. Key files:

| File | Responsibility |
|------|----------------|
| `ContentView.swift` | Root view, `AppViewModel`, `ApprovalBubble`, `DetailView` |
| `AgentRunner.swift` | `AgentRunner` struct — drives the `ffi_agent_step` while-loop; `AgentEnvironment` |
| `SwiftAgentExecutor.swift` | Executes each `FfiToolIntent` (FS ops, LLM generate, shell) |
| `OxcerBackend.swift` | `OxcerBackend` protocol + `DefaultOxcerBackend` implementation |
| `oxcer_ffi.swift` | Generated — do not edit manually |
| `oxcer_ffiFFI.h` | Generated — do not edit manually |

---

## FFI Workflow

Every time you add, remove, or change a `#[uniffi::export]` item in `oxcer_ffi/src/lib.rs`, you must regenerate both generated files and commit them together with the Rust change.

### Regenerating bindings

Always regenerate from the **release** dylib:

```bash
./scripts/regen-ffi.sh
```

This script rebuilds `liboxcer_ffi.dylib` in release mode and copies both generated files to their committed locations:

- `apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift`
- `apps/OxcerLauncher/OxcerLauncher/oxcer_ffiFFI.h`

After regenerating, do a **Clean Build Folder** in Xcode (⇧⌘K) before building to avoid stale object files.

### Committing an FFI change

```bash
git add oxcer_ffi/src/lib.rs \
        apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift \
        apps/OxcerLauncher/OxcerLauncher/oxcer_ffiFFI.h
git commit -m 'ffi: <describe the contract change>'
```

Never regenerate from the debug dylib. Debug and release builds produce different ABI checksums. A debug/release mismatch causes `apiChecksumMismatch` at runtime (crash on launch).

### Pre-push hook

Wire the freshness check as a pre-push hook so CI never catches what your local build already knows:

```bash
cp scripts/check-ffi-freshness.sh .git/hooks/pre-push
chmod +x .git/hooks/pre-push
```

The hook builds the release dylib, runs `uniffi-bindgen`, and diffs the output against the committed files. It exits non-zero (blocking the push) if any diff is found.

### CI enforcement

The `uniffi-binding-freshness` CI job performs the same check on every PR and push to `main`. PRs with stale bindings fail automatically. The job checks both `.swift` and `.h` — both must be up to date.

---

## Disabled Workflows

**Do not re-enable Workflow 2 or Workflow 3 without end-to-end validation.**

Two plan expansion paths exist in `oxcer-core/src/orchestrator/planning.rs` and are intentionally disabled in v0.1:

### Workflow 2 — Multi-file summarization (`ReadAndSummarize`)

`start_session` sets `pending_expansion = None` unconditionally. The expansion function `do_expand_plan` in `execution.rs` contains the `ReadAndSummarize` arm, and the plan builder `build_plan_list_then_multi_summarize` is marked `#[allow(dead_code)]`.

The blocker is context-budget safety: `content_accumulator` collects `FsReadFile` results until `{{FILE_CONTENTS}}` is substituted into the `LlmGenerate` task. There is no enforcement that the accumulated content fits within `FS_RESULT_MAX_CHARS` across N files without overflowing the 8 192-token context window.

Before re-enabling: add an `accumulator_byte_limit` guard in `do_expand_plan` that truncates or rejects the expansion if the total accumulated size would exceed safe limits, and add integration tests covering the overflow case.

### Workflow 3 — Folder-level move operations (`MoveToDir`)

Same pattern: `pending_expansion = None`, `build_plan_list_then_move` marked `#[allow(dead_code)]`.

The blocker is fan-out validation: `do_expand_plan` inserts `[FsCreateDir(dest), FsMove×N]`. On large directories (hundreds of files), this produces a plan with hundreds of write intents, each requiring human approval. The approval UI is not designed for that volume, and there is no plan size limit.

Before re-enabling: add a `max_move_fan_out` cap, define the UX for bulk approval, and test against directories of realistic size.

---

## Experimental Features

Features behind `#[cfg(feature = "experimental")]` are not compiled into release builds and are not connected to the production UI. Do not depend on them in code that ships.

### `fsm.rs` — Stateful agent FSM

`oxcer-core/src/fsm.rs` implements `AgentFsm`, a step-driven state machine with `StateDb` (SQLite episodic store) injected as context. It is connected to the `orchestrate_query` FFI export in `oxcer_ffi/src/lib.rs` but not to `ffi_agent_step`.

### Subagent orchestration (`ffi_orchestrate`)

`ffi_orchestrate` in `oxcer_ffi/src/lib.rs` calls `subagent::orchestrate`, which uses `memory.rs` (the Markdown fact store). This path is experimental, not called from the UI, and not covered by the production test suite in its current form.

### `memory.rs`, `db.rs`, `agent_session_log.rs`

All three are fully implemented with real logic and passing unit tests. None are connected to `ffi_agent_step`. They are documented in [ROADMAP.md](ROADMAP.md) as v1.0 wiring work.

Do not add callers to these modules in production code paths without a corresponding tracking issue and end-to-end test coverage.

---

## Filing Issues

### Bug Reports

Use the **Bug Report** issue template. Required fields:

- **Oxcer version** (git SHA or release tag — `git rev-parse --short HEAD`)
- **macOS version** and **Mac chip** (e.g. M3 Max)
- **Xcode version** and **Rust toolchain** (`rustc --version`) if built from source
- **Model file name** (e.g. `Meta-Llama-3-8B-Instruct-Q4_K_M.gguf`)
- **Steps to reproduce** — exact sequence that triggers the bug
- **Expected behaviour** and **actual behaviour** as separate fields
- **Console output or crash log** — set `OXCER_LOG=debug` for verbose Rust output; crash reports are at `~/Library/Logs/DiagnosticReports/OxcerLauncher_*.ips`

**Do not include credentials, API keys, personal file paths, or file contents in bug reports.** Oxcer's DLP scanner redacts credentials before inference, but the issue tracker is public.

### Feature Requests

Use the **Feature Request** template. Lead with the use case (the task you are trying to accomplish), not the implementation. Check [ROADMAP.md](ROADMAP.md) first — your feature may already be planned.

### Security Vulnerabilities

Do **not** open a public issue for security vulnerabilities. Use GitHub's private security advisory flow (linked on the template chooser page) or email `security@oxcer.app`. We aim to respond within 72 hours. See [docs/security.md](docs/security.md) for the full security model and reporting guidelines.

---

## Contributing Code

### Prerequisites

```bash
brew install cmake          # required by llama-cpp-sys
cargo build --release -p oxcer_ffi
open apps/OxcerLauncher/OxcerLauncher.xcodeproj   # ⌘R should launch the app
```

See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for the full build guide.

### Fork and Branch

1. Fork the repository on GitHub.
2. Clone your fork and add the upstream remote:

   ```bash
   git clone https://github.com/your-username/oxcer.git
   cd oxcer
   git remote add upstream https://github.com/your-org/oxcer.git
   ```

3. Create a branch from `main`:

   | Change type | Branch prefix | Example |
   |---|---|---|
   | Bug fix | `fix/` | `fix/approval-overlay-dismiss` |
   | New feature | `feat/` | `feat/streaming-output` |
   | Documentation | `docs/` | `docs/update-security-guide` |
   | Refactoring | `refactor/` | `refactor/agent-runner-cleanup` |
   | CI / tooling | `ci/` | `ci/add-clippy-job` |

### Pull Request Workflow

1. **Keep PRs focused.** One logical change per PR. Separate refactors from features.
2. **Write a clear description.** Explain what changed and why.
3. **Reference the issue.** `Closes #123` or `Related to #123` in the PR body.
4. **Pass all CI checks** before requesting review.
5. **FFI changes:** regenerate bindings before opening the PR (see [FFI Workflow](#ffi-workflow)).
6. **Stale PRs** may be closed after 30 days of inactivity.

---

## Testing

Run the full test suite before opening a PR:

```bash
# Rust core — unit + integration tests
cargo test -p oxcer-core

# FFI contract tests — catches stale bindings and wrong return types
cargo test -p oxcer_ffi

# Full workspace type-check (no link step; faster than cargo build)
cargo check --workspace
```

All three must pass. CI runs them with `--locked` (pinned `Cargo.lock`).

For Swift: run the **OxcerLauncherTests** target in Xcode (⌘U). Add `XCTest` cases in `apps/OxcerLauncher/OxcerLauncherTests/` for any change that affects view model logic or FFI wiring.

### Adding tests

- New functionality in `oxcer-core` must include unit tests in the same file or in `oxcer-core/tests/`.
- New FFI exports must include a round-trip test in `oxcer_ffi/src/lib.rs` `#[cfg(test)]`.
- New Swift view model logic must include an `XCTest` case.

### macOS app build check

After Swift or Xcode project changes:

```bash
cargo build --release -p oxcer_ffi
xcodebuild \
  -project apps/OxcerLauncher/OxcerLauncher.xcodeproj \
  -scheme OxcerLauncher \
  -destination 'platform=macOS' \
  build
```

### CI overview

| Job | Runs on | What it checks |
|-----|---------|----------------|
| **Lint** | Linux | `cargo fmt --check`, `cargo clippy --all-targets -D warnings` |
| **Rust tests** | Linux | `cargo check --workspace`, `cargo test` for `oxcer-core` + `oxcer_ffi` |
| **macOS build + FFI freshness** | macOS | Dylib build, Xcode build, UniFFI binding freshness (`.swift` + `.h`) |

The macOS job only starts after both Linux jobs pass.

### Pre-commit hooks

```bash
pip install pre-commit
pre-commit install
brew install swiftformat
```

Hooks: `cargo-fmt`, `swiftformat`, `detect-private-key`, `check-yaml`, `check-json`, `check-merge-conflict`, `end-of-file-fixer`, `trailing-whitespace`. Clippy is `manual` stage (run explicitly when needed):

```bash
pre-commit run --hook-stage manual cargo-clippy --all-files
```

---

## Code Style

### Rust

Format with `rustfmt` before committing:

```bash
cargo fmt
```

Linting:

```bash
cargo clippy -- -D warnings
```

Style notes:
- Prefer `thiserror` for error types in library crates. Avoid `Box<dyn Error>` at API boundaries.
- Keep `pub` surface minimal — expose only what callers need.
- Use `tracing::` macros (`tracing::info!`, `tracing::debug!`, etc.) for diagnostics. Never `println!` or `eprintln!`. Control verbosity with `OXCER_LOG=debug`.
- All text handed to the LLM must pass through `scrub_for_llm_call`. Do not bypass the data sensitivity pipeline.

### Swift / SwiftUI

Format before committing:

```bash
swiftformat apps/OxcerLauncher/OxcerLauncher/
```

Style notes:
- View state that must survive re-renders belongs in `@StateObject` or `@ObservedObject`, not local `@State` on a parent.
- Views that observe `ObservableObject` properties must use `@ObservedObject`, not `let`. Plain `let` on a struct view does not subscribe to `objectWillChange`.
- Use `os.Logger` over `print()`. `.debug()` for verbose output, `.info()` for lifecycle events, `.error()` for failures.
- Avoid `.id()` as a "refresh key" on views that own `@StateObject` — it destroys the subtree and recreates state.

---

## Network Access Policy

**Oxcer does not make arbitrary HTTP requests and is not a web-browsing agent.** This is a deliberate design constraint.

No tool in the agent loop (`fs_list_dir`, `fs_read_file`, `shell_run`, etc.) makes outbound network calls. The local inference path (`LlamaCppPhiRuntime` via llama.cpp + Metal) is fully offline. No token, prompt, or file content leaves the machine during inference.

### Permitted network calls

1. **Model download.** `ensure_local_model()` fetches the GGUF file from a fixed, pinned URL over HTTPS on first run, with user awareness. This is the only current outbound call; it is not triggered by the agent loop.
2. **Cloud model APIs.** The `CLOUD_ENGINE_SLOT` backend supports OpenAI, Anthropic, Gemini, and Grok as optional, explicitly opt-in backends. When enabled, prompts pass through the same DLP scanner as local inference before being sent.

### Do not add

- HTTP client code (`reqwest`, `URLSession`, `curl` subprocess) outside the model-download path without opening an issue first.
- Any tool intent that fetches URLs or executes web requests.
- Shell commands in tests or examples that make outbound connections.

If your feature requires network access, describe the exact scope, endpoint set, and security controls in your issue before writing code.

---

## Commit Conventions

Lightweight prefix scheme:

```
<type>(<scope>): <short summary>
```

| Type | When to use |
|---|---|
| `feat` | A new user-visible feature |
| `fix` | A bug fix |
| `refactor` | Code change with no behaviour change |
| `test` | Adding or fixing tests |
| `docs` | Documentation only |
| `ci` | CI/CD changes |
| `chore` | Maintenance (dependency bumps, cleanup) |
| `ffi` | FFI contract change — always regenerate bindings |

Scope is optional but helpful: `ffi`, `agent`, `security`, `swift`, `llm`, `ci`.

Examples:

```
feat(agent): add session pin and rename to sidebar
fix(ffi): correct list_workspaces return type to prevent 88 GB VM spike
ffi(orchestrator): expose ffi_agent_step with session JSON opaque blob
docs: rewrite ROADMAP to align with v1.0 milestone structure
ci: add cargo clippy and Swift build jobs
```

- **Summary line:** ≤ 72 characters, present tense, no trailing period.
- **Body:** Optional. Use it to explain *why*, not *what*.
- **Breaking changes:** Add `BREAKING CHANGE:` in the commit body and describe the migration path.

---

## License

By submitting a pull request you agree that your contribution will be licensed under the same license as this project (see [LICENSE](LICENSE)).
