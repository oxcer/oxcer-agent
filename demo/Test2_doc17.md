# The Local LLM Backend

## Model Choice

Oxcer v0.1.0 ships with Meta Llama 3 8B Instruct, quantised to Q4_K_M format (~4.9 GiB on disk) using the bartowski GGUF conversion. The 8B parameter size fits comfortably in the unified memory of a base M-series MacBook and delivers fast inference via Metal GPU offload. The Q4_K_M quantisation level provides a good balance between speed, memory use, and generation quality for the short-answer tasks Oxcer performs.

## llama-cpp-2

The Rust inference wrapper is `llama-cpp-2`, a safe Rust binding over `llama.cpp` compiled with Metal support. The dependency is declared with `default-features = false` to disable OpenMP and Android-specific features. CMake must be installed via Homebrew for the `llama-cpp-sys-2` build script to locate the compiler toolchain during `cargo build`.

## Chat Template

Llama 3 Instruct uses a specific chat template that wraps system and user turns in `<|start_header_id|>` / `<|eot_id|>` tags. The `build_llama3_prompt` function in `runtime.rs` constructs this template correctly and prepends `<|begin_of_text|>` as the first token of the string. `AddBos::Never` is passed to the tokeniser because the BOS token is already embedded in the string literal — passing `AddBos::Always` would duplicate it and confuse the model.

## Context Window Management

The runtime uses a context size of 8 192 tokens and a safety margin of 64 tokens to ensure the generation budget does not overflow the KV cache. `safe_max_tokens` is calculated as `min(params.max_tokens, CONTEXT_LIMIT - prompt_tokens - GENERATION_SAFETY_MARGIN)`. If this value is zero or negative — meaning the prompt is too long to generate any response — the runtime returns an error rather than attempting a zero-token generation.

## System Prompt Parity

The `DESKTOP_AGENT_SYSTEM_PROMPT` constant in `runtime.rs` must remain identical to `CLOUD_AGENT_SYSTEM_PROMPT` in `cloud.rs`. Both prompts instruct the model to use tools rather than fabricate answers, forbid saying "I cannot access your files," and describe the available filesystem tools. Keeping them in sync ensures that switching between local and cloud inference does not change the agent's behaviour from the user's perspective.
