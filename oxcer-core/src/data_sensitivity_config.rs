//! Config-driven data sensitivity rules (skeleton).
//!
//! This module defines the structure for loading rules from YAML/JSON in the future.
//! Currently, rules are hardcoded in `data_sensitivity::RULES`; this module provides
//! the loader signature and a proof-of-concept parse for 1–2 rules.

use crate::data_sensitivity::SensitivityLevel;
use serde::Deserialize;
use std::fmt;

/// Config for a single data sensitivity rule (YAML/JSON format).
#[derive(Clone, Debug, Deserialize)]
pub struct DataSensitivityRuleConfig {
    /// Stable rule ID (e.g. `aws_access_key`, `jwt`).
    pub id: String,
    /// Regex pattern string.
    pub pattern: String,
    /// Sensitivity level: "low" | "medium" | "high".
    pub level: String,
    /// If true, prompt_sanitizer returns NeverSendToLlm for this finding.
    #[serde(default)]
    pub never_send: bool,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Error returned when loading rules from YAML fails.
#[derive(Debug)]
pub enum RuleLoadError {
    Parse(String),
    Validation(String),
}

impl fmt::Display for RuleLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleLoadError::Parse(s) => write!(f, "parse error: {}", s),
            RuleLoadError::Validation(s) => write!(f, "validation error: {}", s),
        }
    }
}

impl std::error::Error for RuleLoadError {}

/// Parse a level string into SensitivityLevel.
fn parse_level(s: &str) -> Result<SensitivityLevel, RuleLoadError> {
    match s.to_lowercase().as_str() {
        "low" => Ok(SensitivityLevel::Low),
        "medium" => Ok(SensitivityLevel::Medium),
        "high" => Ok(SensitivityLevel::High),
        _ => Err(RuleLoadError::Validation(format!(
            "invalid level: {} (expected low|medium|high)",
            s
        ))),
    }
}

/// Load rules from a YAML string.
///
/// Returns a list of validated rule configs. On parse failure, returns `Err`.
/// This is a proof-of-concept: it loads a small built-in YAML for 1–2 rules,
/// or parses the given `yaml` if provided. For full migration, extend to read
/// from file and compile patterns into Regex.
pub fn load_rules_from_yaml(yaml: &str) -> Result<Vec<DataSensitivityRuleConfig>, RuleLoadError> {
    #[derive(Deserialize)]
    struct YamlRoot {
        rules: Vec<DataSensitivityRuleConfig>,
    }

    let root: YamlRoot =
        serde_yaml::from_str(yaml).map_err(|e| RuleLoadError::Parse(e.to_string()))?;

    for r in &root.rules {
        parse_level(&r.level)?;
        if r.pattern.is_empty() {
            return Err(RuleLoadError::Validation(format!(
                "rule {} has empty pattern",
                r.id
            )));
        }
    }

    Ok(root.rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"
version: 1
rules:
  - id: aws_access_key
    level: high
    never_send: true
    pattern: "AKIA[0-9A-Z]{16}"
    description: "AWS access key ID"
  - id: ip_port
    level: medium
    never_send: false
    pattern: '\b(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})(:\d{1,5})?\b'
    description: "IPv4 with optional port"
"#;

    #[test]
    fn load_rules_from_yaml_ok() {
        let rules = load_rules_from_yaml(SAMPLE_YAML).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].id, "aws_access_key");
        assert_eq!(rules[0].level, "high");
        assert!(rules[0].never_send);
        assert_eq!(rules[1].id, "ip_port");
        assert_eq!(rules[1].level, "medium");
        assert!(!rules[1].never_send);
    }

    #[test]
    fn load_rules_from_yaml_invalid_level() {
        let yaml = r#"
rules:
  - id: bad
    level: invalid
    pattern: "x"
"#;
        let err = load_rules_from_yaml(yaml).unwrap_err();
        assert!(matches!(err, RuleLoadError::Validation(_)));
    }

    #[test]
    fn load_rules_from_yaml_empty_pattern() {
        let yaml = r#"
rules:
  - id: empty
    level: high
    pattern: ""
"#;
        let err = load_rules_from_yaml(yaml).unwrap_err();
        assert!(matches!(err, RuleLoadError::Validation(_)));
    }
}
