//! Gemini (Google) LLM API client.
//! Uses `HttpClient::for_tool(NetworkTool::Gemini)` — no ad-hoc reqwest.

use serde::{Deserialize, Serialize};

use crate::network::{HttpClient, HttpError, NetworkTool};

/// Gemini chat request (simplified; extend as needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiChatRequest {
    pub contents: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<serde_json::Value>,
}

/// Gemini chat response (simplified; extend as needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiChatResponse {
    pub candidates: Option<Vec<serde_json::Value>>,
}

/// Call Gemini chat API. Requires `client` built with
/// `HttpClient::for_tool(NetworkTool::Gemini)`.
pub async fn call_gemini_chat(
    client: &HttpClient,
    model: &str,
    api_key: &str,
    request: &GeminiChatRequest,
) -> Result<GeminiChatResponse, HttpError> {
    if client.tool() != NetworkTool::Gemini {
        return Err(HttpError {
            message: "HttpClient must be bound to NetworkTool::Gemini".to_string(),
        });
    }
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );
    let resp = client.post_json(&url, request).await?;
    let status = resp.status();
    let body = resp.text().await.map_err(HttpError::from)?;
    if !status.is_success() {
        return Err(HttpError {
            message: format!("Gemini API error ({}): {}", status, body),
        });
    }
    serde_json::from_str(&body).map_err(|e| HttpError {
        message: format!("Gemini response parse error: {}", e),
    })
}
