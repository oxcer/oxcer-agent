//! Network tool layer: explicit allowlist of outbound HTTP tools.
//!
//! Only the four LLM providers (Gemini, OpenAI, Anthropic, Grok) and explicitly
//! added HTTP tools can make outbound requests. All network access goes through
//! `HttpClient::for_tool(NetworkTool)` — no ad-hoc `reqwest::Client` elsewhere.

mod error;
mod tools;

pub use error::HttpError;
pub use tools::{
    allowed_tools, tool_bindings, AgentTool, HttpClient, NetworkTool, ToolNetworkBinding,
};

// LLM client wrappers (thin modules that use HttpClient)
pub mod anthropic_client;
pub mod gemini_client;
pub mod grok_client;
pub mod openai_client;
