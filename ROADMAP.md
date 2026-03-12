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

These are on the radar but have no timeline commitment:

| Area | Description |
|------|-------------|
| **Eval harness** | Deterministic test fixtures for Workflow 1–3 so regressions are caught in CI, not in manual testing. |
| **Windows / Linux** | OxcerLauncher on Windows (WinUI 3 or Tauri) and a headless Linux mode. The Rust core is platform-agnostic; the gap is the launcher UI. |
| **Cloud model toggle** | Optional cloud backend (OpenAI, Anthropic API) as an alternative to the local model for users who prefer it. Requires explicit opt-in and scrubbing guarantees. |
| **Smaller local model** | Phi-3 Mini or Gemma 2B as a faster, lower-memory alternative for simple tasks on machines with 8 GB RAM. |
| **Plugin system** | YAML-defined shell and file-indexer plugins that extend the tool catalog. Infrastructure exists; wiring into the agent loop is the remaining work. |

---

## How to get involved

**Use it and report what breaks.** The most useful contributions right now are bug reports on Workflow 1 edge cases — unusual file types, long files, non-ASCII filenames, large Documents/Downloads folders.

**Good first issues** are labeled [`good first issue`](../../issues?q=label%3A%22good+first+issue%22) in the issue tracker.

**Areas where contributors can help most:**
- Eval fixtures for Workflow 1 (reproducible test files + expected output).
- Windows launcher (WinUI 3 or Tauri shell).
- Streaming output wiring in `oxcer-core` and `OxcerLauncher`.
- Smaller local model support and benchmarking.

Before opening a pull request, read [CONTRIBUTING.md](CONTRIBUTING.md).

---

*Last updated: v0.1.0 release.*
