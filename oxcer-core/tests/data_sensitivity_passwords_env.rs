//! Parameterized tests for password and env-style secret rules.

use oxcer_core::data_sensitivity::{classify_and_mask_default, SensitivityLevel};

fn has_pattern(findings: &[oxcer_core::data_sensitivity::SensitivityFinding], pattern_id: &str) -> bool {
    findings.iter().any(|f| f.pattern_id == pattern_id)
}

#[test]
fn password_equals_should_match() {
    let cases = [
        "DB_PASSWORD=super_secret_123",
        "PASSWORD=mysecret",
        "API_SECRET=abcdefgh",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "password_equals") || has_pattern(&r.findings, "env_secret_pass"),
            "password/secret rule should match: {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High);
    }
}

#[test]
fn password_equals_should_not_match() {
    let cases = [
        "PASSWORD=",                             // no value
        "PASS=ab",                               // too short
        "NON_PASSWORD=something",                // different var
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        let has = has_pattern(&r.findings, "password_equals");
        assert!(!has, "password_equals should NOT match: {:?}", s);
    }
}

#[test]
fn pass_in_url_should_match() {
    let cases = [
        "https://user:secret@example.com/path",
        "http://foo:bar@host.com",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "pass_in_url"),
            "pass_in_url should match: {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High);
    }
}

#[test]
fn pass_in_url_should_not_match() {
    let cases = [
        "https://example.com/path",           // no user:pass
        "http://user@host.com",               // user but no password
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "pass_in_url"),
            "pass_in_url should NOT match: {:?}",
            s
        );
    }
}

#[test]
fn env_secret_pass_should_match() {
    let cases = [
        "MY_SECRET=abc123xyz",
        "DATABASE_PASSWORD=secret",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "env_secret_pass") || has_pattern(&r.findings, "password_equals"),
            "env secret/password should match: {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High);
    }
}

#[test]
fn env_secret_pass_should_not_match() {
    let cases = [
        "MY_SECRET=",                         // no value
        "PUBLIC_KEY=abc",                     // no SECRET or PASSWORD in name
        "API_URL=https://example.com",        // different var
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "env_secret_pass"),
            "env_secret_pass should NOT match: {:?}",
            s
        );
    }
}
