//! Hybrid LLM engine: local first, fallback to external.
//!
//! [HybridEngine] composes a primary (typically local) and fallback (typically HTTP) engine.
//! Strategy: try primary; on failure or when configured to offload, delegate to fallback.
//! Routing logic (e.g. by task type or sensitivity) can be extended later.

use std::sync::Arc;

use crate::llm::{GenerationParams, LlmEngine, LlmError};

/// Combines a primary (e.g. local) and fallback (e.g. HTTP) engine.
/// Callers use [LlmEngine::generate]; selection is internal.
pub struct HybridEngine {
    primary: Arc<dyn LlmEngine>,
    fallback: Arc<dyn LlmEngine>,
}

impl HybridEngine {
    pub fn new(primary: Arc<dyn LlmEngine>, fallback: Arc<dyn LlmEngine>) -> Self {
        Self { primary, fallback }
    }

    /// Try primary; on failure, call fallback. No routing policy yet.
    fn generate_impl(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError> {
        match self.primary.generate(prompt, params) {
            Ok(out) => Ok(out),
            Err(e) => {
                log::warn!("Local LLM failed, falling back to external: {}", e);
                self.fallback.generate(prompt, params).map_err(|fallback_e| {
                    log::error!("Hybrid fallback also failed: {}", fallback_e);
                    LlmError::GenerationFailed(format!(
                        "primary failed: {}; fallback failed: {}",
                        e, fallback_e
                    ))
                })
            }
        }
    }
}

impl LlmEngine for HybridEngine {
    fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError> {
        self.generate_impl(prompt, params)
    }
}
