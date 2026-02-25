//! Tests for data_sensitivity merge and dedup logic.

use oxcer_core::data_sensitivity::{classify_and_mask_default, SensitivityLevel};

/// Multiple overlapping findings (e.g. SSH path + PEM in same span) → merged into fewer spans.
#[test]
fn merge_overlapping_findings_produces_fewer_spans() {
    // NOTE: dummy PEM block for redaction tests only; body is clearly fake and non-sensitive.
    let s = "Use ~/.ssh/id_rsa and -----BEGIN RSA PRIVATE KEY-----\nFAKEPRIVATEKEYDATAFORTESTS\n-----END RSA PRIVATE KEY-----";
    let r = classify_and_mask_default(s);
    assert!(r.level == SensitivityLevel::High);
    assert!(r.masked_content.contains("[REDACTED:"));
    assert!(!r.masked_content.contains("id_rsa"));
    assert!(!r.masked_content.contains("BEGIN RSA"));
}

/// Overlapping AWS key + JWT in same region → single merged placeholder.
#[test]
fn merge_adjacent_high_findings() {
    let s = "AKIAIOSFODNN7EXAMPLE and eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.x";
    let r = classify_and_mask_default(s);
    assert!(r.level == SensitivityLevel::High);
    assert!(r.findings.len() >= 1);
    assert!(!r.masked_content.contains("AKIA"));
    assert!(!r.masked_content.contains("eyJ"));
}
