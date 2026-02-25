//! Anthropic (Claude) API client.
//! Uses `HttpClient::for_tool(NetworkTool::Anthropic)` — no ad-hoc reqwest.
//!
//! Auth: Anthropic requires `x-api-key: {key}` and `anthropic-version: 2023-06-01`.
//! Do NOT use `Authorization: Bearer` — Anthropic rejects it with 401.

use serde::{Deserialize, Serialize};

use crate::network::{HttpClient, HttpError, NetworkTool};

/// Required `anthropic-version` header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic Messages API request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<serde_json::Value>,
}

/// Anthropic Messages API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicMessagesResponse {
    pub content: Option<Vec<serde_json::Value>>,
}

/// Call Anthropic Messages API. Requires `client` built with
/// `HttpClient::for_tool(NetworkTool::Anthropic)`.
///
/// Sends `x-api-key` + `anthropic-version` headers (not `Authorization: Bearer`).
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
    let resp = client
        .post_json_with_headers(
            url,
            request,
            &[
                ("x-api-key", api_key),
                ("anthropic-version", ANTHROPIC_VERSION),
            ],
        )
        .await?;
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
