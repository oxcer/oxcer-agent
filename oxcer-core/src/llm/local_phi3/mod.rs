//! In-process Phi-3-small engine. No HTTP, no server, no port binding.
//!
//! All inference is done via in-process function calls (runtime: stub, later llama.cpp/GGUF or
//! ONNX Runtime). Weights and tokenizer are loaded from the filesystem.
//!
//! **Singleton use:** This engine loads ~2–3GB into memory. Launchers (FFI, Tauri) must create
//! it once (e.g. via [crate::llm::bootstrap::create_engine_for_profile]) and reuse the same
//! `Arc<dyn LlmEngine>` for all generation calls. Do not call `LocalPhi3Engine::new()` in a loop
//! or on every request.

mod loader;
mod runtime;

use std::path::Path;

use tokenizers::Tokenizer;

use crate::llm::{GenerationParams, LlmEngine, LlmError};

pub use runtime::PhiRuntime;

/// In-process Phi-3-small engine. Holds tokenizer and runtime; no network.
pub struct LocalPhi3Engine {
    tokenizer: Tokenizer,
    runtime: Box<dyn PhiRuntime>,
}

impl LocalPhi3Engine {
    /// Load tokenizer and initialize runtime from `model_root`. Fails if files are missing.
    pub fn new(model_root: &Path) -> Result<Self, LlmError> {
        let paths = loader::resolve_model_paths(model_root)?;
        log::info!(
            "Loading Phi-3 tokenizer from {:?}",
            paths.tokenizer_json
        );

        let tokenizer = loader::load_tokenizer(&paths.tokenizer_json).map_err(|e| {
            log::error!("Tokenizer load failed: {}", e);
            e
        })?;

        // TODO: Initialize llama.cpp or ONNX Runtime with paths.model_gguf.
        // For now use stub so the pipeline is wired and ready for real inference.
        let runtime: Box<dyn PhiRuntime> = Box::new(runtime::StubPhiRuntime);

        log::info!("Local Phi-3 engine initialized (stub runtime); model at {:?}", paths.model_gguf);

        Ok(Self { tokenizer, runtime })
    }
}

impl LlmEngine for LocalPhi3Engine {
    fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError> {
        println!(
            "[Rust] creating request context at {:?}",
            std::time::SystemTime::now()
        );

        // Tokenize prompt
        let encoding = self.tokenizer.encode(prompt, true).map_err(|e| {
            LlmError::GenerationFailed(format!("Tokenization failed: {}", e))
        })?;
        let input_ids: Vec<u32> = encoding.get_ids().to_vec();

        println!("[Rust] request context created");

        if input_ids.is_empty() {
            return Ok(String::new());
        }

        // Call runtime in-process (no HTTP, no ports)
        let output_ids = self.runtime.generate(&input_ids, params).map_err(|e| {
            log::error!("Phi-3 runtime generation failed: {}", e);
            e
        })?;

        // Detokenize
        if output_ids.is_empty() {
            return Ok(String::new());
        }
        let decoded = self.tokenizer.decode(&output_ids, true).map_err(|e| {
            LlmError::GenerationFailed(format!("Detokenization failed: {}", e))
        })?;
        Ok(decoded)
    }
}
