//! Gemini API client (placeholder).
//!
//! Stub for future Gemini-specific endpoint and request/response mapping.
//! For now, use OpenAI-compatible endpoints or add real Gemini API here.

use crate::llm::LlmError;

/// Placeholder: call Gemini API. Not implemented yet.
#[allow(dead_code)]
pub fn call_gemini_blocking(
    _endpoint_url: &str,
    _api_key: &str,
    _model: &str,
    _prompt: &str,
    _max_tokens: usize,
    _temperature: f32,
    _top_p: f32,
) -> Result<String, LlmError> {
    Err(LlmError::NotAvailable(
        "Gemini HTTP backend is a stub; use OpenAI-compatible endpoint or implement Gemini API"
            .to_string(),
    ))
}
