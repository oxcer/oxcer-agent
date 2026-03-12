//! Pre-prompt sanitizer and scrubbing pipeline for every LLM call.
//!
//! Ensures sensitive paths and secrets are never sent to untrusted LLMs.
//! All model inputs that include file content or user-supplied text must go
//! through this module. No code path should bypass it to call the model directly.
//!
//! ## Pipeline (integrate into every LLM call)
//!
//! 1. Build a combined **raw payload** from task, file snippets, shell outputs, tool outputs, metadata
//!    (e.g. `build_raw_payload(&parts)`).
//! 2. Before sending to the provider: run `scrub_for_llm_call(&raw_payload, &options)`.
//! 3. If `Err(ScrubbingError::TooMuchSensitiveData)`: do **not** call the LLM; return an error to the
//!    Orchestrator so it can fall back to tools-only or prompt the user.
//! 4. Otherwise use `SensitivityResult.masked_content` (via the Ok string) to build the scrubbed
//!    prompt/messages that go over the wire. Never send `raw_payload` directly.
//!
//! Text scrubbing is delegated to the **data_sensitivity** classifier (DLP at the prompt boundary);
//! this module keeps path checks, the `sanitize_for_llm` API, and the central pipeline hook.

use serde::{Deserialize, Serialize};

use crate::data_sensitivity;

const SENSITIVE_FILE_PLACEHOLDER: &str = "[REDACTED_SENSITIVE_FILE]";

/// Fraction of original length below which we refuse to call the LLM (≥ this much redacted).
const SCRUB_THRESHOLD_RATIO: f64 = 0.5;

// -----------------------------------------------------------------------------
// Sensitive path patterns (paths that must not be sent as content)
// -----------------------------------------------------------------------------

/// Path segments or file names that indicate sensitive data. Case-insensitive where appropriate.
fn sensitive_path_patterns() -> &'static [&'static str] {
    &[
        ".ssh",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
        ".aws/credentials",
        ".aws/credentials.",
        ".gitconfig",
        ".gnupg",
        ".env",
        ".env.local",
        ".env.production",
        ".env.development",
        ".netrc",
        ".git-credentials",
        ".docker/config.json",
        ".kube/config",
        "keychains",
        "passwords",
        ".terraform",
        "gcloud",
        ".azure",
        ".pem",
        ".key",
        ".pfx",
        ".p12",
        ".keystore",
        ".jks",
        "password",
        "secret",
        "credentials",
        "token",
        ".token",
    ]
}

/// File name patterns (suffix or contains). Match against the last component of the path.
fn sensitive_filename_patterns() -> &'static [&'static str] {
    &[
        ".pem",
        ".key",
        ".pfx",
        ".p12",
        ".keystore",
        ".jks",
        ".token",
        ".credentials",
        ".secret",
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
    ]
}

/// Returns true if the path (or its filename) matches sensitive patterns.
/// Accepts paths with or without leading ~; ~ is not expanded (caller may normalize).
pub fn is_sensitive_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_lowercase();
    let path_lower = normalized.as_str();

    for pat in sensitive_path_patterns() {
        if path_lower.contains(&pat.to_lowercase()) {
            return true;
        }
    }

    let filename = path_lower.split('/').last().unwrap_or(path_lower);
    for pat in sensitive_filename_patterns() {
        if filename.ends_with(&pat.to_lowercase()) || filename.contains(pat) {
            return true;
        }
    }

    if filename.starts_with("password") || filename.starts_with("secret") {
        return true;
    }

    false
}

// -----------------------------------------------------------------------------
// Secret patterns in text (delegate to data_sensitivity classifier)
// -----------------------------------------------------------------------------

/// Redacts secrets and sensitive content in text using the data_sensitivity classifier.
/// Runs on every piece of text that may be sent to an LLM. Returns the masked string.
pub fn sanitize_text(text: &str) -> String {
    data_sensitivity::classify_and_mask_default(text).masked_content
}

/// Same as `sanitize_text` but with explicit classifier options (e.g. workspace_root for path normalization).
pub fn sanitize_text_with_options(
    text: &str,
    options: &data_sensitivity::ClassifierOptions,
) -> String {
    data_sensitivity::classify_and_mask(text, options).masked_content
}

// -----------------------------------------------------------------------------
// Central scrubbing pipeline (hook for every LLM call)
// -----------------------------------------------------------------------------

/// Decision for the scrubbing audit log: whether the payload was sent, blocked by threshold, or blocked by hard rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrubbingDecision {
    /// Payload was scrubbed and sent to the LLM.
    ScrubbedAndSent,
    /// Payload was scrubbed but blocked because ≥50% was redacted.
    ScrubbedAndBlocked,
    /// Payload was blocked by a hard never-send rule (e.g. private key, credentials).
    BlockedByHardRule,
}

/// One audit log entry for a scrubbing operation. Written to scrubbing.log (or events.log) so scrubbing is auditable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScrubbingLogEntry {
    pub timestamp: String,
    pub session_id: String,
    pub original_length: usize,
    pub redacted_length: usize,
    pub max_sensitivity_level: String,
    pub matched_kinds: Vec<String>,
    pub decision: ScrubbingDecision,
}

/// Error returned when the scrubbing pipeline refuses to send (e.g. too much sensitive data or never-send rule).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScrubbingError {
    /// ≥50% of the payload was redacted; LLM call must be skipped.
    TooMuchSensitiveData {
        /// Human-readable message for the Orchestrator / UI.
        message: String,
    },
    /// Hard "never send to LLM" rule triggered (e.g. private key, credential file content). Call aborted.
    NeverSendToLlm {
        /// Human-readable message; high-severity event should be logged by caller.
        message: String,
        /// Finding kind that triggered the rule (e.g. "ssh_private_key").
        finding_kind: String,
    },
}

impl std::fmt::Display for ScrubbingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScrubbingError::TooMuchSensitiveData { message } => write!(f, "{}", message),
            ScrubbingError::NeverSendToLlm { message, .. } => write!(f, "{}", message),
        }
    }
}

impl std::error::Error for ScrubbingError {}

/// Standard message returned when context is too sensitive to send to the LLM.
pub const TOO_MUCH_SENSITIVE_DATA_MESSAGE: &str =
    "Context contains too much sensitive data; LLM call skipped. Try tools-only or manually inspect.";

/// Finding kinds that must never be sent to the LLM (private keys, credentials). When any finding
/// matches, the scrubber aborts the call and logs a high-severity event.
const NEVER_SEND_FINDING_KINDS: &[&str] = &[
    "ssh_private_key",
    "pem_private_key_header",
    "ssh_private_key_path",
    "aws_access_key_id",
    "aws_credentials",
    "jwt_or_oauth_token",
    "password_in_env",
    "password_in_url",
    "secret_or_password_env",
    "api_key",
];

fn is_never_send_finding(kind: &str) -> bool {
    NEVER_SEND_FINDING_KINDS.contains(&kind)
}

/// Log a high-severity event when a never-send rule is triggered. Call once when scrubber aborts.
pub fn log_never_send_llm_triggered(finding_kind: &str, context: &str) {
    if let Ok(json) = serde_json::to_string(&serde_json::json!({
        "event": "OXCER_SECURITY_NEVER_SEND_LLM",
        "severity": "high",
        "finding_kind": finding_kind,
        "message": "Content matched never-send rule; LLM call aborted.",
        "context_preview": if context.len() > 200 { format!("{}...", &context[..200]) } else { context.to_string() },
    })) {
        eprintln!("[OXCER_SECURITY] {}", json);
    }
}

/// Runs the sensitivity classifier on the raw payload, enforces the threshold, and hard never-send rules.
/// Call this **before** every LLM request; use the returned string for the wire, never `raw_payload`.
///
/// If any finding matches a never-send pattern (private keys, credentials), returns `Err(ScrubbingError::NeverSendToLlm)`,
/// logs a high-severity event, and the caller must not call the LLM.
/// If `original_length > 0` and `redacted_length <= original_length * 0.5` (i.e. ≥50% removed),
/// returns `Err(ScrubbingError::TooMuchSensitiveData)` and the caller must **not** call the LLM—
/// return this error to the Orchestrator so it can fall back to tools-only or prompt the user.
pub fn scrub_for_llm_call(
    raw_payload: &str,
    options: &data_sensitivity::ClassifierOptions,
) -> Result<String, ScrubbingError> {
    let result = data_sensitivity::classify_and_mask(raw_payload, options);

    for f in &result.findings {
        if is_never_send_finding(&f.kind) {
            log_never_send_llm_triggered(&f.kind, raw_payload);
            return Err(ScrubbingError::NeverSendToLlm {
                message: "Content contains credentials or private keys; LLM call aborted. Remove sensitive data or use tools-only.".to_string(),
                finding_kind: f.kind.clone(),
            });
        }
    }

    let original = result.original_length;
    let redacted = result.redacted_length;

    if original > 0 {
        let ratio = redacted as f64 / original as f64;
        if ratio <= SCRUB_THRESHOLD_RATIO {
            return Err(ScrubbingError::TooMuchSensitiveData {
                message: TOO_MUCH_SENSITIVE_DATA_MESSAGE.to_string(),
            });
        }
    }

    Ok(result.masked_content)
}

fn sensitivity_level_to_string(level: data_sensitivity::SensitivityLevel) -> String {
    match level {
        data_sensitivity::SensitivityLevel::Low => "low".to_string(),
        data_sensitivity::SensitivityLevel::Medium => "medium".to_string(),
        data_sensitivity::SensitivityLevel::High => "high".to_string(),
    }
}

/// Like `scrub_for_llm_call` but also returns a `ScrubbingLogEntry` for audit logging.
/// The caller should append the entry to scrubbing.log (or events.log) so scrubbing is auditable.
pub fn scrub_for_llm_call_audit(
    raw_payload: &str,
    options: &data_sensitivity::ClassifierOptions,
    session_id: &str,
) -> (Result<String, ScrubbingError>, ScrubbingLogEntry) {
    let result = data_sensitivity::classify_and_mask(raw_payload, options);
    let timestamp = chrono::Utc::now().to_rfc3339();
    let matched_kinds: Vec<String> = result.findings.iter().map(|f| f.kind.clone()).collect();
    let max_sensitivity_level = sensitivity_level_to_string(result.level);
    let original = result.original_length;
    let redacted = result.redacted_length;

    for f in &result.findings {
        if is_never_send_finding(&f.kind) {
            log_never_send_llm_triggered(&f.kind, raw_payload);
            let entry = ScrubbingLogEntry {
                timestamp: timestamp.clone(),
                session_id: session_id.to_string(),
                original_length: original,
                redacted_length: redacted,
                max_sensitivity_level: max_sensitivity_level.clone(),
                matched_kinds: matched_kinds.clone(),
                decision: ScrubbingDecision::BlockedByHardRule,
            };
            return (
                Err(ScrubbingError::NeverSendToLlm {
                    message: "Content contains credentials or private keys; LLM call aborted. Remove sensitive data or use tools-only.".to_string(),
                    finding_kind: f.kind.clone(),
                }),
                entry,
            );
        }
    }

    if original > 0 {
        let ratio = redacted as f64 / original as f64;
        if ratio <= SCRUB_THRESHOLD_RATIO {
            let entry = ScrubbingLogEntry {
                timestamp: timestamp.clone(),
                session_id: session_id.to_string(),
                original_length: original,
                redacted_length: redacted,
                max_sensitivity_level: max_sensitivity_level.clone(),
                matched_kinds: matched_kinds.clone(),
                decision: ScrubbingDecision::ScrubbedAndBlocked,
            };
            return (
                Err(ScrubbingError::TooMuchSensitiveData {
                    message: TOO_MUCH_SENSITIVE_DATA_MESSAGE.to_string(),
                }),
                entry,
            );
        }
    }

    let entry = ScrubbingLogEntry {
        timestamp,
        session_id: session_id.to_string(),
        original_length: original,
        redacted_length: redacted,
        max_sensitivity_level,
        matched_kinds,
        decision: ScrubbingDecision::ScrubbedAndSent,
    };
    (Ok(result.masked_content), entry)
}

/// Converts an absolute path to workspace-relative for LLM context. Use when building
/// file snippets so the prompt never contains full user paths (e.g. `/Users/j/proj/src/foo.rs` -> `./src/foo.rs`).
pub fn to_workspace_relative_path(absolute_path: &str, workspace_root: &str) -> String {
    let abs = absolute_path.trim_end_matches('/');
    let root = workspace_root.trim_end_matches('/');
    if root.is_empty() {
        return absolute_path.to_string();
    }
    if let Some(rest) = abs.strip_prefix(root) {
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            return ".".to_string();
        }
        return format!("./{}", rest);
    }
    absolute_path.to_string()
}

/// Input parts for building the combined payload sent to the LLM.
/// The runner (frontend or backend) assembles these and then runs `scrub_for_llm_call` on the result.
/// When adding file snippets, use **workspace-relative paths** (e.g. via `to_workspace_relative_path`)
/// so full user paths are never sent to the LLM.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LlmPayloadParts {
    /// Task or user message.
    #[serde(default)]
    pub task: String,
    /// File snippets (path + content) to include. Use workspace-relative paths; sensitive paths should be excluded or placeholder-only.
    #[serde(default)]
    pub file_snippets: Vec<FileContentChunk>,
    /// Shell command outputs (e.g. stdout/stderr) if any.
    #[serde(default)]
    pub shell_outputs: Vec<String>,
    /// Previous tool outputs (e.g. FS read, list dir) if any.
    #[serde(default)]
    pub tool_outputs: Vec<String>,
    /// Optional metadata (model hint, workspace path, etc.). Will be scrubbed like the rest.
    #[serde(default)]
    pub metadata: Vec<String>,
}

/// Builds a single raw payload string from all parts. The caller must run `scrub_for_llm_call`
/// on this string before sending to any LLM provider. Never send the return value directly over the wire.
pub fn build_raw_payload(parts: &LlmPayloadParts) -> String {
    let mut sections = Vec::new();

    if !parts.task.is_empty() {
        sections.push(format!("Task:\n{}", parts.task));
    }
    for chunk in &parts.file_snippets {
        if is_sensitive_path(&chunk.path) {
            sections.push(format!(
                "--- {} ---\n{}",
                chunk.path, SENSITIVE_FILE_PLACEHOLDER
            ));
        } else {
            sections.push(format!("--- {} ---\n{}", chunk.path, chunk.content));
        }
    }
    for (i, out) in parts.shell_outputs.iter().enumerate() {
        sections.push(format!("Shell output [{}]:\n{}", i + 1, out));
    }
    for (i, out) in parts.tool_outputs.iter().enumerate() {
        sections.push(format!("Tool output [{}]:\n{}", i + 1, out));
    }
    for m in &parts.metadata {
        if !m.is_empty() {
            sections.push(m.clone());
        }
    }

    sections.join("\n\n")
}

/// Build the raw payload from parts, then scrub it. Use this as the single pipeline entry point:
/// if `Ok(scrubbed)`, use `scrubbed` for the LLM request; if `Err(TooMuchSensitiveData)`, do not
/// call the LLM and return the error to the Orchestrator.
pub fn build_and_scrub_for_llm(
    parts: &LlmPayloadParts,
    options: &data_sensitivity::ClassifierOptions,
) -> Result<String, ScrubbingError> {
    let raw = build_raw_payload(parts);
    scrub_for_llm_call(&raw, options)
}

// -----------------------------------------------------------------------------
// Public API: prepare model input (task + optional file contents)
// -----------------------------------------------------------------------------

/// Input to the sanitizer when building model prompt (task + optional path->content).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SanitizeForLlmInput {
    /// User task or message (will be sanitized for secrets).
    pub task: String,
    /// Optional file contents to include. For each (path, content): if path is sensitive, only a placeholder is used.
    #[serde(default)]
    pub file_contents: Vec<FileContentChunk>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileContentChunk {
    pub path: String,
    pub content: String,
}

/// Sanitized result: safe string to send to the model. No code path should send unsanitized input.
pub fn sanitize_for_llm(input: &SanitizeForLlmInput) -> String {
    let task_sanitized = sanitize_text(&input.task);

    if input.file_contents.is_empty() {
        return task_sanitized;
    }

    let mut parts = vec![task_sanitized];
    for chunk in &input.file_contents {
        if is_sensitive_path(&chunk.path) {
            parts.push(format!("{}: {}", chunk.path, SENSITIVE_FILE_PLACEHOLDER));
        } else {
            let content_sanitized = sanitize_text(&chunk.content);
            parts.push(format!("--- {} ---\n{}", chunk.path, content_sanitized));
        }
    }
    parts.join("\n\n")
}

/// Sanitize only the task string (e.g. when building LlmGenerate intent).
/// Use this when no file contents are attached; use `sanitize_for_llm` when assembling full prompt with files.
pub fn sanitize_task_for_llm(task: &str) -> String {
    sanitize_text(task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_path_ssh() {
        assert!(is_sensitive_path("~/.ssh/id_rsa"));
        assert!(is_sensitive_path("/home/u/.ssh/id_ed25519"));
        assert!(is_sensitive_path("id_rsa"));
    }

    #[test]
    fn sensitive_path_aws_pem() {
        assert!(is_sensitive_path(".aws/credentials"));
        assert!(is_sensitive_path("key.pem"));
        assert!(is_sensitive_path("secret.env"));
    }

    #[test]
    fn non_sensitive_path() {
        assert!(!is_sensitive_path("src/main.rs"));
        assert!(!is_sensitive_path("README.md"));
        assert!(!is_sensitive_path("/tmp/workspace/foo.txt"));
    }

    #[test]
    fn sanitize_text_redacts_jwt_like() {
        let s = "token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5B_X1zSNY8Q_fFwGQN26zIU";
        let out = sanitize_text(s);
        assert!(out.contains("[REDACTED:"));
        assert!(!out.contains("eyJ"));
    }

    #[test]
    fn sanitize_for_llm_sensitive_file_placeholder() {
        let input = SanitizeForLlmInput {
            task: "Explain this file".to_string(),
            file_contents: vec![FileContentChunk {
                path: "~/.ssh/id_rsa".to_string(),
                content: "secret key data".to_string(),
            }],
        };
        let out = sanitize_for_llm(&input);
        assert!(out.contains(SENSITIVE_FILE_PLACEHOLDER));
        assert!(!out.contains("secret key data"));
    }

    #[test]
    fn sanitize_for_llm_safe_file_included() {
        let input = SanitizeForLlmInput {
            task: "What does this do?".to_string(),
            file_contents: vec![FileContentChunk {
                path: "src/main.rs".to_string(),
                content: "fn main() {}".to_string(),
            }],
        };
        let out = sanitize_for_llm(&input);
        assert!(out.contains("fn main()"));
        assert!(out.contains("src/main.rs"));
    }

    /// POLICY (sensitive data / API keys): Full secrets (API keys, JWTs, private keys, AWS keys, etc.)
    /// are NEVER sent to LLMs (NeverSend). API key PREFIXES (e.g. "sk-...") are sensitive but
    /// allowed after scrubbing: payload MUST contain a REDACTED marker and must NOT contain the raw prefix.
    #[test]
    fn sanitize_text_redacts_api_key_prefix() {
        let s = "Use API key sk-1234567890abcdef for the client.";
        let out = sanitize_text(s);
        assert!(
            out.contains("[REDACTED"),
            "policy: redacted output must contain a REDACTED marker; got: {}",
            out
        );
        assert!(
            !out.contains("sk-1234567890abcdef"),
            "policy: raw API key prefix must not appear in sanitized text"
        );
    }

    #[test]
    fn sanitize_task_redacts_inline_secret() {
        let task = "Debug with Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let out = sanitize_task_for_llm(task);
        assert!(out.contains("[REDACTED:"));
        assert!(!out.contains("eyJ"));
    }

    // ---- Scrubbing pipeline ----
    #[test]
    fn scrub_for_llm_call_ok_when_little_redacted() {
        let s = "Hello world, explain Rust.";
        let opts = data_sensitivity::ClassifierOptions::default();
        let out = scrub_for_llm_call(s, &opts).unwrap();
        assert_eq!(out, s);
    }

    /// AWS keys trigger NeverSend; scrub_for_llm_call aborts. Use medium-sensitivity patterns for scrub-and-allow.
    #[test]
    fn scrub_for_llm_call_err_never_send_on_aws_key() {
        let s = "Connect with key AKIAIOSFODNN7EXAMPLE and then run the script.";
        let opts = data_sensitivity::ClassifierOptions::default();
        let r = scrub_for_llm_call(s, &opts);
        assert!(
            matches!(r, Err(ScrubbingError::NeverSendToLlm { finding_kind: k, .. }) if k == "aws_access_key_id")
        );
    }

    /// Scrub-and-allow: IP address is redacted but LLM call proceeds.
    #[test]
    fn scrubber_placeholders_preserve_surrounding_text_for_medium_sensitivity() {
        let s = "Connect to 192.168.1.1:8080 and then run the script.";
        let opts = data_sensitivity::ClassifierOptions::default();
        let out = scrub_for_llm_call(s, &opts).unwrap();
        assert!(out.starts_with("Connect to "));
        assert!(out.contains("[REDACTED:"));
        assert!(out.ends_with(" and then run the script."));
        assert!(!out.contains("192.168"));
    }

    /// One never-send pattern (AWS key) in payload -> NeverSend, not threshold.
    #[test]
    fn scrub_for_llm_call_err_never_send_on_aws_key_in_long_payload() {
        let mut s = "Explain the following code.\n\n".to_string();
        s.push_str(&"fn main() { }\n".repeat(50));
        s.push_str("\nUse key AKIAIOSFODNN7EXAMPLE for AWS.");
        let opts = data_sensitivity::ClassifierOptions::default();
        let r = scrub_for_llm_call(&s, &opts);
        assert!(
            matches!(r, Err(ScrubbingError::NeverSendToLlm { finding_kind: k, .. }) if k == "aws_access_key_id")
        );
    }

    /// Scrub-and-allow when only medium-sensitivity patterns; redacted fraction small.
    #[test]
    fn scrub_for_llm_call_ok_when_under_threshold() {
        let mut s = "Explain the following code.\n\n".to_string();
        s.push_str(&"fn main() { }\n".repeat(50));
        s.push_str("\nConnect to 192.168.1.1 for debugging.");
        let opts = data_sensitivity::ClassifierOptions::default();
        let out = scrub_for_llm_call(&s, &opts).unwrap();
        assert!(out.contains("[REDACTED:"));
        assert!(!out.contains("192.168"));
    }

    /// JWT triggers NeverSend before threshold check; we get NeverSendToLlm, not TooMuchSensitiveData.
    #[test]
    fn scrub_for_llm_call_err_never_send_on_jwt() {
        let long_jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.".to_string()
            + &"a".repeat(80)
            + "."
            + &"b".repeat(80);
        let s = format!("Task: explain this token. {}", long_jwt);
        let opts = data_sensitivity::ClassifierOptions::default();
        let r = scrub_for_llm_call(&s, &opts);
        assert!(
            matches!(r, Err(ScrubbingError::NeverSendToLlm { finding_kind: k, .. }) if k == "jwt_or_oauth_token")
        );
    }

    /// Payload with only medium-sensitivity patterns but ≥50% redacted -> TooMuchSensitiveData.
    #[test]
    fn scrub_for_llm_call_err_when_too_much_redacted() {
        let long_blob = "A".repeat(200);
        let s = format!("Short prefix. {}", long_blob);
        let opts = data_sensitivity::ClassifierOptions::default();
        let r = scrub_for_llm_call(&s, &opts);
        assert!(matches!(
            r,
            Err(ScrubbingError::TooMuchSensitiveData { .. })
        ));
        if let Err(ScrubbingError::TooMuchSensitiveData { message }) = r {
            assert!(message.contains("too much sensitive data"));
        }
    }

    #[test]
    fn scrubber_threshold_redacted_length_under_half_blocks_llm_call() {
        let opts = data_sensitivity::ClassifierOptions::default();
        let payload = "x".repeat(20)
            + "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9."
            + &"a".repeat(100)
            + "."
            + &"b".repeat(100);
        let (result, entry) = scrub_for_llm_call_audit(&payload, &opts, "sess");
        assert!(
            result.is_err(),
            "payload mostly JWT should be blocked (threshold or never-send)"
        );
        assert!(
            entry.decision == ScrubbingDecision::ScrubbedAndBlocked
                || entry.decision == ScrubbingDecision::BlockedByHardRule
        );
        assert!(entry.original_length > 0);
    }

    #[test]
    fn build_raw_payload_includes_parts() {
        let parts = LlmPayloadParts {
            task: "List files".to_string(),
            file_snippets: vec![FileContentChunk {
                path: "src/main.rs".to_string(),
                content: "fn main() {}".to_string(),
            }],
            shell_outputs: vec!["output".to_string()],
            tool_outputs: vec![],
            metadata: vec![],
        };
        let raw = build_raw_payload(&parts);
        assert!(raw.contains("List files"));
        assert!(raw.contains("src/main.rs"));
        assert!(raw.contains("fn main()"));
        assert!(raw.contains("Shell output"));
        assert!(raw.contains("output"));
    }

    #[test]
    fn build_and_scrub_for_llm_returns_scrubbed_when_no_never_send() {
        let parts = LlmPayloadParts {
            task: "Explain the code in 192.168.1.1 and ./src/main.rs".to_string(),
            ..Default::default()
        };
        let opts = data_sensitivity::ClassifierOptions::default();
        let out = build_and_scrub_for_llm(&parts, &opts).unwrap();
        assert!(
            out.contains("Explain")
                && (out.contains("[REDACTED: ip_address]") || out.contains("192"))
        );
    }

    #[test]
    fn build_and_scrub_for_llm_err_never_send_on_aws_key() {
        let parts = LlmPayloadParts {
            task: "Use key AKIAIOSFODNN7EXAMPLE then explain.".to_string(),
            ..Default::default()
        };
        let opts = data_sensitivity::ClassifierOptions::default();
        let r = build_and_scrub_for_llm(&parts, &opts);
        assert!(matches!(r, Err(ScrubbingError::NeverSendToLlm { .. })));
    }

    #[test]
    fn scrub_for_llm_call_err_never_send_on_private_key() {
        // NOTE: dummy PEM block for redaction tests only; body is clearly fake and non-sensitive.
        let payload = "Here is my key:\n-----BEGIN RSA PRIVATE KEY-----\nFAKEPRIVATEKEYDATAFORTESTS\n-----END RSA PRIVATE KEY-----";
        let opts = data_sensitivity::ClassifierOptions::default();
        let r = scrub_for_llm_call(payload, &opts);
        assert!(
            matches!(r, Err(ScrubbingError::NeverSendToLlm { finding_kind: k, .. }) if k == "ssh_private_key")
        );
    }

    /// Test names must not shadow public utility functions; use a distinct name
    /// (e.g. `to_workspace_relative_path_basic_cases`) so the real function is called.
    #[test]
    fn to_workspace_relative_path_basic_cases() {
        assert_eq!(
            to_workspace_relative_path("/Users/j/proj/src/foo.rs", "/Users/j/proj"),
            "./src/foo.rs"
        );
        assert_eq!(
            to_workspace_relative_path("/Users/j/proj", "/Users/j/proj"),
            "."
        );
        assert_eq!(
            to_workspace_relative_path("/other/file", "/Users/j/proj"),
            "/other/file"
        );
    }

    #[test]
    fn to_workspace_relative_path_edge_cases() {
        assert_eq!(
            to_workspace_relative_path("/Users/j/proj/", "/Users/j/proj"),
            "."
        );
        assert_eq!(to_workspace_relative_path("/a/b/c", "/a/b"), "./c");
    }

    #[test]
    fn scrub_for_llm_call_audit_returns_entry_with_decision() {
        let opts = data_sensitivity::ClassifierOptions::default();
        let (result, entry) = scrub_for_llm_call_audit("Hello world", &opts, "sess-1");
        assert!(result.is_ok());
        assert_eq!(entry.session_id, "sess-1");
        assert_eq!(entry.original_length, 11);
        assert_eq!(entry.decision, ScrubbingDecision::ScrubbedAndSent);
        assert!(entry.matched_kinds.is_empty());

        let (result2, entry2) = scrub_for_llm_call_audit(
            "Key: AKIAIOSFODNN7EXAMPLE and more text here.",
            &opts,
            "sess-2",
        );
        assert!(result2.is_err());
        assert_eq!(entry2.decision, ScrubbingDecision::BlockedByHardRule);
        assert!(entry2
            .matched_kinds
            .iter()
            .any(|k| k == "aws_access_key_id"));
    }
}
