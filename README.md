# Oxcer

<p align="center">
  <img src="readme-main.png" alt="Oxcer app screenshot" width="800">
</p>

**Local-First Document Intelligence for non-developers.**

Oxcer lets you summarize and work with documents on your Mac by describing what you want in plain English. No terminal. No cloud account. No document text ever leaves the machine.

> **v0.1 — Early Access.** One workflow is officially supported: single-file summarization. Read [What Oxcer does in v0.1](#what-oxcer-does-in-v01) before installing. See [ROADMAP.md](ROADMAP.md) for what is coming next, and [docs/whitepaper.pdf](docs/whitepaper.pdf) for the full architecture and design rationale.

---

## What Oxcer does in v0.1

Oxcer reads a file you name — by mentioning the filename and its location — and summarizes it locally using a quantized Llama 3 8B model running on Metal. No internet connection is required during summarization. No text is sent to any server.

### Try it

After completing setup, type:

```
Summarize Test1_doc.md in Downloads
```

Replace `Test1_doc.md` with any `.md` or `.txt` file (or other UTF-8 plain-text format) in your `~/Downloads` folder.

**Supported file types (v0.1):** `.md`, `.txt`, `.csv`, `.json`, `.yaml`, `.yml`, `.log`, `.rst` — any UTF-8 encoded plain-text file. PDF support is planned for v1.0.

**Hardware:** Apple Silicon Mac (M1 or later). 8 GB RAM minimum; 16 GB recommended when running Oxcer alongside other apps.

**Privacy:** All inference runs on-device via llama.cpp + Metal. Oxcer makes no network requests during summarization. All prompt text is scrubbed for credentials, API keys, JWTs, and PEM keys before reaching the model.

---

## Roadmap

See **[ROADMAP.md](ROADMAP.md)** for upcoming milestones and how to contribute.

---

## Features

| Feature | Detail |
|---|---|
| **On-device LLM** | Meta Llama 3 8B Instruct (Q4_K_M GGUF) via llama.cpp + Metal, full GPU offload |
| **Plan-first orchestration** | Deterministic heuristic planner builds a `Vec<ToolCallIntent>` before any tool runs — no ReAct-style on-the-fly decisions |
| **Agent tool loop** | `fs_list_dir`, `fs_read_file`, `fs_write_file`, `fs_delete`, `fs_rename`, `fs_move`, `fs_create_dir`, `shell_run` |
| **Human-in-the-loop** | Write operations (delete, move, write, shell, create dir) require explicit approval. Read-only ops (`fs_list_dir`, `fs_read_file`) do not. |
| **DLP scrubbing** | Pre-prompt scanner redacts credentials, API keys, JWTs, and PEM keys before any LLM call |
| **Narration sanitizer** | Detects and rejects LLM output that describes tool calls instead of summarizing content |
| **Cloud provider opt-in** | OpenAI, Anthropic, Gemini, Grok — configured in Settings, off by default |
| **Multi-session chat** | Sidebar with unlimited sessions; pin, rename, delete |
| **Structured logging** | JSON tracing (Rust) + `os.Logger` (Swift), filterable with `jq` or Console.app; set `OXCER_LOG=debug` for verbose output |
| **Light / Dark / System theme** | Follows macOS appearance or can be forced |

---

## Supported Platforms

| Platform | Support level |
|---|---|
| macOS (Apple Silicon, M1 and later) | Primary target. Developed and regularly tested. |
| macOS (Intel) | Best-effort. Builds and runs; not regularly tested. |
| Windows | Planned. Tauri shell exists as a backend-only stub; native WinUI 3 launcher is the target. |
| Linux | On the roadmap. Rust core is platform-agnostic. No timeline committed. |

---

## Requirements (macOS)

| Dependency | Version |
|---|---|
| macOS | 14 (Sonoma) or later |
| Xcode | 15 or later |
| Rust toolchain | stable (see `rust-toolchain.toml`) |
| CMake | 3.15 or later — required by `llama-cpp-sys` |

Install CMake via Homebrew if you do not already have it:

```bash
brew install cmake
```

You also need a GGUF model file. The default model is **Meta Llama 3 8B Instruct** (~4.9 GB, Q4\_K\_M quantization).

**Official source (recommended):** Download from the [meta-llama/Meta-Llama-3-8B-Instruct](https://huggingface.co/meta-llama/Meta-Llama-3-8B-Instruct) repository on Hugging Face. You must accept the Meta Llama 3 Community License on the model page before downloading.

**Alternative — community GGUF quantizations** (third-party, not affiliated with Meta): pre-quantized builds such as [`bartowski/Meta-Llama-3-8B-Instruct-GGUF`](https://huggingface.co/bartowski/Meta-Llama-3-8B-Instruct-GGUF) are an option if you do not want to quantize the model yourself. Still subject to the Meta Llama 3 Community License.

> **Model license:** Meta Llama 3 is distributed under the [Meta Llama 3 Community License](https://llama.meta.com/llama3/license/), which is separate from Oxcer's MIT license. See [LICENSES.md](LICENSES.md) for details.

---

## Getting Started

### 1. Clone the repository

```bash
git clone https://github.com/your-org/oxcer.git
cd oxcer
```

### 2. Build the Rust core

```bash
cargo build --release -p oxcer_ffi
```

This produces `target/release/liboxcer_ffi.dylib`. The first build compiles llama.cpp via CMake and may take several minutes.

### 3. Open the macOS app in Xcode

```bash
open apps/OxcerLauncher/OxcerLauncher.xcodeproj
```

Select scheme **OxcerLauncher**, destination **My Mac**, then press **⌘R**.

Xcode automatically runs `cargo build --release -p oxcer_ffi` before linking, so subsequent builds only rebuild what changed.

### 4. Configure

On first launch, Oxcer walks you through:

1. Accepting the Meta Llama 3 Community License.
2. Downloading the GGUF model file (~4.9 GB, one-time).
3. Optionally configuring workspace folders (directories the agent is allowed to read and write).

Configuration is stored in `~/Library/Application Support/Oxcer/config.json`.

---

## Project Layout

```
oxcer-core/          # Pure Rust library: agent orchestrator, LLM engine, security, tools
oxcer_ffi/           # Rust → Swift FFI layer (UniFFI 0.28, attribute-based)
apps/
  OxcerLauncher/     # macOS SwiftUI app (primary UI target)
  desktop-tauri/     # Cross-platform Tauri shell (backend-only stub, no production UI)
  windows-launcher/  # Planned WinUI 3 launcher (stub)
plugins/             # YAML plugin definitions
config/              # Policies and defaults
docs/                # Architecture, development, security docs, and whitepaper.pdf
scripts/             # regen-ffi.sh, check-ffi-freshness.sh, and other dev helpers
demo/                # Test files for Workflow 1 (Test1_doc.md) and Workflow 2 (Test2_doc*.md)
```

---

## Documentation

| Document | Description |
|---|---|
| [docs/whitepaper.pdf](docs/whitepaper.pdf) | Architecture, design rationale, and roadmap in full |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Component overview, agent loop, FFI bridge |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Build system, testing, FFI workflow |
| [docs/security.md](docs/security.md) | Security model, policy engine, HITL approval |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to contribute, code style, PR workflow |
| [ROADMAP.md](ROADMAP.md) | Upcoming milestones and how to get involved |

---

## Running Tests

```bash
# Rust core unit and integration tests
cargo test -p oxcer-core

# FFI contract tests (catches stale bindings and wrong return types)
cargo test -p oxcer_ffi

# Full workspace type-check
cargo check --workspace
```

For Swift: run the **OxcerLauncherTests** target in Xcode (⌘U).

---

## Current Limitations

- **macOS only.** Windows and Linux launchers are stubs; only OxcerLauncher is functional.
- **Single local model.** Model switching requires replacing the GGUF file and restarting.
- **No streaming output.** Responses are buffered until generation completes. Streaming is v1.0.
- **No multi-file batch.** Workflow 2 (multi-file summarize) and Workflow 3 (folder operations) are implemented but intentionally disabled pending end-to-end validation.
- **PDF not supported.** v1.0 target.
- **Context window ceiling.** Files larger than ~4 000 characters are truncated before LLM injection (~1 000 tokens, leaving headroom for the prompt frame and generation within the 8 192-token context).
- **Model file not bundled.** You must download the GGUF separately; the app will guide you through this on first launch.
- **No app store distribution.** The app is unsigned for local development; distributable builds require a Developer ID certificate and notarization.

---

## Built with Meta Llama 3

Oxcer uses Meta Llama 3 as its default local inference model.
"Meta Llama 3 is licensed under the Meta Llama 3 Community License, Copyright © Meta Platforms, Inc. All Rights Reserved."

---

## Acknowledgements

- [Claude](https://claude.ai) and [Claude Code](https://github.com/anthropics/claude-code) — assisted with design, prompting, and agent scaffolding throughout the development of Oxcer.
- [llama.cpp](https://github.com/ggerganov/llama.cpp) and the [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs) Rust bindings — local inference backend.
- [Meta Llama 3](https://llama.meta.com/) — default on-device inference model.
- [NVIDIA NeMo Guardrails](https://github.com/NVIDIA/NeMo-Guardrails) and [vLLM](https://github.com/vllm-project/vllm) — influenced the guardrails design and semantic router approach.

---

## Licensing

| Component | License |
|---|---|
| Oxcer source code | [MIT](LICENSE) |
| Meta Llama 3 model weights | [Meta Llama 3 Community License](https://llama.meta.com/llama3/license/) |

Oxcer source code is MIT-licensed. The model weights are **not** included in this repository. When Oxcer downloads or bundles the GGUF model it complies with all obligations under the Meta Llama 3 Community License (first-run consent screen, license file bundled in the app, "Built with Meta Llama 3" attribution in the UI).

Users who download and run Meta Llama 3 must comply with the [Meta Llama 3 Community License](https://llama.meta.com/llama3/license/) and the [Llama 3 Acceptable Use Policy](https://llama.meta.com/llama3/use-policy/).

See [LICENSES.md](LICENSES.md) for the full attribution notice and third-party component details.
