//! OpenAI-compatible chat completions client.
//!
//! Calls `/v1/chat/completions` (or configurable endpoint) and maps response to plain text.
//! Used by [super::HttpLlmEngine]; all external HTTP LLM traffic goes through this module.

use serde::{Deserialize, Serialize};

use crate::llm::LlmError;

/// Request body for OpenAI-compatible chat completions.
#[derive(Debug, Serialize)]
pub struct OpenAiChatRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    pub max_tokens: u32,
    pub temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct OpenAiMessage {
    pub role: String,
    pub content: String,
}

/// Response from OpenAI-compatible API.
#[derive(Debug, Deserialize)]
pub struct OpenAiChatResponse {
    pub choices: Option<Vec<OpenAiChoice>>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiChoice {
    pub message: Option<OpenAiMessageOut>,
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiMessageOut {
    pub content: Option<String>,
}

/// Call OpenAI-compatible `/v1/chat/completions` with blocking HTTP.
/// Returns the first choice content as a string.
pub fn call_openai_completions_blocking(
    endpoint_url: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
    max_tokens: usize,
    temperature: f32,
    top_p: f32,
    stop_sequences: &[String],
) -> Result<String, LlmError> {
    let url = format!("{}/v1/chat/completions", endpoint_url.trim_end_matches('/'));
    let request = OpenAiChatRequest {
        model: model.to_string(),
        messages: vec![OpenAiMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
        }],
        max_tokens: max_tokens as u32,
        temperature,
        top_p: Some(top_p),
        stop: stop_sequences.to_vec(),
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| LlmError::Internal(format!("HTTP client build failed: {}", e)))?;

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .json(&request)
        .send()
        .map_err(|e| LlmError::GenerationFailed(format!("OpenAI request failed: {}", e)))?;

    let status = resp.status();
    let body = resp
        .text()
        .map_err(|e| LlmError::GenerationFailed(format!("OpenAI response read failed: {}", e)))?;

    if !status.is_success() {
        return Err(LlmError::GenerationFailed(format!(
            "OpenAI API error ({}): {}",
            status, body
        )));
    }

    let parsed: OpenAiChatResponse = serde_json::from_str(&body).map_err(|e| {
        LlmError::GenerationFailed(format!("OpenAI response parse error: {}", e))
    })?;

    let text = parsed
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message.and_then(|m| m.content).or(c.text))
        .unwrap_or_default();

    Ok(text)
}
