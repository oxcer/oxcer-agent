//! Phi-3 runtime abstraction. Implementations: stub (for wiring), later llama.cpp or ONNX.
//!
//! All inference is in-process; no HTTP, no localhost, no ports.

use crate::llm::{GenerationParams, LlmError};

/// Internal trait for Phi-3 inference. Engine calls this with token ids and params;
/// implementations can be stub, llama.cpp/GGUF, or ONNX Runtime.
pub trait PhiRuntime: Send + Sync {
    /// Generate token ids from input ids. Called by [super::LocalPhi3Engine] after tokenizing the prompt.
    fn generate(
        &self,
        input_ids: &[u32],
        params: &GenerationParams,
    ) -> Result<Vec<u32>, LlmError>;
}

/// Stub runtime: no real inference. Use for integration until llama.cpp or ONNX is wired.
pub struct StubPhiRuntime;

impl PhiRuntime for StubPhiRuntime {
    fn generate(
        &self,
        input_ids: &[u32],
        params: &GenerationParams,
    ) -> Result<Vec<u32>, LlmError> {
        let _ = (input_ids, params);
        // Return empty so detokenizer produces minimal output; or a few tokens for testing.
        Ok(vec![])
    }
}
