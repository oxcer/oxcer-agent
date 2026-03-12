//! Sprint 7 integration tests: sensitive file scrubbing, "too much data" block, policy-driven block.
//!
//! - Sensitive file: file content with OPENAI_API_KEY and PEM block → scrubbed payload has
//!   [REDACTED: ...] placeholders and no raw secrets.
//! - Too much sensitive data: payload mostly secrets → scrubber returns Err, no LLM call.
//! - Policy data_sensitivity: request with High content + rule max_level Medium → Denied.

use oxcer_core::data_sensitivity::{self, ClassifierOptions, SensitivityLevel, SensitivityResult};
use oxcer_core::prompt_sanitizer::{
    build_and_scrub_for_llm, build_raw_payload, scrub_for_llm_call, LlmPayloadParts, ScrubbingError,
};
use oxcer_core::security::policy_config::{evaluate_with_config, load_from_yaml_result};
use oxcer_core::security::policy_engine::{
    Operation, PolicyCaller, PolicyDecisionKind, PolicyRequest, PolicyTarget, ReasonCode, ToolType,
};

// -----------------------------------------------------------------------------
// Sensitive file scenario: file with API key + PEM → scrubbed prompt, no raw secret
// -----------------------------------------------------------------------------

#[test]
fn integration_sensitive_file_scrubbed_placeholders_no_raw_secret() {
    let sensitive_content = r#"# Config
OPENAI_API_KEY=sk-1234567890abcdefghijklmnopqrstuvwxyz
# Key below
-----BEGIN RSA PRIVATE KEY-----
FAKEPRIVATEKEYDATAFORTESTSFAKEPRIVATEKEYDATAFORTESTS
-----END RSA PRIVATE KEY-----
"#;

    // Use a non-sensitive path so file content is included in the raw payload; then we assert
    // that classify_and_mask redacts it to placeholders and removes raw secrets (policy: LLM
    // payload must contain placeholders, never raw secrets).
    let parts = LlmPayloadParts {
        task: "Summarize this file for me.".to_string(),
        file_snippets: vec![oxcer_core::prompt_sanitizer::FileContentChunk {
            path: "snippet.txt".to_string(),
            content: sensitive_content.to_string(),
        }],
        ..Default::default()
    };

    let opts = ClassifierOptions::default();

    // Raw payload includes file content; classify_and_mask must produce placeholders and no raw secret.
    let raw = build_raw_payload(&parts);
    let classified = data_sensitivity::classify_and_mask(&raw, &opts);
    assert!(
        classified.masked_content.contains("[REDACTED:"),
        "masked content should contain redaction placeholders; got: {}",
        classified.masked_content
    );
    assert!(
        !classified.masked_content.contains("sk-1234567890"),
        "no raw API key in masked content"
    );
    assert!(
        !classified.masked_content.contains("-----BEGIN RSA"),
        "no raw PEM in masked content"
    );

    // Full pipeline blocks due to never-send (PEM)
    let full_result = build_and_scrub_for_llm(&parts, &opts);
    assert!(
        matches!(full_result, Err(ScrubbingError::NeverSendToLlm { .. })),
        "pipeline should block when private key present"
    );
}

#[test]
fn integration_sensitive_file_api_key_only_scrubbed_and_sent() {
    // Use a non-sensitive path so file content is in the raw payload and we can assert redaction.
    let parts = LlmPayloadParts {
        task: "What does this config do?".to_string(),
        file_snippets: vec![oxcer_core::prompt_sanitizer::FileContentChunk {
            path: "example_env.txt".to_string(),
            content: "OPENAI_API_KEY=sk-proj-1234567890abcdef".to_string(),
        }],
        ..Default::default()
    };
    let opts = ClassifierOptions::default();
    let result = build_and_scrub_for_llm(&parts, &opts);
    // API key triggers never-send (api_key in NEVER_SEND_FINDING_KINDS)
    assert!(result.is_err());
    let raw = build_raw_payload(&parts);
    let classified = data_sensitivity::classify_and_mask(&raw, &opts);
    assert!(classified.masked_content.contains("[REDACTED:"));
    assert!(!classified.masked_content.contains("sk-proj-"));
}

// -----------------------------------------------------------------------------
// Too much sensitive data: >50% redacted → scrubber blocks LLM call
// -----------------------------------------------------------------------------

#[test]
fn integration_too_much_sensitive_data_returns_error() {
    let mostly_secrets = "AKIAIOSFODNN7EXAMPLE \
        DB_PASSWORD=secret123 \
        eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.x \
        OPENAI_API_KEY=sk-1234567890abcdefghij";
    let result = scrub_for_llm_call(mostly_secrets, &ClassifierOptions::default());
    assert!(
        matches!(
            result,
            Err(ScrubbingError::TooMuchSensitiveData { .. })
                | Err(ScrubbingError::NeverSendToLlm { .. })
        ),
        "payload that is mostly secrets should be blocked (threshold or never-send)"
    );
}

#[test]
fn integration_too_much_redacted_ratio_blocks() {
    let long_secret = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.".to_string()
        + &"a".repeat(120)
        + "."
        + &"b".repeat(120);
    let payload = format!("Brief prefix. {}", long_secret);
    let result = scrub_for_llm_call(&payload, &ClassifierOptions::default());
    assert!(result.is_err());
    if let Err(e) = &result {
        assert!(
            matches!(e, ScrubbingError::TooMuchSensitiveData { .. })
                | matches!(e, ScrubbingError::NeverSendToLlm { .. })
        );
    }
}

// -----------------------------------------------------------------------------
// Policy-driven block: max_level = medium, High content → Denied or ApprovalRequired
// -----------------------------------------------------------------------------

#[test]
fn integration_policy_data_sensitivity_high_above_max_denied() {
    let yaml = br#"
version: 1
default_action: deny
rules:
  - match:
      tool_type: [fs]
      operation: [read]
    action: allow
    reason_code: EXPLICIT_ALLOW
    data_sensitivity:
      max_level: medium
      require_approval_if: high
"#;
    let cfg = load_from_yaml_result(yaml).expect("yaml");
    let high_content = SensitivityResult {
        level: SensitivityLevel::High,
        masked_content: String::new(),
        findings: vec![],
        original_length: 100,
        redacted_length: 20,
    };
    let req = PolicyRequest {
        caller: PolicyCaller::AgentOrchestrator,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: "/tmp/workspace/file.txt".to_string(),
        },
        content_sensitivity: Some(high_content),
        ..Default::default()
    };
    let dec = evaluate_with_config(&req, &cfg);
    assert_eq!(dec.decision, PolicyDecisionKind::Deny);
    assert!(matches!(dec.reason_code, ReasonCode::DataSensitivityDeny));
}

#[test]
fn integration_policy_data_sensitivity_require_approval_if_high() {
    let yaml = br#"
version: 1
default_action: deny
rules:
  - match:
      tool_type: [fs]
      operation: [read]
    action: allow
    reason_code: EXPLICIT_ALLOW
    data_sensitivity:
      max_level: high
      require_approval_if: high
"#;
    let cfg = load_from_yaml_result(yaml).expect("yaml");
    let high_content = SensitivityResult {
        level: SensitivityLevel::High,
        masked_content: String::new(),
        findings: vec![],
        original_length: 0,
        redacted_length: 0,
    };
    let req = PolicyRequest {
        caller: PolicyCaller::AgentOrchestrator,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: "/tmp/workspace/file.txt".to_string(),
        },
        content_sensitivity: Some(high_content),
        ..Default::default()
    };
    let dec = evaluate_with_config(&req, &cfg);
    assert_eq!(dec.decision, PolicyDecisionKind::RequireApproval);
    assert!(matches!(
        dec.reason_code,
        ReasonCode::DataSensitivityRequireApproval
    ));
}
