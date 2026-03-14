<!--
  Thanks for contributing to Oxcer.
  Fill in the sections below. Delete any section that genuinely doesn't apply,
  but err on the side of leaving them in — a brief "N/A" is fine.
  Keep the PR focused: one logical change per PR makes review faster and
  makes it easier to revert if something goes wrong.
-->

## Summary

<!-- One or two sentences. What does this PR do? -->


## Motivation and context

<!--
  Why is this change needed? Link to the relevant issue(s) if any.
  e.g. "Closes #42 — FFI freshness check was only diffing oxcer_ffi.swift,
  not the C bridging header, so stale headers slipped through CI."
-->


## Implementation notes

<!--
  Brief description of the approach taken and any non-obvious decisions.
  If you chose one design over another, say why.
  Keep this scannable — bullet points are fine.
-->


## Testing

<!--
  Describe how you tested the change. Check every box that applies and
  note the result. If a check is not applicable, mark it [N/A] with a brief reason.
-->

**Rust core**
```bash
cargo test -p oxcer-core
cargo test -p oxcer_ffi
cargo check --workspace --locked
```

**Lint**
```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

**macOS build** *(required for changes touching Swift, FFI, or Xcode project)*
```bash
cargo build --release -p oxcer_ffi
xcodebuild -project apps/OxcerLauncher/OxcerLauncher.xcodeproj \
           -scheme OxcerLauncher -destination 'platform=macOS' \
           -configuration Debug build CODE_SIGNING_ALLOWED=NO
```

**FFI binding freshness** *(required if `oxcer_ffi/src/lib.rs` changed)*
```bash
./scripts/regen-ffi.sh
# Verify no diff; commit both generated files if they changed.
```

**Additional tests or manual verification:**

<!-- Describe any manual steps, demo workflows (Workflow 1/2/3), or edge cases tested. -->


## Security and privacy considerations

<!--
  Oxcer's core promise: nothing leaves your machine.
  Answer each question with one sentence or "Not affected."
-->

- **Does this change what files or data Oxcer reads?**
- **Does this change what files or data Oxcer writes?**
- **Does this affect any outbound network calls?** (The only permitted outbound call is model download in `ensure_local_model`; see CONTRIBUTING.md § Network Access Policy.)
- **Does this change what content is sent to the LLM, or bypass `scrub_for_llm_call`?**
- **Does this affect the policy engine, HITL approval gate, or tool permissions?**


## UX impact

<!--
  Oxcer is designed for non-developer users. If this change affects the UI,
  chat behaviour, or any user-visible text, describe the before/after.
  If not applicable, delete this section.
-->


## Documentation and release notes

<!--
  List any docs that need updating. If this is a user-visible change,
  a line in CHANGELOG.md (or a note here that one is needed) is expected.
-->

- [ ] `docs/` updated if architecture or behaviour changed
- [ ] `CHANGELOG.md` entry added (or N/A for internal/infra changes)
- [ ] ROADMAP.md updated if a milestone was completed or scope changed


## Checklist

- [ ] I have run the relevant tests listed above and they pass.
- [ ] I have considered security and privacy impact and noted it above.
- [ ] This PR is scoped to one logical change and does not mix unrelated work.
- [ ] CI is green (or I have explained any expected failures above).
