# Contributing to Oxcer

> **Read this if** you want to open an issue or pull request. Covers code style, the FFI workflow, and what makes a clean contribution.

Thank you for your interest in contributing. This guide covers everything you need to open a useful issue or a clean pull request.

---

## Table of Contents

1. [Filing Issues](#filing-issues)
2. [Contributing Code](#contributing-code)
3. [Code Style](#code-style)
4. [Network Access Policy](#network-access-policy)
5. [Testing](#testing)
6. [Commit Conventions](#commit-conventions)
7. [License](#license)

---

## Filing Issues

### Bug Reports

Use the **Bug Report** issue template and include:

- **Oxcer version** (git SHA or release tag).
- **macOS version** and **Xcode version**.
- **Rust toolchain version** (`rustc --version`).
- **Model file name and size** (e.g. `Meta-Llama-3-8B-Instruct-Q4_K_M.gguf`, ~4.7 GB).
- **Steps to reproduce** — the exact sequence that triggers the bug.
- **Expected behavior** and **actual behavior**.
- **Console output or crash log** — run from Xcode and copy the relevant lines from the debug console. For Rust panics, include the full backtrace (`RUST_BACKTRACE=1`).

**Do not include credentials, API keys, or personal file paths in bug reports.**

### Feature Requests

Use the **Feature Request** template and describe:

- The use case you are trying to solve (not just the feature itself).
- Any constraints you are aware of (on-device only, security model, etc.).
- Whether you are willing to implement it.

### Security Vulnerabilities

Do **not** open a public issue for security vulnerabilities. Instead, email `security@oxcer.dev` (or use the private security advisory feature on GitHub) with a description and reproduction steps. We aim to respond within 72 hours.

---

## Contributing Code

### Prerequisites

Make sure you can build the project locally before opening a PR:

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

3. Create a branch from `main` using the convention below:

   | Change type | Branch prefix | Example |
   |---|---|---|
   | Bug fix | `fix/` | `fix/approval-overlay-dismiss` |
   | New feature | `feat/` | `feat/session-export` |
   | Documentation | `docs/` | `docs/update-security-guide` |
   | Refactoring | `refactor/` | `refactor/agent-runner-cleanup` |
   | CI / tooling | `ci/` | `ci/add-clippy-job` |

### Pull Request Workflow

1. **Keep PRs focused.** One logical change per PR. Separate refactors from features.
2. **Write a clear description.** Explain what changed and why, not just what the diff shows.
3. **Reference the issue.** Use `Closes #123` or `Related to #123` in the PR body.
4. **Pass all CI checks** before requesting review.
5. **Respond to review comments** within a reasonable time. Stale PRs may be closed after 30 days of inactivity.

**FFI changes require extra steps.** If you add, remove, or change any `#[uniffi::export]` item in `oxcer_ffi/src/lib.rs`, you must regenerate the Swift bindings before opening the PR:

```bash
./scripts/regen-ffi.sh
git add oxcer_ffi/src/lib.rs \
        apps/OxcerLauncher/OxcerLauncher/oxcer_ffi.swift \
        apps/OxcerLauncher/OxcerLauncher/oxcer_ffiFFI.h
```

CI enforces binding freshness; PRs with stale bindings will fail automatically.

---

## Code Style

### Rust

**Formatting:** All Rust code must be formatted with `rustfmt`. The CI `lint` job runs:

```bash
cargo fmt --check
```

To auto-format before committing:

```bash
cargo fmt
```

**Linting:** The CI `lint` job also runs Clippy with warnings promoted to errors:

```bash
cargo clippy -- -D warnings
```

Fix all Clippy warnings before opening a PR. If a lint is a false positive, suppress it with `#[allow(...)]` and a comment explaining why.

**Style notes:**
- Prefer `thiserror` for error types in library crates; avoid `Box<dyn Error>` at API boundaries.
- Keep `pub` surface minimal — expose only what callers need.
- Use `tracing::` macros (not `println!` or `eprintln!`) for diagnostics. The structured JSON subscriber is initialised by `ensure_logging_init()` in `oxcer_ffi`. Control verbosity with `OXCER_LOG=debug`.
- All paths handed to the LLM must pass through `scrub_for_llm_call`. Do not bypass the data sensitivity pipeline.

### Swift / SwiftUI

**Formatting:** Install [SwiftFormat](https://github.com/nicklockwood/SwiftFormat) and run it before committing:

```bash
brew install swiftformat
swiftformat apps/OxcerLauncher/OxcerLauncher/
```

A `.swiftformat` config file at the repo root sets project-wide rules.

**Style notes:**
- All view state that must survive re-renders belongs in a `@StateObject` or `@ObservedObject`, not in local `@State` on a parent view.
- Views that observe `ObservableObject` properties must use `@ObservedObject`, not `let`. Plain `let` on a struct view does not subscribe to `objectWillChange`.
- Prefer `os.Logger` over `print()` for all diagnostics. Use `.debug()` for verbose output (compiled out in Release at Info level), `.info()` for lifecycle events, `.error()` for failures.
- Avoid `.id()` as a "refresh key" on views that own `@StateObject`; it destroys the subtree and recreates state.

---

## Network Access Policy

**Oxcer does not make arbitrary HTTP requests and is not a web-browsing agent.**

This is a deliberate design constraint, not an oversight. Understanding it will save you from opening a PR that cannot be accepted.

### What Oxcer does not do

- Oxcer has no general-purpose HTTP client or fetch capability.
- It cannot browse URLs, scrape HTML, or retrieve arbitrary web content.
- No tool in the agent loop (`fs_list_dir`, `fs_read_file`, `shell_run`, etc.) makes outbound network calls.
- The local inference path (`LlamaCppPhiRuntime` via llama.cpp + Metal) is fully offline. No token or prompt data leaves the machine during inference.

### Why

- **Security.** An open HTTP fetch capability creates a vector for data exfiltration and SSRF. Keeping Oxcer offline-by-default ensures that no file content, credential fragment, or user query can be sent to an attacker-controlled endpoint by a manipulated prompt.
- **Scope.** Oxcer is a file-task agent, not a research assistant. Adding web access would require a new category of permission, trust boundary, and policy review that is out of scope for this project at this stage.

### What is permitted (future)

Network access may be added in two narrowly scoped forms:

1. **Model download.** `ensure_local_model()` fetches the GGUF file from a fixed, pinned URL over HTTPS on first run, with user awareness. This is the only current outbound call and it is not triggered by the agent loop.
2. **Cloud model APIs.** A future optional cloud backend (see [ROADMAP.md](ROADMAP.md)) may allow the agent to call a specific model API (Gemini, Anthropic, OpenAI, Grok) if the user explicitly enables it. Any such integration must pass through the same scrubbing and guardrails as local inference. It will be documented and opt-in, not on by default.

### Contributor guidance

Do not add:
- HTTP client code (`reqwest`, `URLSession`, `curl` subprocess) outside of the model-download path without opening an issue and getting explicit agreement first.
- Any tool intent that fetches URLs or executes web requests.
- Shell commands in tests or examples that make outbound connections.

If you are proposing a feature that requires network access, describe the exact scope, endpoint set, and security controls in your issue before writing code.

---

## Testing

**Before opening a PR, run the full Rust test suite:**

```bash
cargo test -p oxcer-core    # unit + integration tests for the core
cargo test -p oxcer_ffi     # FFI contract tests
```

All tests must pass. Do not open a PR with failing tests unless the failure is in existing code and you are explicitly fixing it (explain this in the PR description).

**Adding tests:**

- New Rust functionality in `oxcer-core` must include unit tests in the same file or in `oxcer-core/tests/`.
- New FFI exports must include a round-trip test in `oxcer_ffi/src/lib.rs` `#[cfg(test)]`.
- For Swift: add `XCTest` cases in `apps/OxcerLauncher/OxcerLauncherTests/` if the change affects view model logic or FFI wiring.

**macOS app build check:**

After making Swift or Xcode project changes, verify the app builds cleanly:

```bash
cargo build --release -p oxcer_ffi   # must succeed first
xcodebuild -project apps/OxcerLauncher/OxcerLauncher.xcodeproj \
           -scheme OxcerLauncher \
           -destination 'platform=macOS' \
           build
```

---

## Commit Conventions

We use a lightweight prefix scheme inspired by [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <short summary>
```

**Types:**

| Type | When to use |
|---|---|
| `feat` | A new user-visible feature |
| `fix` | A bug fix |
| `refactor` | Code change with no behaviour change |
| `test` | Adding or fixing tests |
| `docs` | Documentation only |
| `ci` | CI/CD changes |
| `chore` | Maintenance (dependency bumps, cleanup) |
| `ffi` | FFI contract change (always regenerate bindings) |

**Scope** is optional but helpful: `ffi`, `agent`, `security`, `swift`, `llm`, `ci`.

**Examples:**

```
feat(agent): add session pin and rename to sidebar
fix(ffi): correct list_workspaces return type to prevent 88 GB VM spike
refactor(security): extract scrub_for_llm_call into standalone module
ffi(orchestrator): expose ffi_agent_step with session JSON opaque blob
docs: replace internal sprint notes with public architecture guide
ci: add cargo clippy and Swift build jobs
```

- **Summary line:** ≤ 72 characters, present tense, no trailing period.
- **Body:** Optional. Use it to explain *why*, not *what*.
- **Breaking changes:** Add `BREAKING CHANGE:` in the commit body (or `!` after the type) and describe the migration path.

---

## License

By submitting a pull request you agree that your contribution will be licensed under the same license as this project (see [LICENSE](LICENSE)).

If your organisation requires a Contributor License Agreement (CLA), one will be linked here when available.
