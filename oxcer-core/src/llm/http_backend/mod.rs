//! HTTP-backed LLM engines. All external LLM calls go through this module.
//!
//! - [HttpLlmEngine]: OpenAI-compatible endpoint (configurable URL, single-provider).
//! - [CloudLlmEngine]: Routes to any of the four cloud providers; used by the FFI
//!   layer for the `CLOUD_ENGINE_SLOT` DI path.

mod cloud;
mod gemini;
mod openai;

pub use cloud::CloudLlmEngine;
use crate::llm::{GenerationParams, LlmEngine, LlmError};

/// Configuration for the HTTP LLM backend (endpoint, model, API key).
#[derive(Clone, Debug)]
pub struct HttpLlmConfig {
    pub endpoint_url: String,
    pub model: String,
    pub api_key: String,
}

impl HttpLlmConfig {
    pub fn new(endpoint_url: String, model: String, api_key: String) -> Self {
        Self {
            endpoint_url,
            model,
            api_key,
        }
    }
}

/// LLM engine that performs outbound HTTP calls to an OpenAI-compatible (or similar) API.
pub struct HttpLlmEngine {
    config: HttpLlmConfig,
}

impl HttpLlmEngine {
    pub fn new(config: HttpLlmConfig) -> Self {
        Self { config }
    }
}

impl LlmEngine for HttpLlmEngine {
    fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError> {
        openai::call_openai_completions_blocking(
            &self.config.endpoint_url,
            &self.config.api_key,
            &self.config.model,
            prompt,
            params.max_tokens,
            params.temperature,
            params.top_p,
            &params.stop_sequences,
        )
    }
}
