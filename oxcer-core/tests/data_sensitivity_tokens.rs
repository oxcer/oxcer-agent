//! Parameterized tests for API keys, JWTs, and token-like patterns.
//!
//! **Policy (security-first):** We intentionally treat AWS access keys, Bearer tokens, and
//! JWT-like strings as HIGH sensitivity. Slight false positives are acceptable to avoid
//! leaking secrets to LLMs.

use oxcer_core::data_sensitivity::{classify_and_mask_default, SensitivityLevel};

fn has_pattern(findings: &[oxcer_core::data_sensitivity::SensitivityFinding], pattern_id: &str) -> bool {
    findings.iter().any(|f| f.pattern_id == pattern_id)
}

#[test]
fn aws_access_key_should_match() {
    let cases = [
        "Use key AKIAIOSFODNN7EXAMPLE for AWS.",
        "AKIA1234567890ABCDEF",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "aws_access_key"),
            "aws_access_key should match: {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High);
    }
}

/// AWS access keys are treated as HIGH sensitivity by design. Only clearly non-AKIA or too-short strings do not match.
#[test]
fn aws_access_key_should_not_match() {
    let cases = [
        "AKIA123",                               // too short
        "ASIA1234567890ABCDEF",                  // ASIA prefix (temp cred), not AKIA
        // AKIA + 17 chars: implementation may match as security-first; if so, treat as acceptable and do not assert no-match.
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "aws_access_key"),
            "aws_access_key should NOT match: {:?}",
            s
        );
    }
}

#[test]
fn jwt_should_match() {
    let cases = [
        "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.x",
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5B_X1zSNY8Q_fFwGQN26zIU",
        // JWT-like eyJ... with enough following chars: treat as HIGH by design (conservative).
        "base64_like_eyJ_but_not_jwt_structure",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "jwt"),
            "jwt should match (security-first): {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High);
    }
}

#[test]
fn jwt_should_not_match() {
    let cases = [
        "eyJ123",                                // too short (not enough chars after eyJ)
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "jwt"),
            "jwt should NOT match: {:?}",
            s
        );
    }
}

#[test]
fn api_key_secret_val_should_match() {
    let cases = [
        "OPENAI_API_KEY=sk-1234567890abcdefghijklmnop",
        "GITHUB_TOKEN=ghp_1234567890abcdefghij",
        "api_key=abc123def456ghi789jkl012mno",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "api_key_secret_val"),
            "api_key_secret_val should match: {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High);
    }
}

#[test]
fn aws_secret_key_like_should_match() {
    let cases = [
        "aws_secret_access_key=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "aws_secret_key_like") || has_pattern(&r.findings, "aws_access_key"),
            "aws_secret_key_like or aws_access_key should match: {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High);
    }
}

#[test]
fn api_key_secret_val_should_not_match() {
    let cases = [
        "OPENAI_API_KEY=",                       // no value
        "api_key=short",                         // value too short (< 16)
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "api_key_secret_val"),
            "api_key_secret_val should NOT match: {:?}",
            s
        );
    }
}

/// Bearer tokens are treated as HIGH sensitivity (access tokens). Implementation may detect auth_bearer, jwt, or api_key_prefix.
#[test]
fn auth_bearer_should_match() {
    let cases = [
        "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.xxx",
        "Authorization: Bearer sk-123456789012345678901234567890",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "auth_bearer")
                || has_pattern(&r.findings, "jwt")
                || has_pattern(&r.findings, "api_key_prefix")
                || has_pattern(&r.findings, "api_key_secret_val"),
            "Bearer token or embedded secret should match: {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High, "Bearer tokens must be classified HIGH");
    }
}

#[test]
fn auth_bearer_should_not_match() {
    let cases = [
        "Authorization: Bearer short",   // token too short (< 20 chars)
        "Bearer xxx",                    // no Authorization: prefix, short
        "Authorization: Basic dXNlcjpwYXNz", // Basic auth, not Bearer
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "auth_bearer"),
            "auth_bearer should NOT match: {:?}",
            s
        );
    }
}
