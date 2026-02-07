//! Network tool registry and typed HTTP client.
//!
//! This module is the **single source of truth** for which remote tools exist.
//! All outbound HTTP MUST go through `HttpClient::for_tool(NetworkTool)` — no
//! ad-hoc `reqwest::Client` elsewhere in the codebase.

use serde::{Deserialize, Serialize};

use crate::network::HttpError;

/// Explicitly allowed network tools. Only these can make outbound requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkTool {
    Gemini,
    OpenAI,
    Anthropic,
    Grok,
    // Add any future HTTP tools explicitly here.
    // Example: WebSearch, GitHubIssues, Jira,
}

/// The fixed, explicit set of allowed outbound tools.
pub fn allowed_tools() -> &'static [NetworkTool] {
    &[
        NetworkTool::Gemini,
        NetworkTool::OpenAI,
        NetworkTool::Anthropic,
        NetworkTool::Grok,
    ]
}

fn is_tool_allowed(tool: NetworkTool) -> bool {
    allowed_tools().contains(&tool)
}

/// Typed HTTP client. Only construct via `HttpClient::for_tool(NetworkTool)`.
/// Ensures no network call happens without going through the tool allowlist.
pub struct HttpClient {
    inner: reqwest::Client,
    tool: NetworkTool,
}

impl HttpClient {
    /// Create an HTTP client for a specific tool. Only allowed tools succeed.
    pub fn for_tool(tool: NetworkTool) -> Result<Self, HttpError> {
        if !is_tool_allowed(tool) {
            return Err(HttpError {
                message: format!("Network tool {:?} is not in the allowlist", tool),
            });
        }
        let inner = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| HttpError {
                message: e.to_string(),
            })?;
        Ok(Self { inner, tool })
    }

    /// Which tool this client is bound to.
    pub fn tool(&self) -> NetworkTool {
        self.tool
    }

    /// POST JSON body and return response. Caller (LLM client) is responsible
    /// for constructing the correct URL for the bound tool.
    pub async fn post_json<T: serde::Serialize>(
        &self,
        url: &str,
        body: &T,
    ) -> Result<reqwest::Response, HttpError> {
        self.inner
            .post(url)
            .json(body)
            .send()
            .await
            .map_err(HttpError::from)
    }

    /// POST JSON with Bearer auth. For APIs that use `Authorization: Bearer <key>`.
    pub async fn post_json_bearer<T: serde::Serialize>(
        &self,
        url: &str,
        body: &T,
        bearer_token: &str,
    ) -> Result<reqwest::Response, HttpError> {
        self.inner
            .post(url)
            .bearer_auth(bearer_token)
            .json(body)
            .send()
            .await
            .map_err(HttpError::from)
    }

    /// GET request. For tools that need read-only API calls.
    pub async fn get(&self, url: &str) -> Result<reqwest::Response, HttpError> {
        self.inner.get(url).send().await.map_err(HttpError::from)
    }
}

// -----------------------------------------------------------------------------
// Optional: AgentTool → NetworkTool binding for Security Policy Engine
// -----------------------------------------------------------------------------

/// High-level agent-facing tool (for Policy Engine mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentTool {
    LlmChat,
    LlmCode,
    WebSearch,
}

/// Binds an agent tool to the network primitive it uses.
pub struct ToolNetworkBinding {
    pub agent_tool: AgentTool,
    pub network_tool: NetworkTool,
}

/// Mapping from agent tools to network tools. Used by the Policy Engine to
/// answer: "Which external endpoints can this agent/tool ever talk to?"
pub fn tool_bindings() -> Vec<ToolNetworkBinding> {
    vec![
        ToolNetworkBinding {
            agent_tool: AgentTool::LlmChat,
            network_tool: NetworkTool::Gemini,
        },
        ToolNetworkBinding {
            agent_tool: AgentTool::LlmCode,
            network_tool: NetworkTool::OpenAI,
        },
    ]
}
