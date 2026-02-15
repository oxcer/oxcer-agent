//! LLM abstraction layer for Oxcer.
//!
//! All agent, tool, and router logic depends only on the [LlmEngine] trait and
//! [GenerationParams]. No HTTP or provider-specific types are exposed here.
//!
//! - **Local (in-process):** [LocalPhi3Engine] — no ports, no HTTP.
//! - **Remote:** [HttpLlmEngine] — all external HTTP LLM calls.
//! - **Hybrid:** [HybridEngine] — local first, fallback to remote.

mod bootstrap;
mod config;
mod downloader;
mod hybrid;
mod http_backend;
mod local_phi3;
mod model_downloader;

pub use bootstrap::create_engine_for_profile;
pub use config::{load_llm_profiles, load_models_config, LlmProfilesConfig, ModelsConfig};
pub use downloader::{download_file, DownloadProgressCallback};
pub use hybrid::HybridEngine;
pub use http_backend::{HttpLlmConfig, HttpLlmEngine};
pub use local_phi3::LocalPhi3Engine;
pub use model_downloader::{ensure_model_present, DownloadProgress};

// -----------------------------------------------------------------------------
// Core trait and types (no HTTP, no provider-specific types)
// -----------------------------------------------------------------------------

/// Parameters for a single generation request.
#[derive(Clone, Debug)]
pub struct GenerationParams {
    pub max_tokens: usize,
    pub temperature: f32,
    pub top_p: f32,
    /// Stop sequences; generation stops when any is produced.
    pub stop_sequences: Vec<String>,
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            max_tokens: 2048,
            temperature: 0.7,
            top_p: 0.95,
            stop_sequences: Vec::new(),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum LlmError {
    #[error("LLM engine not available: {0}")]
    NotAvailable(String),

    #[error("LLM generation failed: {0}")]
    GenerationFailed(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Engine that can generate text from a prompt. Implementations may be
/// in-process (local) or remote (HTTP); callers depend only on this trait.
pub trait LlmEngine: Send + Sync {
    fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError>;
}
