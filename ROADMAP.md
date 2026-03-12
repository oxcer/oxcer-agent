# Roadmap

Oxcer is a local-first AI agent for macOS that reads files you name and acts on them — privately, on-device, without sending data to any server. The goal is to make common file-based tasks (summarize, organize, find) as simple as describing them in plain English.

This is a directional plan, not a commitment. Priorities may shift based on contributor interest and user feedback.

---

## Now — v0.1 (stable)

One workflow is officially supported and has been validated end-to-end on Apple Silicon:

- **Named-file summarization.** Type `Summarize Test1_doc.md in Downloads` and Oxcer reads the file, runs local inference via Llama-3 8B on Metal, and returns a summary. No internet required.
- **Human-in-the-loop approval.** Destructive and write operations (delete, move, write, shell) require explicit user approval. Read-only access to the file you named does not.
- **Data sensitivity scrubbing.** Credentials, API keys, JWTs, and PEM keys are redacted before any LLM call.
- **Multi-session chat.** Sidebar with unlimited sessions; pin, rename, delete.

**What is explicitly out of scope for v0.1:** multi-file batch operations, file organization, intent routing via a model classifier, streaming output, and cloud model backends.

---

## v0.2 — Multi-file summary

Summarize a set of named files into a single overview in one request.

- Accumulate file content across multiple `FsReadFile` steps (context-budget handling for the 8K token limit).
- Dynamic plan expansion: after listing a directory, splice one `FsReadFile` per matching file into the plan before the final `LlmGenerate` step.
- File pattern matching so `"summarize all Test2_doc reports in Downloads"` resolves without listing everything.
- Truncation notice when total content exceeds the context window.

> The Workflow 2 code path exists in `oxcer-core/src/orchestrator/planning.rs` and is disabled with `#[allow(dead_code)]`. It will be enabled once validated end-to-end.

---

## v0.3 — File organization

Move a set of files from one location to another by describing the operation in plain English.

- `FsCreateDir` (idempotent): create the destination folder if it does not exist.
- Pattern-matched `FsMove` fan-out: one move step per matching file.
- Confirmation step: the agent describes what it will move before doing it; user approves.
- Scope limited to workspace directories configured in Settings.

> The Workflow 3 code path exists and is disabled alongside Workflow 2. Both share the same dynamic plan-expansion infrastructure.

---

## v0.4 — LLM-backed intent routing

Replace the current string-match heuristics in the semantic router with a small, fast model-based classifier.

- Phrasing variation is handled gracefully: `"give me a summary of the thing I just downloaded"` triggers the same workflow as naming a file explicitly.
- Reduced false positives on ambiguous requests.
- Classifier runs locally and is under the same privacy guarantees as the main model.

---

## v0.5 — Streaming output

Stream model output token-by-token to the UI.

- Long summaries appear incrementally instead of all at once.
- Stop button cancels mid-stream.
- Requires a streaming-capable interface from `llama-cpp-2`; the runtime already supports per-token callbacks internally.

---

## Later / Community interest

These are on the radar but have no timeline commitment.

### Eval harness

Deterministic test fixtures for Workflow 1–3 so regressions are caught in CI, not in manual testing. Each fixture is a (task description, file set, expected plan) triple that can be run with `cargo test`.

### Plugin system

YAML-defined shell and file-indexer plugins that extend the tool catalog. The loader infrastructure exists in `oxcer-core`; wiring plugin-derived tool intents into the planner is the remaining work.

### Platforms & runtimes

- **Windows launcher (WinUI 3).** The target platform for the Windows launcher is WinUI 3, not Tauri. The Rust core is already platform-agnostic; the gap is a native Windows UI shell. A Tauri shell exists in `apps/desktop-tauri/` as a backend-only stub but is not the intended Windows distribution target.
- **Linux headless mode.** A CLI or minimal GTK/Qt wrapper for Linux, oriented toward headless and server use cases.
- **GPU-accelerated and ONNX runtime options.** Alternative inference backends for users who want broader hardware support or a runtime other than llama.cpp. Metal (Apple Silicon) is the current GPU path; DirectML/CUDA on Windows and ONNX runtime are candidates.

### Model backends

Oxcer's architecture cleanly separates the agent loop from the inference backend, so the same orchestrator can drive a local or remote model without changing the planner, tools, or security layer. A future optional cloud backend is architecturally feasible, but two conditions must hold before any remote model is treated as a first-class supported backend:

1. **Semantic routing is implemented.** A model-based classifier (see v0.4) must be able to choose between local and remote inference in a principled way — not just a settings toggle that bypasses routing.
2. **Full workflow parity is verified.** The same Oxcer workflows (Workflow 1 named-file summary, Workflow 2 multi-file summary, Workflow 3 file organization) must pass end-to-end with the remote backend, including guardrail checks, HITL approval, and DLP scrubbing. "It returns some text" is not sufficient.

Candidate APIs: Gemini, Grok, Anthropic (Claude), OpenAI. All would be opt-in, require explicit user configuration, and pass through the same `scrub_for_llm_call` pipeline as local inference.

- **Smaller local model.** Phi-3 Mini or Gemma 2B as a faster, lower-memory alternative for users with 8 GB RAM or for simple tasks where the full 8B model is unnecessary.

---

## How to get involved

**Use it and report what breaks.** The most useful contributions right now are bug reports on Workflow 1 edge cases — unusual file types, long files, non-ASCII filenames, large Documents/Downloads folders.

**Good first issues** are labeled [`good first issue`](../../issues?q=label%3A%22good+first+issue%22) in the issue tracker.

**Areas where contributors can help most:**
- Eval fixtures for Workflow 1 (reproducible test files + expected output).
- Windows launcher (WinUI 3).
- Streaming output wiring in `oxcer-core` and `OxcerLauncher`.
- Smaller local model support and benchmarking.

Before opening a pull request, read [CONTRIBUTING.md](CONTRIBUTING.md).

---

*Last updated: v0.1.0 release.*
