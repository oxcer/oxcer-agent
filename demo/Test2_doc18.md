# The Cloud LLM Backend

## Design Intent

Although Oxcer is local-first, the orchestrator and tool layer are deliberately backend-agnostic. The `LlmEngine` trait defines a single `generate(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError>` method. Any type that implements this trait can be dropped into the engine slot. `CloudLlmEngine` implements `LlmEngine` over blocking HTTP for four providers: OpenAI, Anthropic, Gemini, and Grok.

## Provider Routing

`CloudLlmEngine::generate` dispatches to a per-provider method based on the `ProviderKind` enum. OpenAI and Grok share the same request and response shape (OpenAI-compatible), so they reuse the same `extract_openai_text` response parser. Anthropic uses a `system` top-level field rather than a system message role, and Gemini uses `systemInstruction.parts[0].text`. Each provider has its own HTTP client call, but all four share the same system prompt constant.

## System Prompt Parity

`CLOUD_AGENT_SYSTEM_PROMPT` in `cloud.rs` is identical to `DESKTOP_AGENT_SYSTEM_PROMPT` in `runtime.rs`. This is enforced by a code comment and a test rather than a shared constant, because the two crates have a clean dependency boundary (the cloud backend does not depend on the local runtime). Any change to the system prompt must be applied to both locations simultaneously.

## No Streaming

The cloud backend uses blocking HTTP (`reqwest::blocking::Client`) with a 120-second timeout. It does not use streaming APIs. For the summarisation and overview tasks that Oxcer performs, the latency difference between streaming and non-streaming is small, and non-streaming simplifies the response parsing code significantly. Streaming support may be added in a future version.

## API Key Management

Cloud provider API keys are stored in the macOS Keychain via the `SettingsView`, never in plaintext on disk. The Swift layer reads the key from Keychain and passes it to the Rust FFI layer as a plain string argument when constructing the `CloudLlmEngine`. The key is not logged, traced, or stored in `SessionState`.
