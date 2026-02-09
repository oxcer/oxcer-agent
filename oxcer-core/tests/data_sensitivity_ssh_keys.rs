//! Parameterized tests for SSH key path and PEM rules.
//!
//! **Policy (SSH key paths):** We treat canonical SSH key paths as HIGH sensitivity signals.
//! Paths such as `/etc/ssh/id_rsa`, `~/.ssh/id_rsa`, `/home/<user>/.ssh/id_rsa`, and similar
//! (id_ed25519, id_ecdsa, with or without `.pub`) should be classified as HIGH and masked/redacted
//! before sending to an LLM. We accept some false positives on these canonical paths in exchange
//! for stronger protection.

use oxcer_core::data_sensitivity::{classify_and_mask_default, SensitivityLevel};

fn has_pattern(findings: &[oxcer_core::data_sensitivity::SensitivityFinding], pattern_id: &str) -> bool {
    findings.iter().any(|f| f.pattern_id == pattern_id)
}

#[test]
fn ssh_key_path_should_match() {
    let cases = [
        "See ~/.ssh/id_rsa for auth.",
        "Config at /home/dev/.ssh/id_ed25519",
        "Key file: /home/user/.ssh/id_ecdsa.pub",
        "path=/etc/ssh/id_rsa",
        "path=/home/alice/.ssh/id_rsa",
        "  ~/.ssh/id_rsa  ",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "ssh_key_path"),
            "ssh_key_path should match: {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High);
    }
}

#[test]
fn ssh_key_path_should_not_match() {
    let cases = [
        "id_rsa without path prefix",           // no path structure
        "/tmp/other_file.txt",                   // different filename
        "~/.ssh/config",                         // config, not key
        "~/.ssh/known_hosts",                    // known_hosts
        "mention id_rsa in documentation",       // word in sentence, no path
        "echo id_rsa",                           // command arg, no path
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "ssh_key_path"),
            "ssh_key_path should NOT match: {:?}",
            s
        );
    }
}

#[test]
fn pem_block_should_match() {
    let cases = [
        "-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END RSA PRIVATE KEY-----",
        "-----BEGIN OPENSSH PRIVATE KEY-----\nxyz\n-----END OPENSSH PRIVATE KEY-----",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "pem_block") || has_pattern(&r.findings, "pem_header"),
            "pem should match: {:?}",
            s
        );
        assert_eq!(r.level, SensitivityLevel::High);
    }
}

#[test]
fn pem_header_should_match() {
    let s = "key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIE...";
    let r = classify_and_mask_default(s);
    assert!(
        has_pattern(&r.findings, "pem_block") || has_pattern(&r.findings, "pem_header"),
        "pem header should match"
    );
}

#[test]
fn pem_should_not_match() {
    let cases = [
        "-----BEGIN CERTIFICATE-----",           // cert, not private key
        "-----BEGIN PUBLIC KEY-----",            // public key
        "some random base64 MIIE...",            // no PEM header
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "pem_block") && !has_pattern(&r.findings, "pem_header"),
            "pem should NOT match: {:?}",
            s
        );
    }
}
