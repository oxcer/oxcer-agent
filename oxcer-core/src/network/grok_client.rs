//! xAI (Grok) API client.
//! Uses `HttpClient::for_tool(NetworkTool::Grok)` — no ad-hoc reqwest.

use serde::{Deserialize, Serialize};

use crate::network::{HttpClient, HttpError, NetworkTool};

/// Grok chat request (OpenAI-compatible format; extend as needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrokChatRequest {
    pub model: String,
    pub messages: Vec<serde_json::Value>,
}

/// Grok chat response (simplified; extend as needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrokChatResponse {
    pub choices: Option<Vec<serde_json::Value>>,
}

/// Call xAI Grok chat API. Requires `client` built with
/// `HttpClient::for_tool(NetworkTool::Grok)`.
pub async fn call_grok_chat(
    client: &HttpClient,
    request: &GrokChatRequest,
    api_key: &str,
) -> Result<GrokChatResponse, HttpError> {
    if client.tool() != NetworkTool::Grok {
        return Err(HttpError {
            message: "HttpClient must be bound to NetworkTool::Grok".to_string(),
        });
    }
    let url = "https://api.x.ai/v1/chat/completions";
    let resp = client.post_json_bearer(url, request, api_key).await?;
    let status = resp.status();
    let body = resp.text().await.map_err(HttpError::from)?;
    if !status.is_success() {
        return Err(HttpError {
            message: format!("xAI API error ({}): {}", status, body),
        });
    }
    serde_json::from_str(&body).map_err(|e| HttpError {
        message: format!("xAI response parse error: {}", e),
    })
}
