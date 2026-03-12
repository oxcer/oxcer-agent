//! Cloud LLM engine — implements `LlmEngine` over blocking HTTP for all four
//! cloud providers (OpenAI, Anthropic, Gemini, Grok).
//!
//! This engine is the only part of the code that changes when the user switches
//! from local to cloud inference. The FSM, tool layer, approval gates, and all
//! `ToolCallIntent` structures are identical regardless of which engine is active.
//!
//! # System prompt
//! Cloud providers support first-class system messages via their API.
//! `CLOUD_AGENT_SYSTEM_PROMPT` must match `DESKTOP_AGENT_SYSTEM_PROMPT` in
//! `local_phi3/runtime.rs` exactly — both inject the same policy so on-device
//! and cloud paths behave identically from the orchestrator's point of view.

use serde_json;

use crate::cloud_provider::ProviderKind;
use crate::llm::{GenerationParams, LlmEngine, LlmError};

// ─────────────────────────────────────────────────────────────────────────────
// Shared system prompt
// MUST stay in sync with DESKTOP_AGENT_SYSTEM_PROMPT in local_phi3/runtime.rs.
// ─────────────────────────────────────────────────────────────────────────────

const CLOUD_AGENT_SYSTEM_PROMPT: &str = "\
You are Oxcer, a local desktop AI assistant running entirely on this machine.\n\
\n\
RULES — follow these without exception:\n\
1. If the user asks about the contents of a folder or file, you MUST use the \
corresponding filesystem tool (fs_list_dir for folders, fs_read_file for files) \
and base your answer solely on the tool result.\n\
2. Never invent, guess, or fabricate file names, folder structures, or file \
contents. If a tool fails or the path does not exist, say so explicitly.\n\
3. Never say \"I'm unable to directly view or access files on your computer\" — \
you have full access to the local filesystem through the tools below.\n\
4. When the user asks you to create, write, save, or summarise into a file:\n\
   a. Infer the target from prior tool outputs in this conversation — do NOT \
ask \"which file?\" if you already listed or read files in this session.\n\
   b. Choose a sensible output name (e.g. summary.md) if the user has not \
specified one.\n\
   c. Call fs_write_file to create the file, then report the path you wrote.\n\
   d. Only ask for clarification when there is genuine unresolvable ambiguity \
(e.g. two completely unrelated directories were listed and the intent truly \
cannot be determined from context).\n\
5. Never ask the user to repeat information that is already available from a \
prior tool result in the same conversation.\n\
6. Discover before acting on unknown paths: if the user says \"that folder\", \
\"those files\", or any implicit reference and no explicit path has been given, \
call fs_list_dir on the most recently referenced directory first, confirm what \
is there, then act. Never invent a path or assume a file exists without first \
verifying it with a tool call.\n\
7. After a successful fs_list_dir or fs_read_file, use the path that tool \
returned for all subsequent operations in this session — not the path that \
was originally guessed. The confirmed path from the tool result is ground truth.\n\
8. Never narrate tool calls. Do NOT write \"I will now call fs_read_file\", \
\"I have executed fs_list_dir\", \"I'm going to use fs_write_file\", or any \
similar narration. Tool calls are executed by the system BEFORE your response \
is generated. By the time you are responding, all planned tool calls have already \
run and their results are embedded directly in this prompt. Narrating tool calls \
is a hallucination — you are describing actions you cannot take at response time.\n\
9. FILE CONTENTS is ground truth. When your prompt contains a section marked \
\"FILE CONTENTS\", that section holds the real text read from disk by fs_read_file. \
Write your answer using ONLY that text. If the FILE CONTENTS section is empty or \
says \"(no content)\", output EXACTLY this one sentence and nothing else: \
\"No file contents were loaded — please check the target directory and retry.\"\n\
\n\
Available filesystem tools:\n\
- fs_list_dir   : lists files and subdirectories in a folder. \
Use this when the user asks \"what's in this folder?\", \"show me the files\", \
\"list the directory\", or any similar request.\n\
- fs_read_file  : reads the text content of a file. \
Use this when the user asks to summarise, explain, review, or quote a file.\n\
- fs_write_file : creates or overwrites a file with text content. \
Use this when the user asks to \"make a summary\", \"write this to a file\", \
\"create summary.md\", or any similar write request. Infer a sensible file \
name from context if the user has not specified one — do not ask for the name.\n\
\n\
Example of the desired autonomous behaviour:\n\
  User: \"list my Desktop\"\n\
  Agent calls fs_list_dir and sees [notes.md, report.md, draft.md].\n\
  User: \"make summary.md with that\"\n\
  Agent calls fs_read_file on each .md file, then calls fs_write_file to write \
summary.md in the same directory. It does NOT ask \"which file did you want \
summarised?\" because the context is already clear from the prior tool output.\n\
\n\
When you need filesystem information, use the appropriate tool rather than \
inventing or declining.";

// ─────────────────────────────────────────────────────────────────────────────
// CloudLlmEngine
// ─────────────────────────────────────────────────────────────────────────────

/// LLM engine that routes generation requests to a cloud provider over blocking
/// HTTP. Implements the same `LlmEngine` trait as `LocalPhi3Engine` so it can
/// be dropped into `CLOUD_ENGINE_SLOT` in the FFI layer without any changes to
/// the FSM, tool layer, or approval gate code.
pub struct CloudLlmEngine {
    provider: ProviderKind,
    api_key: String,
    model: String,
}

impl CloudLlmEngine {
    /// Create a new cloud engine for the given provider. The model is the
    /// provider's cost-efficient default (see `ProviderKind::default_model`).
    pub fn new(provider: ProviderKind, api_key: String) -> Self {
        let model = provider.default_model().to_string();
        Self { provider, api_key, model }
    }

    fn http_client() -> Result<reqwest::blocking::Client, LlmError> {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| LlmError::Internal(format!("HTTP client build failed: {}", e)))
    }
}

impl LlmEngine for CloudLlmEngine {
    fn generate(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError> {
        match self.provider {
            ProviderKind::OpenAI => self.generate_openai(prompt, params),
            ProviderKind::Anthropic => self.generate_anthropic(prompt, params),
            ProviderKind::Gemini => self.generate_gemini(prompt, params),
            ProviderKind::Grok => self.generate_grok(prompt, params),
            ProviderKind::LocalLlama => Err(LlmError::NotAvailable(
                "CloudLlmEngine cannot route to LocalLlama".to_string(),
            )),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-provider generation
// ─────────────────────────────────────────────────────────────────────────────

impl CloudLlmEngine {
    fn generate_openai(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError> {
        let client = Self::http_client()?;
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": CLOUD_AGENT_SYSTEM_PROMPT},
                {"role": "user",   "content": prompt}
            ],
            "max_tokens":  params.max_tokens,
            "temperature": params.temperature,
        });
        let resp = client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .map_err(|e| LlmError::GenerationFailed(format!("OpenAI request failed: {}", e)))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| LlmError::GenerationFailed(format!("OpenAI response read: {}", e)))?;
        if !status.is_success() {
            return Err(LlmError::GenerationFailed(format!(
                "OpenAI error ({}): {}",
                status, text
            )));
        }
        extract_openai_text(&text)
    }

    fn generate_anthropic(
        &self,
        prompt: &str,
        params: &GenerationParams,
    ) -> Result<String, LlmError> {
        let client = Self::http_client()?;
        // Anthropic uses a top-level `system` field (not a message role).
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": params.max_tokens,
            "system": CLOUD_AGENT_SYSTEM_PROMPT,
            "messages": [
                {"role": "user", "content": prompt}
            ]
        });
        let resp = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .map_err(|e| LlmError::GenerationFailed(format!("Anthropic request failed: {}", e)))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| LlmError::GenerationFailed(format!("Anthropic response read: {}", e)))?;
        if !status.is_success() {
            return Err(LlmError::GenerationFailed(format!(
                "Anthropic error ({}): {}",
                status, text
            )));
        }
        extract_anthropic_text(&text)
    }

    fn generate_gemini(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError> {
        let client = Self::http_client()?;
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );
        // Gemini uses `systemInstruction` for the system prompt.
        let body = serde_json::json!({
            "systemInstruction": {
                "parts": [{"text": CLOUD_AGENT_SYSTEM_PROMPT}]
            },
            "contents": [
                {"role": "user", "parts": [{"text": prompt}]}
            ],
            "generationConfig": {
                "maxOutputTokens": params.max_tokens,
                "temperature":     params.temperature,
            }
        });
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .map_err(|e| LlmError::GenerationFailed(format!("Gemini request failed: {}", e)))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| LlmError::GenerationFailed(format!("Gemini response read: {}", e)))?;
        if !status.is_success() {
            return Err(LlmError::GenerationFailed(format!(
                "Gemini error ({}): {}",
                status, text
            )));
        }
        extract_gemini_text(&text)
    }

    fn generate_grok(&self, prompt: &str, params: &GenerationParams) -> Result<String, LlmError> {
        let client = Self::http_client()?;
        // xAI Grok is OpenAI-compatible — same request/response shape.
        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": CLOUD_AGENT_SYSTEM_PROMPT},
                {"role": "user",   "content": prompt}
            ],
            "max_tokens":  params.max_tokens,
            "temperature": params.temperature,
        });
        let resp = client
            .post("https://api.x.ai/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .map_err(|e| LlmError::GenerationFailed(format!("Grok request failed: {}", e)))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| LlmError::GenerationFailed(format!("Grok response read: {}", e)))?;
        if !status.is_success() {
            return Err(LlmError::GenerationFailed(format!(
                "xAI error ({}): {}",
                status, text
            )));
        }
        // Grok is OpenAI-compatible — reuse the same extractor.
        extract_openai_text(&text)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Response text extraction helpers
// ─────────────────────────────────────────────────────────────────────────────

fn extract_openai_text(body: &str) -> Result<String, LlmError> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| LlmError::GenerationFailed(format!("OpenAI parse error: {}", e)))?;
    Ok(v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

fn extract_anthropic_text(body: &str) -> Result<String, LlmError> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| LlmError::GenerationFailed(format!("Anthropic parse error: {}", e)))?;
    Ok(v["content"][0]["text"].as_str().unwrap_or("").to_string())
}

fn extract_gemini_text(body: &str) -> Result<String, LlmError> {
    let v: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| LlmError::GenerationFailed(format!("Gemini parse error: {}", e)))?;
    Ok(v["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .to_string())
}
