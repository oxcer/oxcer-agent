# Licenses

## Oxcer

Oxcer source code is distributed under the **MIT License**.

See [LICENSE](LICENSE) for the full text.

---

## Meta Llama 3 (default inference model)

> **Built with Meta Llama 3**

The default local inference model used by Oxcer is **Meta Llama 3 8B Instruct**. The model weights are **not included** in this source repository. However, when Oxcer is distributed as a DMG or package, it may include the GGUF model weights inside the application bundle or download them automatically on first launch. In either case, Oxcer is acting as a redistributor of Llama Materials under the terms of the Meta Llama 3 Community License.

### Required attribution

> "Meta Llama 3 is licensed under the Meta Llama 3 Community License,
> Copyright © Meta Platforms, Inc. All Rights Reserved."

### License and acceptable use

Meta Llama 3 is distributed under the
**[Meta Llama 3 Community License Agreement](https://llama.meta.com/llama3/license/)**.

Key obligations for anyone downloading and using Meta Llama 3:

| Obligation | Requirement |
|---|---|
| Attribution | Display "Built with Meta Llama 3" prominently on any related website, UI, blog post, about page, or product documentation |
| Copyright notice | Include the copyright notice above in any distribution of Llama materials |
| Redistribution | Provide a copy of the Meta Llama 3 Community License with any distribution that includes Llama materials |
| Acceptable use | Comply with the [Meta Llama 3 Acceptable Use Policy](https://llama.meta.com/llama3/use-policy/) |
| Large-scale commercial use | If your product or service exceeds 700 million monthly active users, you must obtain a separate license from Meta |
| Derivative models | Any AI model trained on or derived from Llama 3 must include "Llama 3" at the beginning of its name |

### What this means for Oxcer users

- **Oxcer source code** is MIT-licensed. You may use, copy, modify, and distribute the Oxcer source code under the MIT License without restriction.
- **The Llama 3 model weights** are subject to the Meta Llama 3 Community License. By accepting the in-app first-run consent screen you acknowledge this license and agree to its terms, including the [Acceptable Use Policy](https://llama.meta.com/llama3/use-policy/).
- **Oxcer as a distributed application** (DMG/package) includes or downloads Llama Materials and therefore must — and does — comply with all redistribution obligations in the Meta Llama 3 Community License, including:
  - Displaying "Built with Meta Llama 3" prominently in the UI (Settings → About).
  - Including the full license text in the application bundle (`LLAMA3_LICENSE.txt`).
  - Presenting a first-run consent screen before any model use or download.

### Where to obtain the model

- **Official** (recommended): [meta-llama/Meta-Llama-3-8B-Instruct](https://huggingface.co/meta-llama/Meta-Llama-3-8B-Instruct) on Hugging Face — requires accepting the Community License directly with Meta.
- **Community GGUF quantizations** (third-party, not affiliated with Meta): [bartowski/Meta-Llama-3-8B-Instruct-GGUF](https://huggingface.co/bartowski/Meta-Llama-3-8B-Instruct-GGUF) — still subject to the same Meta Llama 3 Community License terms.

---

## Third-party Rust and Swift dependencies

Oxcer's Rust workspace and Swift app depend on open-source libraries. A full list of Rust crate licenses can be generated with:

```bash
cargo install cargo-license
cargo license
```

Notable dependencies and their licenses:

| Dependency | License | Notes |
|---|---|---|
| [llama.cpp](https://github.com/ggerganov/llama.cpp) | MIT | Bundled via `llama-cpp-sys-2` |
| [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs) | MIT | Rust bindings to llama.cpp |
| [uniffi](https://github.com/mozilla/uniffi-rs) | MPL-2.0 | Rust → Swift FFI codegen |
| [tracing](https://github.com/tokio-rs/tracing) | MIT | Structured logging |
| [serde](https://github.com/serde-rs/serde) | MIT / Apache-2.0 | Serialization |

The full dependency tree and licenses are in `Cargo.lock`. None of the bundled dependencies impose restrictions on the Oxcer source code beyond what their individual licenses state.
