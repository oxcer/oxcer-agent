//! Integration tests for oxcer-core.
//!
//! These tests use the crate as a library and exercise public APIs only.

use oxcer_core::security::policy_config;

/// Policy config loads from YAML and returns a valid config.
#[test]
fn policy_load_from_yaml_returns_config() {
    let yaml = r#"
version: 1
default_action: deny
rules:
  - match:
      tool_type: [fs]
      operation: [read]
    action: allow
    reason_code: test_rule
"#;
    let config = policy_config::load_from_yaml(yaml.as_bytes());
    assert_eq!(config.version, 1);
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].reason_code.as_deref(), Some("test_rule"));
}
