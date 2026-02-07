//! Anthropic (Claude) API client.
//! Uses `HttpClient::for_tool(NetworkTool::Anthropic)` — no ad-hoc reqwest.

use serde::{Deserialize, Serialize};

use crate::network::{HttpClient, HttpError, NetworkTool};

/// Anthropic messages request (simplified; extend as needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<serde_json::Value>,
}

/// Anthropic messages response (simplified; extend as needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessagesResponse {
    pub content: Option<Vec<serde_json::Value>>,
}

/// Call Anthropic messages API. Requires `client` built with
/// `HttpClient::for_tool(NetworkTool::Anthropic)`.
pub async fn call_anthropic_messages(
    client: &HttpClient,
    request: &AnthropicMessagesRequest,
    api_key: &str,
) -> Result<AnthropicMessagesResponse, HttpError> {
    if client.tool() != NetworkTool::Anthropic {
        return Err(HttpError {
            message: "HttpClient must be bound to NetworkTool::Anthropic".to_string(),
        });
    }
    let url = "https://api.anthropic.com/v1/messages";
    let resp = client.post_json_bearer(url, request, api_key).await?;
    let status = resp.status();
    let body = resp.text().await.map_err(HttpError::from)?;
    if !status.is_success() {
        return Err(HttpError {
            message: format!("Anthropic API error ({}): {}", status, body),
        });
    }
    serde_json::from_str(&body).map_err(|e| HttpError {
        message: format!("Anthropic response parse error: {}", e),
    })
}
