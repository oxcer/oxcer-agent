# Project overview

- Project name: Oxcer
- Type: Local, security-focused agentic AI desktop app
- Tech stack:
  - Rust core (agent orchestration, FFI)
  - Swift / SwiftUI macOS app (OxcerLauncher)
  - UniFFI-based Rust ↔ Swift FFI, attribute macros (no .udl file)
- Priorities:
  1. Safety and correctness over speed
  2. Predictable resource usage (no surprise memory explosions)
  3. Clear, maintainable architecture and tests

# How to talk to me

- You can assume I am an experienced Rust/Swift developer.
- Use concise, technical explanations.
- When in doubt, prefer code + a short comment over long prose.
- Always show file paths when proposing edits (e.g. `src/lib.rs`, `apps/OxcerLauncher/.../OxcerBackend.swift`).

# Output style

1. Do NOT use emojis in any code, comments, logs, documentation, commit messages, or test names generated for this project.
2. Use a neutral, professional tone. Avoid overly dramatic or roleplay-style narration.
3. Keep responses focused and structured with clear headings or bullet points when needed.
4. For non-trivial changes, first propose a short plan, then show concrete diffs.
5. Prefer explicit, descriptive names over cute or humorous ones.

# Rust guidelines

1. Follow idiomatic Rust (Rust 2021 edition), favoring clarity over cleverness.
2. When touching FFI-facing code:
   - Keep function signatures simple and explicit.
   - Avoid unnecessary generic complexity at the FFI boundary.
   - Prefer `Result<T, OxcerError>` for fallible FFI functions.
3. Always update or add tests when changing behavior:
   - `cargo test -p oxcer_ffi`
   - Add focused unit tests rather than giant integration tests where possible.

# Swift / macOS app guidelines

1. Target modern Swift and Swift Concurrency (async/await) where appropriate.
2. Keep the UI thin: heavy logic should live in the Rust core or well-defined Swift service layers.
3. When modifying FFI-facing Swift code:
   - Use the generated `oxcer_ffi.swift` as the single source of truth.
   - Do not create additional manual copies of the bindings.

# FFI and safety rules

1. Treat the Rust ↔ Swift (UniFFI) boundary as a critical safety zone.
2. Never change FFI function signatures or struct layouts without:
   - Running `./scripts/regen-ffi.sh` to regenerate bindings.
   - Running all FFI tests and memory sentinel tests (OxcerFFITests, ffi_validation).
3. For `list_workspaces` and `WorkspaceInfo`:
   - Follow the staged migration pattern used during the “88GB virtual memory incident”:
     - primitive → single struct → `Vec<WorkspaceInfo>` → real implementation.
4. If a change could affect memory usage or allocation patterns:
   - Propose a phased rollout and explicit tests (including memory sentinel checks) before applying it.

# Testing and CI expectations

1. Before suggesting large refactors, check existing tests and CI configuration:
   - `cargo test -p oxcer_ffi`
   - Swift tests under `apps/OxcerLauncher/.../OxcerFFITests.swift`
2. When adding new functionality:
   - Add or update tests in both Rust and Swift layers where appropriate.
3. If a change requires new scripts or CI steps:
   - Propose the script content (e.g. `scripts/regen-ffi.sh`, `scripts/check-ffi-freshness.sh`).
   - Explain how to wire it into Git hooks or CI YAML.

# Change management

1. Prefer small, reviewable changes grouped by intent (e.g. “FFI contract cleanup”, “test coverage improvements”).
2. When proposing multi-file edits:
   - Summarize the plan first (high level).
   - Then show per-file diffs or snippets.
3. Always call out any behavior changes that might affect:
   - Memory usage
   - Error handling
   - FFI contract or external APIs

# Things to avoid

1. No emojis anywhere in generated output for this repo.
2. No fictional or roleplay-style text in code, comments, or tests.
3. Do not introduce new external dependencies without explicitly discussing trade-offs.
4. Do not silently change FFI contracts or generated binding files.
