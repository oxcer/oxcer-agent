# Architecture

> **Read this if** you want to understand how Oxcer's components fit together before reading or modifying the code. Start here if you are new to the codebase.

> **Preview — v0.1.0:** This document describes the current macOS implementation. The runtime described here has been validated on Apple Silicon (M1 and later) only. Intel macOS builds are expected to work but are not regularly tested. Windows and Linux launchers are stubs and not yet functional. Architecture details may change as the project evolves toward a stable release.

Oxcer is a local-first AI agent for macOS. All computation happens on-device; no data is sent to external servers.

---

## Component Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  OxcerLauncher  (macOS SwiftUI app)                             │
│                                                                 │
│  SidebarView ── SessionListRow ── DetailView                    │
│       │                               │                         │
│  AppViewModel ──────────────── AgentRunner ── SwiftAgentExecutor│
│       │                           │                             │
│  OxcerBackend (protocol)     ApprovalGate                       │
└────────────────────┬────────────────────────────────────────────┘
                     │  UniFFI (C ABI)
                     │  liboxcer_ffi.dylib
┌────────────────────┴────────────────────────────────────────────┐
│  oxcer_ffi  (Rust FFI crate)                                    │
│                                                                 │
│  ffi_agent_step()   generate_text()   list_workspaces()         │
└────────────────────┬────────────────────────────────────────────┘
                     │
┌────────────────────┴────────────────────────────────────────────┐
│  oxcer-core  (pure Rust library)                                │
│                                                                 │
│  Orchestrator ── SemanticRouter ── SecurityPolicy               │
│       │                                  │                      │
│  LlmEngine ── LlamaCppPhiRuntime    PromptScrubber              │
│       │            (Metal)          DataSensitivity             │
│  Tools: fs, shell, plugin loader                                │
└─────────────────────────────────────────────────────────────────┘
```

---

## Layers

### OxcerLauncher (SwiftUI, macOS)

The native macOS application. All UI runs in SwiftUI; there are no WebViews or HTML pages.

Key types:

| Type | Responsibility |
|---|---|
| `AppViewModel` | Session list, sending messages, stopping generation |
| `ConversationSession` | Per-session state: messages, streaming buffer, approval gate, task handle |
| `AgentRunner` | Drives the `ffi_agent_step` loop (up to 20 steps); handles approval requests |
| `SwiftAgentExecutor` | Executes each `FfiToolIntent`: filesystem ops, shell commands, LLM generation |
| `OxcerBackend` (protocol) | Abstraction over the FFI layer; enables testing without a live dylib |

The UI and the agent loop interact through `ConversationSession` only. `AppViewModel` writes to the session; views observe it via `@ObservedObject`. This keeps re-render scope tight.

### oxcer_ffi (Rust FFI crate)

Exposes the Rust core to Swift via [UniFFI](https://mozilla.github.io/uniffi-rs/) (attribute-based, no `.udl` file). The generated Swift wrappers and C header are committed to the repository and must be regenerated whenever the public Rust API changes.

Primary exported functions:

| Function | Description |
|---|---|
| `ffi_agent_step(env, session_json, last_result)` | Single step of the agent loop; returns status `need_tool`, `complete`, or `awaiting_approval` |
| `generate_text(prompt)` | One-shot LLM call; used by the `llm_generate` tool intent |
| `list_workspaces(app_config_dir)` | Returns the workspace list from `config.json` |

The `session_json` field is an **opaque blob**: Swift passes it back unchanged on every step. It encodes the full agent session state; Swift never reads or modifies its contents.

### oxcer-core (pure Rust library)

No Tauri, no FFI, no async runtime. All logic is synchronous and independently testable with `cargo test -p oxcer-core`.

Key modules:

| Module | Responsibility |
|---|---|
| `orchestrator` | Agent session state machine: `start_session`, `agent_step`, `next_action` |
| `semantic_router` | Routes tasks to a strategy: `CheapModel`, `ExpensiveModel`, or `ToolsOnly` |
| `llm/local_phi3` | `LlamaCppPhiRuntime` — GGUF model loader with Metal acceleration; default model: Meta Llama 3 8B Instruct |
| `security/policy_engine` | Evaluates `PolicyRequest` → `Allow / Deny / RequireApproval` |
| `data_sensitivity` | DLP classifier: 14 regex rules covering credentials, keys, tokens |
| `prompt_sanitizer` | Redacts sensitive findings before any LLM call; blocks on hard never-send rules |
| `plugins/loader` | Loads YAML plugin descriptors; merges into command catalog and capability registry (loaded at startup; not wired into the agent loop in v0.1) |

---

## Agent Loop

Each user message drives one complete run of the loop:

```
User sends message
        │
AppViewModel.sendMessage()
        │
AgentRunner.run(env)   ← loop starts (max 20 steps)
        │
        ├── ffi_agent_step(env, session_json=nil, last_result=nil)
        │         └─ Orchestrator.start_session → builds plan
        │                  │
        │            status == "need_tool"
        │                  │
        │         ┌── approval required? ──┐
        │         │  yes                   │  no
        │         │                        │
        │   show ApprovalBubble     executor.execute(intent)
        │   await user decision            │
        │         │                        │
        │         └────────────────────────┘
        │                  │
        │         ffi_agent_step(env, session_json, lastResult)
        │                  │
        │            status == "complete"
        │
ConversationSession.finalizeStreaming()
        │
Message appended to chat
```

The loop runs entirely on a detached `Task`; the main actor is only touched to update `@Published` properties. Cancellation (Stop button) is handled via `Task.cancel()` + `Task.checkCancellation()` at the top of each step.

---

## FFI Binding Lifecycle

When the Rust API in `oxcer_ffi/src/lib.rs` changes, bindings must be regenerated:

```bash
./scripts/regen-ffi.sh
```

This rebuilds the release dylib, runs `uniffi-bindgen`, diffs, and copies both generated files into the Xcode project. CI enforces that the committed bindings exactly match what the current Rust source would produce.

See [docs/DEVELOPMENT.md](DEVELOPMENT.md) for the full workflow.

---

## Security Model

Destructive and write operations (delete, move, rename, write, shell) require explicit user approval before execution. Read-only operations (`fs_list_dir`, `fs_read_file`) on files the user named do not require a separate approval step. The agent is treated as an untrusted client — it can propose actions, but it cannot execute destructive ones without the user's consent.

See [docs/security.md](security.md) for the full security architecture.
