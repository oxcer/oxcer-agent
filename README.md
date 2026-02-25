# Oxcer

<p align="center">
  <img src="readme-main.png" alt="Oxcer app screenshot" width="800">
</p>

**Oxcer** is a local-first AI assistant. Reading files, generating docs, running shell commands, and reasoning. Nothing leaves your machine.

> **Status: Early Access — v0.1.0**
> Oxcer is under active development. Features, UI, and the configuration schema may change without notice between releases. Not yet recommended for production or mission-critical use.

---

## Features

| Feature | Detail |
|---|---|
| **On-device LLM** | Meta Llama 3 8B Instruct (Q4 GGUF) via llama.cpp + Metal |
| **Multi-session chat** | Sidebar with unlimited sessions; pin, rename, delete |
| **Agent tool loop** | `fs_list_dir`, `fs_read_file`, `fs_write_file`, `fs_delete`, `fs_rename`, `fs_move`, `shell_run` |
| **Human-in-the-loop** | Every filesystem and shell action requires explicit approval before execution |
| **Data sensitivity** | Pre-prompt DLP scanner redacts credentials, API keys, JWTs, and PEM keys before any LLM call |
| **Structured logging** | JSON tracing (Rust) + `os.Logger` (Swift), filterable with `jq` or Console.app |
| **Light / Dark / System theme** | Follows macOS appearance or can be forced |

---

## Supported Platforms

| Platform | Support level |
|---|---|
| macOS (Apple Silicon, M1 and later) | Primary target. Developed and regularly tested. |
| macOS (Intel) | Best-effort. Builds and runs, but not regularly tested. |
| Windows | Planned. Not available in this release. |
| Linux | On the roadmap. No timeline committed. |

Oxcer 0.1.0 has been developed and validated exclusively on Apple Silicon Macs. Intel macOS builds are expected to work but are not part of the regular test cycle. Windows and Linux are not supported in this early access release.

The Windows launcher stub (`apps/windows-launcher/`) exists in the repository but is not functional. Contributions toward Windows and Linux support are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).

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

You also need a GGUF model file. The default model is **Meta Llama 3 8B Instruct** (~4.7 GB, Q4\_K\_M quantization).

**Official source (recommended):** Download from the [meta-llama/Meta-Llama-3-8B-Instruct](https://huggingface.co/meta-llama/Meta-Llama-3-8B-Instruct) repository on Hugging Face. You must accept the Meta Llama 3 Community License on the Hugging Face model page before downloading. Place the `.gguf` file anywhere on disk; you will point the app to it on first launch.

**Alternative — community GGUF quantizations** (third-party, not affiliated with Meta): pre-quantized builds such as [`bartowski/Meta-Llama-3-8B-Instruct-GGUF`](https://huggingface.co/bartowski/Meta-Llama-3-8B-Instruct-GGUF) are an option if you do not want to quantize the model yourself. These are still subject to the same Meta Llama 3 Community License.

> **Model license:** Meta Llama 3 is distributed under the [Meta Llama 3 Community License](https://llama.meta.com/llama3/license/), which is separate from Oxcer's MIT license. By downloading and using the model you agree to its terms. See [LICENSES.md](LICENSES.md) for details.

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

On first launch, open **Settings** and point Oxcer at:

- Your GGUF model file.
- One or more workspace folders (directories the agent is allowed to read and write).

Configuration is stored in `~/Library/Application Support/Oxcer/config.json`.

---

## Project Layout

```
oxcer-core/          # Pure Rust library: agent orchestrator, LLM engine, security, tools
oxcer_ffi/           # Rust → Swift FFI layer (UniFFI, attribute-based)
apps/
  OxcerLauncher/     # macOS SwiftUI app (primary UI target)
  desktop-tauri/     # Cross-platform Tauri shell (backend-only, no UI)
  windows-launcher/  # Planned WinUI 3 launcher (stub)
plugins/             # YAML plugin definitions
config/              # Policies and defaults
docs/                # Architecture, development, and security docs
scripts/             # regen-ffi.sh and other dev helpers
```

---

## Documentation

| Document | Description |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Component overview, agent loop, FFI bridge |
| [docs/development.md](docs/development.md) | Build system, testing, FFI workflow |
| [docs/security.md](docs/security.md) | Security model, policy engine, HITL approval |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to contribute, code style, PR workflow |

---

## Running Tests

```bash
# Rust core unit and integration tests
cargo test -p oxcer-core

# FFI contract tests
cargo test -p oxcer_ffi

# Full workspace check
cargo check --workspace
```

---

## Current Limitations

- **macOS only.** The Windows and Linux launchers are stubs; only OxcerLauncher is functional.
- **Single local model.** Cloud model backends exist in the codebase but are not wired to the agent loop in this release.
- **Model file not bundled.** You must download the GGUF separately and configure the path in Settings.
- **No app store distribution.** The app is unsigned for local development; distributable builds require a Developer ID certificate.

---

## Roadmap (High Level)

This is a high-level view of direction, not a commitment. Priorities may shift.

| Milestone | Description |
|---|---|
| **v0.x — macOS stability** | Harden error handling, conversation management, model management UI, and test coverage |
| **Additional backends** | GPU-accelerated and ONNX runtime options beyond llama.cpp; cloud model toggle wired to agent loop |
| **Windows support** | Bring OxcerLauncher to functional parity on Windows (WinUI 3 or Tauri shell) |
| **Linux support** | Headless mode and a native Linux launcher |
| **Plugin marketplace** | Discovery and installation of community plugins from the UI |

---

## Built with Meta Llama 3

Oxcer uses Meta Llama 3 as its default local inference model.
"Meta Llama 3 is licensed under the Meta Llama 3 Community License, Copyright © Meta Platforms, Inc. All Rights Reserved."

---

## Acknowledgements

- [OpenClaw](https://github.com/openclaw) — original concept and architecture
- [NVIDIA NeMo Guardrails](https://github.com/NVIDIA/NeMo-Guardrails) and [vLLM](https://github.com/vllm-project/vllm) — guardrails and semantic router design
- [llama.cpp](https://github.com/ggerganov/llama.cpp) and the [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs) Rust bindings
- [Meta Llama 3](https://llama.meta.com/) — default inference model

---

## Licensing

| Component | License |
|---|---|
| Oxcer source code | [MIT](LICENSE) |
| Meta Llama 3 model weights | [Meta Llama 3 Community License](https://llama.meta.com/llama3/license/) |

Oxcer source code is MIT-licensed. The model weights are **not** included in this repository. When Oxcer is distributed as a DMG or package it may include or auto-download the GGUF model file, in which case Oxcer is acting as a redistributor of Llama Materials and complies with all obligations under the Meta Llama 3 Community License (first-run consent screen, license file bundled in the app, "Built with Meta Llama 3" attribution in the UI).

Users who download and run Meta Llama 3 must comply with the [Meta Llama 3 Community License](https://llama.meta.com/llama3/license/) and the [Llama 3 Acceptable Use Policy](https://llama.meta.com/llama3/use-policy/).

See [LICENSES.md](LICENSES.md) for the full attribution notice and third-party component details.

