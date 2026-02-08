//! Parameterized tests for Medium-level rules (keychain, IP, base64, auth bearer).

use oxcer_core::data_sensitivity::{classify_and_mask_default, SensitivityLevel};

fn has_pattern(findings: &[oxcer_core::data_sensitivity::SensitivityFinding], pattern_id: &str) -> bool {
    findings.iter().any(|f| f.pattern_id == pattern_id)
}

#[test]
fn keychain_path_should_match() {
    let cases = [
        "~/Library/Keychains/login.keychain-db",
        "/Library/Keychains/System.keychain",
        "path to .keychain file",
        "1Password vault",
        "KeePass database",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "keychain_path"),
            "keychain_path should match: {:?}",
            s
        );
        assert!(r.level >= SensitivityLevel::Medium);
    }
}

#[test]
fn keychain_path_should_not_match() {
    let cases = [
        "keychain as a word",
        "/tmp/random/file.txt",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "keychain_path"),
            "keychain_path should NOT match: {:?}",
            s
        );
    }
}

#[test]
fn ip_port_should_match() {
    let cases = [
        "Connect to 192.168.1.1:8080",
        "Server 10.0.0.1",
        "127.0.0.1:443",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            has_pattern(&r.findings, "ip_port"),
            "ip_port should match: {:?}",
            s
        );
        assert!(r.level >= SensitivityLevel::Medium);
    }
}

#[test]
fn ip_port_should_not_match() {
    let cases = [
        "no IP address in this text",
        "the quick brown fox",
    ];
    for s in cases {
        let r = classify_and_mask_default(s);
        assert!(
            !has_pattern(&r.findings, "ip_port"),
            "ip_port should NOT match: {:?}",
            s
        );
    }
}

#[test]
fn base64_long_should_match() {
    let blob = "A".repeat(128);
    let s = format!("data: {} end", blob);
    let r = classify_and_mask_default(&s);
    assert!(
        has_pattern(&r.findings, "base64_long"),
        "base64_long should match 128+ char blob"
    );
    assert!(r.level >= SensitivityLevel::Medium);
}

#[test]
fn base64_long_should_not_match() {
    let s = "short base64 Ab1=";
    let r = classify_and_mask_default(s);
    assert!(
        !has_pattern(&r.findings, "base64_long"),
        "base64_long should NOT match short string"
    );
}
