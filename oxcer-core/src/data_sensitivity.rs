//! Data Sensitivity Classifier & Prompt Scrubbing (DLP at the prompt boundary).
//!
//! Extends the Security Policy Engine from "which tools can run" to "which data can leave the box".
//! This module sits on the hot path of every LLM call: classify and mask secrets / sensitive
//! content **before** prompts are sent to any provider (OpenAI, Gemini, Anthropic, Grok, etc.).
//!
//! Design: regex + keyword list classifier inspired by industry DLP and secret-scanning practice.
//! All model inputs that include file content or user-supplied text should go through
//! `classify_and_mask`; no code path should bypass it when calling an LLM.
//!
//! Rule definitions live in `RULES`; config-driven loading is in `data_sensitivity_config`.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

// -----------------------------------------------------------------------------
// Public types
// -----------------------------------------------------------------------------

/// Sensitivity level for a finding; used to decide masking strength and policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SensitivityLevel {
    /// Safe with normalization (e.g. path rewriting only).
    Low,
    /// Prefer masking but sometimes allowed (e.g. IPs, keychain paths).
    Medium,
    /// Block / strongly mask (secrets, private keys, passwords).
    High,
}

/// A single detected sensitive span in the input.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SensitivityFinding {
    pub level: SensitivityLevel,
    pub span_start: usize,
    pub span_end: usize,
    pub kind: String,
    pub pattern_id: String,
}

/// Result of classifying and masking an input string.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SensitivityResult {
    /// Maximum sensitivity level across all findings.
    pub level: SensitivityLevel,
    /// Scrubbed version of the input (secrets/sensitive spans replaced by placeholders).
    pub masked_content: String,
    /// All findings (spans and kinds) before masking.
    pub findings: Vec<SensitivityFinding>,
    pub original_length: usize,
    pub redacted_length: usize,
}

/// Options for the classifier (tune masking behavior).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassifierOptions {
    /// If true, drop entire high-risk blocks when they exceed this many bytes (e.g. full PEM body).
    /// 0 = disabled.
    #[serde(default)]
    pub drop_high_risk_blocks_over_bytes: usize,
    /// If set, normalize absolute paths to workspace-relative using this prefix (e.g. "/Users/j/proj" -> "./").
    #[serde(default)]
    pub workspace_root: Option<String>,
    /// If true, apply low-sensitivity path normalization. Ignored if workspace_root is None.
    #[serde(default = "default_true")]
    pub normalize_paths: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ClassifierOptions {
    fn default() -> Self {
        Self {
            drop_high_risk_blocks_over_bytes: 0,
            workspace_root: None,
            normalize_paths: true,
        }
    }
}

// -----------------------------------------------------------------------------
// Rule metadata (single source of truth)
// -----------------------------------------------------------------------------

/// Metadata for a sensitivity rule. Matches docs/ARCHITECTURE_CORE.md and docs/DEVELOPMENT.md.
#[derive(Clone, Debug)]
pub struct RuleSpec {
    /// Stable ID used in findings and policy tables (e.g. `aws_access_key`, `jwt`).
    pub pattern_id: &'static str,
    /// Sensitivity level (High = block/mask strongly; Medium = mask; Low = path norm only).
    pub level: SensitivityLevel,
    /// Short description for docs and debugging.
    pub description: &'static str,
    /// If true, prompt_sanitizer returns NeverSendToLlm for this finding (see ARCHITECTURE_CORE).
    pub never_send: bool,
}

/// All built-in rules. Sync with docs/ARCHITECTURE_CORE.md and docs/DEVELOPMENT.md.
pub const RULES: &[RuleSpec] = &[
    // High — NeverSend
    RuleSpec {
        pattern_id: "aws_access_key",
        level: SensitivityLevel::High,
        description: "AWS access key ID (AKIA + 16 alphanumeric)",
        never_send: true,
    },
    RuleSpec {
        pattern_id: "aws_secret_key_like",
        level: SensitivityLevel::High,
        description: "aws_secret_access_key or aws_access_key_id env-style assignment",
        never_send: true,
    },
    RuleSpec {
        pattern_id: "jwt",
        level: SensitivityLevel::High,
        description: "JWT / OAuth tokens (eyJ... base64url)",
        never_send: true,
    },
    RuleSpec {
        pattern_id: "pem_block",
        level: SensitivityLevel::High,
        description: "Full PEM private key block (BEGIN … END)",
        never_send: true,
    },
    RuleSpec {
        pattern_id: "pem_header",
        level: SensitivityLevel::High,
        description: "PEM private key header only (truncated key)",
        never_send: true,
    },
    RuleSpec {
        pattern_id: "ssh_key_path",
        level: SensitivityLevel::High,
        description: "File paths containing id_rsa, id_ed25519, or id_ecdsa",
        never_send: true,
    },
    RuleSpec {
        pattern_id: "password_equals",
        level: SensitivityLevel::High,
        description: "PASSWORD=, DB_PASSWORD=, API_SECRET= etc. with value",
        never_send: true,
    },
    RuleSpec {
        pattern_id: "pass_in_url",
        level: SensitivityLevel::High,
        description: "Password in URL (user:pass@host) or pass= in query string",
        never_send: true,
    },
    RuleSpec {
        pattern_id: "env_secret_pass",
        level: SensitivityLevel::High,
        description: "*SECRET= or *PASSWORD= env vars with value",
        never_send: true,
    },
    RuleSpec {
        pattern_id: "api_key_secret_val",
        level: SensitivityLevel::High,
        description: "OPENAI_API_KEY=, GITHUB_TOKEN=, api_key=, etc. with value (16+ chars)",
        never_send: true,
    },
    // Medium — ScrubAndAllow
    RuleSpec {
        pattern_id: "keychain_path",
        level: SensitivityLevel::Medium,
        description: "Keychain paths (~/Library/Keychains, .keychain, KeePass, 1Password)",
        never_send: false,
    },
    RuleSpec {
        pattern_id: "ip_port",
        level: SensitivityLevel::Medium,
        description: "IPv4 address with optional port",
        never_send: false,
    },
    RuleSpec {
        pattern_id: "base64_long",
        level: SensitivityLevel::Medium,
        description: "Long base64 blob (128+ chars)",
        never_send: false,
    },
    RuleSpec {
        pattern_id: "auth_bearer",
        level: SensitivityLevel::Medium,
        description: "Authorization: Bearer <token> header",
        never_send: false,
    },
];

// -----------------------------------------------------------------------------
// Placeholders
// -----------------------------------------------------------------------------

fn placeholder_high(kind: &str) -> String {
    format!("[REDACTED: {}]", kind)
}

fn placeholder_medium(kind: &str) -> String {
    format!("[REDACTED: {}]", kind)
}

// -----------------------------------------------------------------------------
// Regex pattern fragments (shared across PEM/SSH rules; avoids repetition)
// -----------------------------------------------------------------------------

/// PEM key type prefix: RSA, DSA, EC, OPENSSH, or PGP. Used in BEGIN/END lines.
const PEM_KEY_TYPE: &str = r"(?:RSA |DSA |EC |OPENSSH |PGP )?";
/// Character class for word boundaries around paths (space, quotes, angle brackets, equals for key=value).
/// Uses r#"..."# to avoid E0762: single quote in [...] parses as unterminated char literal.
const PATH_BOUNDARY: &str = r#"[\s"'<>=]"#;

// ---- Pattern source strings (named for clarity; used in regex builders) ----
/// Matches: AKIA + 16 alphanumeric (AWS access key ID format).
const PATTERN_AWS_ACCESS_KEY: &str = r"AKIA[0-9A-Z]{16}";
/// Matches: aws_secret_access_key= or aws_access_key_id= with base64-like value (20+ chars).
const PATTERN_AWS_SECRET_KEY_LIKE: &str =
    r#"(?i)(aws_secret_access_key|aws_access_key_id)\s*[=:]\s*['"]?[A-Za-z0-9/+=]{20,}['"]?"#;
/// Matches: eyJ... JWT structure (base64url header + optional payload/signature segments).
const PATTERN_JWT: &str = r"eyJ[a-zA-Z0-9_-]{20,}(?:\.[a-zA-Z0-9_-]+)*";
/// Matches: PASSWORD=, DB_PASSWORD=, API_SECRET= etc. with value 4+ chars.
const PATTERN_PASSWORD_EQUALS: &str =
    r#"(?i)(?:PASSWORD|DB_PASSWORD|DATABASE_PASSWORD|API_SECRET)\s*[=:]\s*['"]?[^\s'"]{4,}['"]?"#;
/// Matches: user:pass@host or pass= in query string.
const PATTERN_PASS_IN_URL: &str =
    r#"(?i)(https?://[^:\s]+:[^@\s]+@[^\s]+)|(?:pass(?:word)?\s*[=:]\s*)[^\s&'"]+"#;
/// Matches: *SECRET= or *PASSWORD= env vars (uppercase) with value 4+ chars.
const PATTERN_ENV_SECRET_PASS: &str =
    r#"(?i)(?:^|\s)((?:[A-Z_][A-Z0-9_]*SECRET|[A-Z_][A-Z0-9_]*PASSWORD)\s*[=:]\s*['"]?)[^\s'"]{4,}"#;
/// Matches: OPENAI_API_KEY=, GITHUB_TOKEN=, api_key=, etc. with value 16+ chars.
const PATTERN_API_KEY_SECRET_VAL: &str =
    r#"(?i)(OPENAI_API_KEY|GITHUB_TOKEN|SLACK_TOKEN|STRIPE_SECRET_KEY|api_key|secret_key)\s*[=:]\s*['"]?[A-Za-z0-9_\-./=]{16,}['"]?"#;
/// Matches: Keychain paths (~/Library/Keychains, .keychain, KeePass, 1Password).
const PATTERN_KEYCHAIN_PATH: &str =
    r#"(?i)(?:~/Library/Keychains|/.*Keychain|KeePass|1Password|\.keychain)[^\s'"]*"#;
/// Matches: IPv4 address with optional :port (e.g. 192.168.1.1:8080).
const PATTERN_IP_PORT: &str = r"\b(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})(:\d{1,5})?\b";
/// Matches: Base64-like blob 128+ chars (word boundary).
const PATTERN_BASE64_LONG: &str = r"\b[A-Za-z0-9+/=]{128,}\b";
/// Matches: Authorization: Bearer <token> (20+ chars).
const PATTERN_AUTH_BEARER: &str = r"(?i)Authorization:\s*Bearer\s+[A-Za-z0-9_\-.]{20,}";
/// Matches: Standalone API key prefix (e.g. OpenAI "sk-...", Anthropic "sk-ant-...") in free text.
const PATTERN_API_KEY_PREFIX: &str = r"\bsk-[A-Za-z0-9_-]{16,}\b";

// -----------------------------------------------------------------------------
// Compiled regexes (lazy, once)
// -----------------------------------------------------------------------------

// ---- High ----
static RE_AWS_ACCESS_KEY: OnceLock<Regex> = OnceLock::new();
static RE_AWS_SECRET_KEY_LIKE: OnceLock<Regex> = OnceLock::new();
static RE_JWT: OnceLock<Regex> = OnceLock::new();
static RE_PEM_PRIVATE_KEY: OnceLock<Regex> = OnceLock::new();
static RE_PEM_BLOCK: OnceLock<Regex> = OnceLock::new();
static RE_SSH_KEY_PATH: OnceLock<Regex> = OnceLock::new();
static RE_PASSWORD_EQUALS: OnceLock<Regex> = OnceLock::new();
static RE_PASS_IN_URL: OnceLock<Regex> = OnceLock::new();
static RE_ENV_SECRET_PASS: OnceLock<Regex> = OnceLock::new();
static RE_API_KEY_SECRET_VAL: OnceLock<Regex> = OnceLock::new();
static RE_API_KEY_PREFIX: OnceLock<Regex> = OnceLock::new();

// ---- Medium ----
static RE_KEYCHAIN_PATH: OnceLock<Regex> = OnceLock::new();
static RE_IP_PORT: OnceLock<Regex> = OnceLock::new();
static RE_BASE64_LONG: OnceLock<Regex> = OnceLock::new();
static RE_AUTH_BEARER: OnceLock<Regex> = OnceLock::new();

// ---- Low (path normalization uses string logic + optional workspace_root) ----

fn re_aws_access_key() -> &'static Regex {
    RE_AWS_ACCESS_KEY
        .get_or_init(|| Regex::new(PATTERN_AWS_ACCESS_KEY).expect("aws_access_key regex"))
}

fn re_aws_secret_key_like() -> &'static Regex {
    RE_AWS_SECRET_KEY_LIKE
        .get_or_init(|| Regex::new(PATTERN_AWS_SECRET_KEY_LIKE).expect("aws_secret_key_like regex"))
}

fn re_jwt() -> &'static Regex {
    RE_JWT.get_or_init(|| Regex::new(PATTERN_JWT).expect("jwt regex"))
}

fn re_pem_private_key() -> &'static Regex {
    RE_PEM_PRIVATE_KEY.get_or_init(|| {
        let pat = format!("-----BEGIN {}PRIVATE KEY-----", PEM_KEY_TYPE);
        Regex::new(&pat).expect("pem header")
    })
}

fn re_pem_block() -> &'static Regex {
    RE_PEM_BLOCK.get_or_init(|| {
        let pat = format!(
            r"(?ms)-----BEGIN {}PRIVATE KEY-----[A-Za-z0-9+/=\s\r\n]+-----END {}PRIVATE KEY-----",
            PEM_KEY_TYPE, PEM_KEY_TYPE
        );
        Regex::new(&pat).expect("pem block")
    })
}

fn re_ssh_key_path() -> &'static Regex {
    RE_SSH_KEY_PATH.get_or_init(|| {
        // Paths containing id_rsa, id_ed25519, or id_ecdsa (optional .pub)
        let boundary = PATH_BOUNDARY;
        let pat = format!(
            r#"(?i)(?:^|{})(~?/[\w./-]*/?(?:id_rsa|id_ed25519|id_ecdsa)(?:\.pub)?)(?:{}|$)"#,
            boundary, boundary
        );
        Regex::new(&pat).expect("ssh path")
    })
}

fn re_password_equals() -> &'static Regex {
    RE_PASSWORD_EQUALS
        .get_or_init(|| Regex::new(PATTERN_PASSWORD_EQUALS).expect("password_equals regex"))
}

fn re_pass_in_url() -> &'static Regex {
    RE_PASS_IN_URL
        .get_or_init(|| Regex::new(PATTERN_PASS_IN_URL).expect("pass_in_url regex"))
}

fn re_env_secret_pass() -> &'static Regex {
    RE_ENV_SECRET_PASS
        .get_or_init(|| Regex::new(PATTERN_ENV_SECRET_PASS).expect("env_secret_pass regex"))
}

fn re_api_key_secret_val() -> &'static Regex {
    RE_API_KEY_SECRET_VAL
        .get_or_init(|| Regex::new(PATTERN_API_KEY_SECRET_VAL).expect("api_key_secret_val regex"))
}

fn re_api_key_prefix() -> &'static Regex {
    RE_API_KEY_PREFIX
        .get_or_init(|| Regex::new(PATTERN_API_KEY_PREFIX).expect("api_key_prefix regex"))
}

fn re_keychain_path() -> &'static Regex {
    RE_KEYCHAIN_PATH
        .get_or_init(|| Regex::new(PATTERN_KEYCHAIN_PATH).expect("keychain_path regex"))
}

fn re_ip_port() -> &'static Regex {
    RE_IP_PORT.get_or_init(|| Regex::new(PATTERN_IP_PORT).expect("ip_port regex"))
}

fn re_base64_long() -> &'static Regex {
    RE_BASE64_LONG
        .get_or_init(|| Regex::new(PATTERN_BASE64_LONG).expect("base64_long regex"))
}

fn re_auth_bearer() -> &'static Regex {
    RE_AUTH_BEARER
        .get_or_init(|| Regex::new(PATTERN_AUTH_BEARER).expect("auth_bearer regex"))
}

// -----------------------------------------------------------------------------
// Rule runners: collect findings (span_start, span_end, level, kind, pattern_id)
// -----------------------------------------------------------------------------

fn run_high_rules(input: &str, findings: &mut Vec<SensitivityFinding>) {
    // AWS access key (raw pattern)
    for m in re_aws_access_key().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "aws_access_key_id".to_string(),
            pattern_id: "aws_access_key".to_string(),
        });
    }
    // AWS secret / key id in env style
    for m in re_aws_secret_key_like().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "aws_credentials".to_string(),
            pattern_id: "aws_secret_key_like".to_string(),
        });
    }
    // JWT / OAuth-style token
    for m in re_jwt().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "jwt_or_oauth_token".to_string(),
            pattern_id: "jwt".to_string(),
        });
    }
    // PEM block (full key) - single regex
    for m in re_pem_block().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "ssh_private_key".to_string(),
            pattern_id: "pem_block".to_string(),
        });
    }
    // PEM header only (if not already covered by block - e.g. truncated content)
    for m in re_pem_private_key().find_iter(input) {
        if !findings.iter().any(|f| f.pattern_id == "pem_block" && m.start() >= f.span_start && m.end() <= f.span_end) {
            findings.push(SensitivityFinding {
                level: SensitivityLevel::High,
                span_start: m.start(),
                span_end: m.end(),
                kind: "pem_private_key_header".to_string(),
                pattern_id: "pem_header".to_string(),
            });
        }
    }
    // SSH key paths
    for m in re_ssh_key_path().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "ssh_private_key_path".to_string(),
            pattern_id: "ssh_key_path".to_string(),
        });
    }
    // PASSWORD=..., DB_PASSWORD=...
    for m in re_password_equals().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "password_in_env".to_string(),
            pattern_id: "password_equals".to_string(),
        });
    }
    // pass: in URL or password= in query
    for m in re_pass_in_url().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "password_in_url".to_string(),
            pattern_id: "pass_in_url".to_string(),
        });
    }
    // *SECRET= *PASSWORD= env
    for m in re_env_secret_pass().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "secret_or_password_env".to_string(),
            pattern_id: "env_secret_pass".to_string(),
        });
    }
    // OPENAI_API_KEY=, GITHUB_TOKEN=, etc.
    for m in re_api_key_secret_val().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "api_key".to_string(),
            pattern_id: "api_key_secret_val".to_string(),
        });
    }
    // Standalone API key prefix in free text (e.g. "Use API key sk-... for the client").
    for m in re_api_key_prefix().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::High,
            span_start: m.start(),
            span_end: m.end(),
            kind: "api_key_prefix".to_string(),
            pattern_id: "api_key_prefix".to_string(),
        });
    }
}

fn run_medium_rules(input: &str, findings: &mut Vec<SensitivityFinding>) {
    for m in re_keychain_path().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::Medium,
            span_start: m.start(),
            span_end: m.end(),
            kind: "keychain_or_credential_path".to_string(),
            pattern_id: "keychain_path".to_string(),
        });
    }
    for m in re_ip_port().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::Medium,
            span_start: m.start(),
            span_end: m.end(),
            kind: "ip_address".to_string(),
            pattern_id: "ip_port".to_string(),
        });
    }
    for m in re_base64_long().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::Medium,
            span_start: m.start(),
            span_end: m.end(),
            kind: "base64_block".to_string(),
            pattern_id: "base64_long".to_string(),
        });
    }
    for m in re_auth_bearer().find_iter(input) {
        findings.push(SensitivityFinding {
            level: SensitivityLevel::Medium,
            span_start: m.start(),
            span_end: m.end(),
            kind: "authorization_bearer".to_string(),
            pattern_id: "auth_bearer".to_string(),
        });
    }
}

/// Merge overlapping or adjacent findings: keep max level, merge span.
/// Input and output: `Vec<SensitivityFinding>`. Modifies in place.
fn merge_findings(findings: &mut Vec<SensitivityFinding>) {
    if findings.is_empty() {
        return;
    }
    findings.sort_by_key(|f| (f.span_start, std::cmp::Reverse(f.level)));
    let merged = merge_overlapping_spans(findings.drain(..).collect());
    *findings = merged;
}

/// Merge overlapping/adjacent findings into a single list. Higher-level findings win when overlapping.
fn merge_overlapping_spans(mut items: Vec<SensitivityFinding>) -> Vec<SensitivityFinding> {
    let mut merged: Vec<SensitivityFinding> = Vec::with_capacity(items.len());
    for f in items.drain(..) {
        if let Some(last) = merged.last_mut() {
            if f.span_start <= last.span_end {
                last.span_end = last.span_end.max(f.span_end);
                if f.level > last.level {
                    last.level = f.level;
                    last.kind = f.kind.clone();
                    last.pattern_id = f.pattern_id.clone();
                }
                continue;
            }
        }
        merged.push(f);
    }
    merged
}

/// Remove any finding that is entirely contained in a higher-level finding (so we don't double-mask).
fn dedup_contained(findings: &mut Vec<SensitivityFinding>) {
    findings.sort_by_key(|f| (f.span_start, std::cmp::Reverse(f.level)));
    let mut out: Vec<SensitivityFinding> = Vec::with_capacity(findings.len());
    for f in findings.drain(..) {
        let contained = out.iter().any(|o: &SensitivityFinding| {
            f.span_start >= o.span_start && f.span_end <= o.span_end && o.level >= f.level
        });
        if !contained {
            out.push(f);
        }
    }
    *findings = out;
}

/// Apply replacements from end to start so indices stay valid. Optionally drop large high-risk blocks.
fn apply_masking(
    input: &str,
    findings: &[SensitivityFinding],
    options: &ClassifierOptions,
) -> String {
    let drop_over = options.drop_high_risk_blocks_over_bytes;
    let mut out = input.to_string();
    for f in findings.iter().rev() {
        let len = f.span_end.saturating_sub(f.span_start);
        let drop_block = drop_over > 0
            && f.level == SensitivityLevel::High
            && len > drop_over;
        let replacement = if drop_block {
            String::new()
        } else {
            match f.level {
                SensitivityLevel::High => placeholder_high(&f.kind),
                SensitivityLevel::Medium => placeholder_medium(&f.kind),
                SensitivityLevel::Low => continue,
            }
        };
        if f.span_start < out.len() && f.span_end <= out.len() {
            out.replace_range(f.span_start..f.span_end, &replacement);
        }
    }
    out
}

/// Path normalization (low): replace absolute paths with workspace-relative when workspace_root is set.
fn normalize_paths_low(content: &str, workspace_root: &str) -> String {
    let mut out = content.to_string();
    let root = workspace_root.trim_end_matches('/');
    if !root.is_empty() && out.contains(root) {
        out = out.replace(root, ".");
    }
    if let Some(home) = dirs_next::home_dir() {
        let home_str = home.to_string_lossy().trim_end_matches('/').to_string();
        if !home_str.is_empty() && out.contains(&home_str) {
            out = out.replace(&home_str, "~");
        }
    }
    out
}

// -----------------------------------------------------------------------------
// Public API
// -----------------------------------------------------------------------------

/// Classify the input for sensitivity and return a scrubbed string plus findings.
/// Use this on every piece of text that may be sent to an LLM.
pub fn classify_and_mask(input: &str, options: &ClassifierOptions) -> SensitivityResult {
    let original_length = input.len();
    let mut findings = Vec::new();

    run_high_rules(input, &mut findings);
    run_medium_rules(input, &mut findings);

    merge_findings(&mut findings);
    dedup_contained(&mut findings);

    let mut final_content = apply_masking(input, &findings, options);
    if options.normalize_paths {
        if let Some(ref root) = options.workspace_root {
            final_content = normalize_paths_low(&final_content, root);
        }
    }

    let level = findings
        .iter()
        .map(|f| f.level)
        .max()
        .unwrap_or(SensitivityLevel::Low);
    let redacted_length = final_content.len();

    SensitivityResult {
        level,
        masked_content: final_content,
        findings,
        original_length,
        redacted_length,
    }
}

/// Convenience: classify and mask with default options (no path normalization, no block dropping).
pub fn classify_and_mask_default(input: &str) -> SensitivityResult {
    classify_and_mask(input, &ClassifierOptions::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_aws_access_key() {
        let s = "Use key AKIAIOSFODNN7EXAMPLE for AWS.";
        let r = classify_and_mask_default(s);
        assert_eq!(r.level, SensitivityLevel::High);
        assert!(r.masked_content.contains("[REDACTED: aws_access_key_id]"));
        assert!(!r.masked_content.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn high_jwt() {
        let s = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5B_X1zSNY8Q_fFwGQN26zIU";
        let r = classify_and_mask_default(s);
        assert_eq!(r.level, SensitivityLevel::High);
        assert!(r.findings.iter().any(|f| f.kind == "jwt_or_oauth_token"));
    }

    #[test]
    fn high_pem_header() {
        // NOTE: dummy PEM header + body for redaction tests only; this is not a real key.
        let s = "key:\n-----BEGIN RSA PRIVATE KEY-----\nFAKEPRIVATEKEYDATAFORTESTS";
        let r = classify_and_mask_default(s);
        assert_eq!(r.level, SensitivityLevel::High);
        assert!(r.masked_content.contains("[REDACTED:"));
    }

    #[test]
    fn high_ssh_path() {
        let s = "See ~/.ssh/id_rsa for auth.";
        let r = classify_and_mask_default(s);
        assert_eq!(r.level, SensitivityLevel::High);
        assert!(r.findings.iter().any(|f| f.kind == "ssh_private_key_path"));
    }

    #[test]
    fn high_password_env() {
        let s = "DB_PASSWORD=super_secret_123";
        let r = classify_and_mask_default(s);
        assert_eq!(r.level, SensitivityLevel::High);
        assert!(r.masked_content.contains("[REDACTED:"));
    }

    #[test]
    fn medium_ip() {
        let s = "Connect to 192.168.1.1:8080";
        let r = classify_and_mask_default(s);
        assert!(r.level >= SensitivityLevel::Medium);
        assert!(r.masked_content.contains("[REDACTED: ip_address]"));
    }

    #[test]
    fn result_lengths() {
        let s = "Hello world";
        let r = classify_and_mask_default(s);
        assert_eq!(r.level, SensitivityLevel::Low);
        assert_eq!(r.original_length, s.len());
        assert_eq!(r.masked_content, "Hello world");
    }

    /// Ensures call sites that iterate over Vec<String> can pass &s to classify_and_mask_default.
    #[test]
    fn classify_and_mask_default_accepts_borrowed_string_in_iteration() {
        let cases: Vec<String> = vec![
            "Hello".to_string(),
            "AKIAIOSFODNN7EXAMPLE".to_string(),
        ];
        for s in &cases {
            let r = classify_and_mask_default(s);
            assert!(r.original_length > 0);
        }
    }

    #[test]
    fn options_drop_large_blocks() {
        let mut opts = ClassifierOptions::default();
        opts.drop_high_risk_blocks_over_bytes = 50;
        // NOTE: dummy PEM block for redaction tests only; body is clearly fake and non-sensitive.
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nFAKEPRIVATEKEYDATAFORTESTSFAKEPRIVATEKEYDATAFORTESTSFAKEPRIVATEKEYDATA\n-----END RSA PRIVATE KEY-----";
        let r = classify_and_mask(pem, &opts);
        assert_eq!(r.level, SensitivityLevel::High);
        assert!(r.masked_content.len() < pem.len());
    }

    // ---- Classifier: API keys, specific kinds ----
    #[test]
    fn high_openai_api_key_env() {
        let s = "OPENAI_API_KEY=sk-1234567890abcdefghijklmnop";
        let r = classify_and_mask_default(s);
        assert_eq!(r.level, SensitivityLevel::High);
        assert!(r.findings.iter().any(|f| f.kind == "api_key"));
        assert!(!r.masked_content.contains("sk-1234567890"));
    }

    #[test]
    fn high_ssh_id_ed25519_path() {
        let s = "Config at /home/dev/.ssh/id_ed25519";
        let r = classify_and_mask_default(s);
        assert_eq!(r.level, SensitivityLevel::High);
        assert!(r.findings.iter().any(|f| f.kind == "ssh_private_key_path"));
    }

    #[test]
    fn medium_long_base64_blob() {
        let blob = "A".repeat(128);
        let s = format!("data: {} end", blob);
        let r = classify_and_mask_default(&s);
        assert!(r.level >= SensitivityLevel::Medium);
        assert!(r.findings.iter().any(|f| f.kind == "base64_block"));
    }

    #[test]
    fn medium_port_in_ip() {
        let s = "Server 10.0.0.1:443 and 10.0.0.2:8080";
        let r = classify_and_mask_default(s);
        assert!(r.level >= SensitivityLevel::Medium);
        assert!(r.masked_content.contains("[REDACTED: ip_address]"));
    }

    #[test]
    fn low_normal_code_unchanged() {
        let s = "fn main() { println!(\"hello\"); }";
        let r = classify_and_mask_default(s);
        assert_eq!(r.level, SensitivityLevel::Low);
        assert_eq!(r.masked_content, s);
        assert!(r.findings.is_empty());
    }

    #[test]
    fn low_workspace_path_normalized() {
        let root = "/Users/jane/project";
        let s = format!("See {} for the crate.", root);
        let mut opts = ClassifierOptions::default();
        opts.workspace_root = Some(root.to_string());
        opts.normalize_paths = true;
        let r = classify_and_mask(&s, &opts);
        assert_eq!(r.level, SensitivityLevel::Low);
        assert!(r.masked_content.contains(".") && !r.masked_content.contains("/Users/jane/project"));
    }

    /// Micro-bench style: classify_and_mask_default on a long synthetic input.
    /// Asserts completion within a loose time bound (no explicit timeout; just that it finishes).
    /// Run with: cargo test -p oxcer-core data_sensitivity_completes_on_long_input
    #[test]
    fn data_sensitivity_completes_on_long_input() {
        let chunk = "fn main() { println!(\"hello\"); }\n";
        let long: String = chunk.repeat(1500); // ~50K chars
        let r = classify_and_mask_default(&long);
        assert_eq!(r.level, SensitivityLevel::Low);
        assert_eq!(r.original_length, long.len());
        assert!(r.findings.is_empty());
    }
}
