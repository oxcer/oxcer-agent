//! Cloud LLM provider abstraction.
//!
//! `ProviderKind` is the single source of truth for which inference providers exist.
//! `test_provider_connection` performs a minimal, one-token health-check for each
//! cloud provider using the security-gated `HttpClient` infrastructure.
//!
//! # Auth patterns
//! Each provider uses a different auth scheme:
//! - OpenAI:    `Authorization: Bearer {key}`        → `/v1/chat/completions`
//! - Anthropic: `x-api-key: {key}` + `anthropic-version: 2023-06-01` → `/v1/messages`
//! - Gemini:    API key in URL query param           → `/v1beta/models/{model}:generateContent`
//! - Grok:      `Authorization: Bearer {key}`        → `/v1/chat/completions` (OpenAI-compatible)

use serde::{Deserialize, Serialize};

use crate::network::{
    anthropic_client::{call_anthropic_messages, AnthropicMessagesRequest},
    gemini_client::{call_gemini_chat, GeminiChatRequest},
    grok_client::{call_grok_chat, GrokChatRequest},
    openai_client::{call_openai_chat, OpenAIChatRequest},
    HttpClient, NetworkTool,
};

// ─────────────────────────────────────────────────────────────────────────────
// ProviderKind
// ─────────────────────────────────────────────────────────────────────────────

/// All inference providers Oxcer can route to.
/// This is the single source of truth; the FFI layer mirrors it as `FfiProviderKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// On-device Meta Llama 3 via llama.cpp + Metal. No API key required.
    LocalLlama,
    /// OpenAI ChatGPT. Requires an OpenAI API key.
    OpenAI,
    /// Anthropic Claude. Requires an Anthropic API key.
    Anthropic,
    /// Google Gemini. Requires a Google AI Studio API key.
    Gemini,
    /// xAI Grok. Requires an xAI API key.
    Grok,
}

impl ProviderKind {
    /// Maps to the security-gated `NetworkTool`. Returns `None` for `LocalLlama`.
    pub fn network_tool(self) -> Option<NetworkTool> {
        match self {
            ProviderKind::LocalLlama => None,
            ProviderKind::OpenAI => Some(NetworkTool::OpenAI),
            ProviderKind::Anthropic => Some(NetworkTool::Anthropic),
            ProviderKind::Gemini => Some(NetworkTool::Gemini),
            ProviderKind::Grok => Some(NetworkTool::Grok),
        }
    }

    /// The default model used for health-checks and initial generation requests.
    /// These are cost-efficient models appropriate for a v0.1 default.
    pub fn default_model(self) -> &'static str {
        match self {
            ProviderKind::LocalLlama => "llama3-8b",
            ProviderKind::OpenAI => "gpt-4o-mini",
            ProviderKind::Anthropic => "claude-3-5-haiku-20241022",
            ProviderKind::Gemini => "gemini-2.0-flash",
            ProviderKind::Grok => "grok-2-1212",
        }
    }

    /// Human-readable name for display in the settings UI.
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderKind::LocalLlama => "Local (Meta Llama 3)",
            ProviderKind::OpenAI => "OpenAI (ChatGPT)",
            ProviderKind::Anthropic => "Anthropic (Claude)",
            ProviderKind::Gemini => "Google (Gemini)",
            ProviderKind::Grok => "xAI (Grok)",
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// ProviderTestResult
// ─────────────────────────────────────────────────────────────────────────────

/// Result of a provider connectivity test.
/// The FFI layer converts this to `FfiProviderTestResult` for Swift.
pub struct ProviderTestResult {
    /// `true` if the provider accepted the API key and returned a valid response.
    pub ok: bool,
    pub provider: ProviderKind,
    /// On success: a friendly confirmation (e.g. "Connected. Default model: gpt-4o-mini").
    /// On failure: a user-readable error (e.g. "Invalid API key. Please check and try again.").
    pub message: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// test_provider_connection
// ─────────────────────────────────────────────────────────────────────────────

/// Perform a minimal, one-token health-check for the given provider and API key.
///
/// The check is intentionally cheap — it sends the smallest valid request supported
/// by each API and verifies the response status code. No meaningful text is generated.
/// This function never panics; all errors are captured in `ProviderTestResult`.
pub async fn test_provider_connection(provider: ProviderKind, api_key: &str) -> ProviderTestResult {
    // LocalLlama has no remote endpoint to test.
    if provider == ProviderKind::LocalLlama {
        return ProviderTestResult {
            ok: false,
            provider,
            message: "The local model has no remote endpoint to test. \
                      It loads automatically on first use."
                .to_string(),
        };
    }

    let tool = match provider.network_tool() {
        Some(t) => t,
        None => unreachable!("all non-local providers have a NetworkTool"),
    };

    let client = match HttpClient::for_tool(tool) {
        Ok(c) => c,
        Err(e) => {
            return ProviderTestResult {
                ok: false,
                provider,
                message: format!("Failed to create network client: {}", e.message),
            }
        }
    };

    let result: Result<String, String> = match provider {
        ProviderKind::OpenAI => {
            let req = OpenAIChatRequest {
                model: provider.default_model().to_string(),
                // Single "Hi" message — cheapest possible valid request.
                messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            };
            call_openai_chat(&client, &req, api_key)
                .await
                .map(|_| format!("Connected. Default model: {}", provider.default_model()))
                .map_err(|e| friendly_error(&e.message))
        }

        ProviderKind::Anthropic => {
            let req = AnthropicMessagesRequest {
                model: provider.default_model().to_string(),
                // max_tokens=1 keeps cost and latency minimal.
                max_tokens: 1,
                messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            };
            // call_anthropic_messages uses the corrected x-api-key + anthropic-version headers.
            call_anthropic_messages(&client, &req, api_key)
                .await
                .map(|_| format!("Connected. Default model: {}", provider.default_model()))
                .map_err(|e| friendly_error(&e.message))
        }

        ProviderKind::Gemini => {
            let req = GeminiChatRequest {
                contents: vec![serde_json::json!({
                    "parts": [{"text": "Hi"}]
                })],
                generation_config: Some(serde_json::json!({"maxOutputTokens": 1})),
            };
            // Gemini passes the API key as a URL query parameter — see gemini_client.rs.
            call_gemini_chat(&client, provider.default_model(), api_key, &req)
                .await
                .map(|_| format!("Connected. Default model: {}", provider.default_model()))
                .map_err(|e| friendly_error(&e.message))
        }

        ProviderKind::Grok => {
            let req = GrokChatRequest {
                model: provider.default_model().to_string(),
                messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            };
            call_grok_chat(&client, &req, api_key)
                .await
                .map(|_| format!("Connected. Default model: {}", provider.default_model()))
                .map_err(|e| friendly_error(&e.message))
        }

        ProviderKind::LocalLlama => unreachable!("handled above"),
    };

    match result {
        Ok(message) => ProviderTestResult {
            ok: true,
            provider,
            message,
        },
        Err(message) => ProviderTestResult {
            ok: false,
            provider,
            message,
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Error mapping
// ─────────────────────────────────────────────────────────────────────────────

/// Maps raw API error strings to user-readable messages.
/// Avoids leaking internal HTTP details to non-technical users.
fn friendly_error(raw: &str) -> String {
    if raw.contains("401")
        || raw.contains("Unauthorized")
        || raw.contains("invalid_api_key")
        || raw.contains("authentication_error")
    {
        "Invalid API key. Please check the key and try again.".to_string()
    } else if raw.contains("403") || raw.contains("Forbidden") || raw.contains("permission") {
        "API key is valid but lacks the required permissions for this model.".to_string()
    } else if raw.contains("429") || raw.contains("rate_limit") || raw.contains("quota") {
        "Rate limit or quota exceeded. Please wait a moment and try again.".to_string()
    } else if raw.contains("timeout") || raw.contains("connect") || raw.contains("dns") {
        "Connection timed out. Check your network connection and try again.".to_string()
    } else if raw.contains("404") || raw.contains("model_not_found") {
        "Model not found. The default model may have been deprecated.".to_string()
    } else {
        // Include the raw error for debuggability, but keep it short.
        let truncated: String = raw.chars().take(120).collect();
        format!("Connection failed: {}", truncated)
    }
}
