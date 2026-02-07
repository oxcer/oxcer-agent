//! OpenAI (ChatGPT) API client.
//! Uses `HttpClient::for_tool(NetworkTool::OpenAI)` — no ad-hoc reqwest.

use serde::{Deserialize, Serialize};

use crate::network::{HttpClient, HttpError, NetworkTool};

/// OpenAI chat request (simplified; extend as needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatRequest {
    pub model: String,
    pub messages: Vec<serde_json::Value>,
}

/// OpenAI chat response (simplified; extend as needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAIChatResponse {
    pub choices: Option<Vec<serde_json::Value>>,
}

/// Call OpenAI chat API. Requires `client` built with
/// `HttpClient::for_tool(NetworkTool::OpenAI)`.
pub async fn call_openai_chat(
    client: &HttpClient,
    request: &OpenAIChatRequest,
    api_key: &str,
) -> Result<OpenAIChatResponse, HttpError> {
    if client.tool() != NetworkTool::OpenAI {
        return Err(HttpError {
            message: "HttpClient must be bound to NetworkTool::OpenAI".to_string(),
        });
    }
    let url = "https://api.openai.com/v1/chat/completions";
    let resp = client.post_json_bearer(url, request, api_key).await?;
    let status = resp.status();
    let body = resp.text().await.map_err(HttpError::from)?;
    if !status.is_success() {
        return Err(HttpError {
            message: format!("OpenAI API error ({}): {}", status, body),
        });
    }
    serde_json::from_str(&body).map_err(|e| HttpError {
        message: format!("OpenAI response parse error: {}", e),
    })
}
