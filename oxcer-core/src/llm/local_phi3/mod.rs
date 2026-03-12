//! In-process Phi-3-mini engine. No HTTP, no server, no port binding.
//!
//! Inference is done in-process via [`LlamaCppPhiRuntime`], which loads the GGUF
//! weights and runs llama.cpp with Metal GPU offload on Apple Silicon.
//!
//! **Singleton use:** This engine holds ~2–3 GB of weights in memory. The FFI layer
//! creates it once via `GLOBAL_ENGINE` and reuses the same `Arc<dyn LlmEngine>` for
//! all calls. Do not call [`LocalPhi3Engine::new`] in a loop or on every request.

mod loader;
mod runtime;

use std::path::Path;

use tokenizers::Tokenizer;

use crate::llm::{GenerationParams, LlmEngine, LlmError};

pub use runtime::PhiRuntime;

/// In-process local GGUF engine. Holds optional tokenizer and runtime; no network.
pub struct LocalPhi3Engine {
    /// HuggingFace tokenizer — only populated when `tokenizer.json` exists alongside
    /// the GGUF. Llama-3 (and other models that embed vocabulary in GGUF) set this to
    /// `None`; the `generate_direct` fast path uses llama.cpp's internal tokeniser.
    tokenizer: Option<Tokenizer>,
    runtime: Box<dyn PhiRuntime>,
}

impl LocalPhi3Engine {
    /// Load model and optionally tokenizer from `model_root`. Fails only if the GGUF is missing.
    pub fn new(model_root: &Path) -> Result<Self, LlmError> {
        let paths = loader::resolve_model_paths(model_root)?;

        let tokenizer = if let Some(ref tok_path) = paths.tokenizer_json {
            log::info!("Loading tokenizer from {:?}", tok_path);
            match loader::load_tokenizer(tok_path) {
                Ok(t) => {
                    log::info!("Tokenizer loaded from {:?}", tok_path);
                    Some(t)
                }
                Err(e) => {
                    log::warn!(
                        "Tokenizer load failed ({}); continuing without HF tokenizer",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        let runtime: Box<dyn PhiRuntime> =
            Box::new(runtime::LlamaCppPhiRuntime::load(&paths.model_gguf)?);

        log::info!(
            "Local engine initialized (llama.cpp/Metal); model at {:?}",
            paths.model_gguf
        );

        Ok(Self { tokenizer, runtime })
    }
}

impl LlmEngine for LocalPhi3Engine {
    fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError> {
        tracing::debug!(
            event = "local_phi3_generate_enter",
            prompt_len = prompt.len(),
            "LocalPhi3Engine generate enter"
        );

        // Fast path: LlamaCppPhiRuntime returns a full string directly (own tokenisation),
        // bypassing the tokenize → generate → detokenize round-trip.
        if let Some(text) = self.runtime.generate_direct(prompt, params)? {
            tracing::debug!(
                event = "local_phi3_generate_done",
                text_len = text.len(),
                "generate_direct done"
            );
            return Ok(text);
        }

        // Tokenize prompt (fallback path — not used by LlamaCppPhiRuntime).
        let tokenizer = self.tokenizer.as_ref().ok_or_else(|| {
            LlmError::GenerationFailed(
                "HF tokenizer not loaded and generate_direct returned None; \
                 no tokenization path available."
                    .to_string(),
            )
        })?;

        let encoding = tokenizer
            .encode(prompt, true)
            .map_err(|e| LlmError::GenerationFailed(format!("Tokenization failed: {}", e)))?;
        let input_ids: Vec<u32> = encoding.get_ids().to_vec();

        if input_ids.is_empty() {
            tracing::warn!(
                event = "local_phi3_tokenize_empty",
                "tokenization produced 0 tokens"
            );
            return Ok(String::new());
        }

        // Call runtime in-process (no HTTP, no ports).
        let output_ids = self.runtime.generate(&input_ids, params).map_err(|e| {
            tracing::error!(event = "local_phi3_runtime_error", err = %e, "runtime generation failed");
            e
        })?;

        // Detokenize.
        if output_ids.is_empty() {
            tracing::warn!(
                event = "local_phi3_output_empty",
                "runtime returned 0 output tokens"
            );
            return Ok(String::new());
        }
        let decoded = tokenizer
            .decode(&output_ids, true)
            .map_err(|e| LlmError::GenerationFailed(format!("Detokenization failed: {}", e)))?;
        Ok(decoded)
    }
}
