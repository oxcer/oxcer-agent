//! Policy definition & configuration: data-driven policies (YAML/JSON).
//!
//! Policies are expressed as data instead of hard-coding. The loader validates
//! the schema and fails safely: invalid policy -> secure default (default-deny).

use serde::{Deserialize, Serialize};

use crate::data_sensitivity::SensitivityLevel;

use super::policy_engine::{Operation, PolicyCaller, PolicyTarget, ToolType};

// -----------------------------------------------------------------------------
// Policy schema
// -----------------------------------------------------------------------------

/// Policy file root.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_action")]
    pub default_action: PolicyAction,
    pub rules: Vec<PolicyRule>,
}

fn default_version() -> u32 {
    1
}

fn default_action() -> PolicyAction {
    PolicyAction::Deny
}

/// Action for a matching rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    Deny,
    RequireApproval,
}

/// Match criteria for a rule.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PolicyMatch {
    /// Caller(s) to match; "*" or absent = any.
    #[serde(default)]
    pub caller: Option<Vec<String>>,
    /// Tool type(s); "*" or absent = any.
    #[serde(default)]
    pub tool_type: Option<Vec<String>>,
    /// Operation(s); absent = any.
    #[serde(default)]
    pub operation: Option<Vec<String>>,
    /// Path patterns (home-relative, e.g. ~/.ssh, ~/.aws); for FS.
    #[serde(default)]
    pub path_patterns: Option<Vec<String>>,
    /// Command patterns (exact or substring); for Shell.
    #[serde(default)]
    pub command_patterns: Option<Vec<String>>,
}

/// Data-sensitivity constraint for a rule: when content is intended for the LLM,
/// compare classification level to these bounds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DataSensitivityRule {
    /// Deny (or require approval) if content sensitivity is above this level.
    #[serde(rename = "max_level")]
    pub max_level: SensitivityLevelConfig,
    /// If set, force RequireApproval when content sensitivity >= this level.
    #[serde(default, rename = "require_approval_if")]
    pub require_approval_if: Option<SensitivityLevelConfig>,
}

/// Config representation of sensitivity level ("low" | "medium" | "high").
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensitivityLevelConfig {
    Low,
    Medium,
    High,
}

impl SensitivityLevelConfig {
    fn to_level(self) -> SensitivityLevel {
        match self {
            SensitivityLevelConfig::Low => SensitivityLevel::Low,
            SensitivityLevelConfig::Medium => SensitivityLevel::Medium,
            SensitivityLevelConfig::High => SensitivityLevel::High,
        }
    }
}


/// A single policy rule.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyRule {
    #[serde(rename = "match")]
    pub match_: PolicyMatch,
    pub action: PolicyAction,
    #[serde(default)]
    pub reason_code: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub risk_level: Option<String>,
    /// Optional data-sensitivity constraint: when request includes content_sensitivity,
    /// apply max_level and require_approval_if before the rule action.
    #[serde(default)]
    pub data_sensitivity: Option<DataSensitivityRule>,
}

// -----------------------------------------------------------------------------
// Loader
// -----------------------------------------------------------------------------

/// Generic result for policy load operations (private; use for collect).
type LoadResultOf<T> = Result<T, PolicyLoadError>;

/// Result of loading a policy config (public API).
pub type LoadResult = Result<PolicyConfig, PolicyLoadError>;

#[derive(Debug)]
pub enum PolicyLoadError {
    Io(std::io::Error),
    Parse(String),
    Validation(String),
}

impl std::fmt::Display for PolicyLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyLoadError::Io(e) => write!(f, "policy load io error: {}", e),
            PolicyLoadError::Parse(e) => write!(f, "policy parse error: {}", e),
            PolicyLoadError::Validation(e) => write!(f, "policy validation error: {}", e),
        }
    }
}

impl std::error::Error for PolicyLoadError {}

/// Loads policy from YAML bytes. On parse/validation failure, returns
/// secure default (default-deny). Fails safely.
///
/// ## Secure fallback invariant
///
/// Invalid or malformed policy (unknown action, missing match, empty rules,
/// etc.) never weakens security. The loader either returns `default_policy()`
/// (default-deny, blocklists preserved) or rejects with validation error.
pub fn load_from_yaml(bytes: &[u8]) -> PolicyConfig {
    match load_from_yaml_result(bytes) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("[policy] load failed, using secure default: {}", e);
            default_policy()
        }
    }
}

/// Loads policy from JSON bytes. Same fail-safe behavior as `load_from_yaml`.
pub fn load_from_json(bytes: &[u8]) -> PolicyConfig {
    match load_from_json_result(bytes) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("[policy] load failed, using secure default: {}", e);
            default_policy()
        }
    }
}

/// Loads policy from JSON bytes. Returns error on failure.
pub fn load_from_json_result(bytes: &[u8]) -> LoadResult {
    let raw: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| PolicyLoadError::Parse(e.to_string()))?;

    let rules: Vec<PolicyRule> = raw
        .get("rules")
        .and_then(|v| v.as_array())
        .ok_or_else(|| PolicyLoadError::Validation("missing 'rules' array".into()))?
        .iter()
        .map(|v| {
            serde_json::from_value(v.clone()).map_err(|e| PolicyLoadError::Parse(e.to_string()))
        })
        .collect::<LoadResultOf<Vec<PolicyRule>>>()?;

    let default_action = raw
        .get("default_action")
        .and_then(|v| serde_json::from_value::<PolicyAction>(v.clone()).ok())
        .unwrap_or(PolicyAction::Deny);

    let version = raw.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

    let config = PolicyConfig {
        version,
        default_action,
        rules: rules.clone(),
    };
    validate(&config)?;
    Ok(config)
}

/// Loads policy from YAML bytes. Returns error on failure.
pub fn load_from_yaml_result(bytes: &[u8]) -> LoadResult {
    let raw: serde_yaml::Value = serde_yaml::from_slice(bytes)
        .map_err(|e| PolicyLoadError::Parse(e.to_string()))?;

    let rules: Vec<PolicyRule> = if let Some(arr) = raw.get("rules").and_then(|v| v.as_sequence()) {
        arr.iter()
            .map(|v| {
                serde_yaml::from_value(v.clone())
                    .map_err(|e| PolicyLoadError::Parse(e.to_string()))
            })
            .collect::<LoadResultOf<Vec<PolicyRule>>>()?
    } else {
        return Err(PolicyLoadError::Validation("missing 'rules' array".into()));
    };

    let default_action = raw
        .get("default_action")
        .and_then(|v| serde_yaml::from_value::<PolicyAction>(v.clone()).ok())
        .unwrap_or(PolicyAction::Deny);

    let version = raw.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

    validate(&PolicyConfig {
        version,
        default_action,
        rules: rules.clone(),
    })?;

    Ok(PolicyConfig {
        version,
        default_action,
        rules,
    })
}

/// Validates policy config. Rejects configs that could weaken security.
/// Invalid policy -> caller must use default_policy() (fail-safe).
fn validate(cfg: &PolicyConfig) -> LoadResultOf<()> {
    if cfg.rules.is_empty() {
        return Err(PolicyLoadError::Validation("rules must not be empty".into()));
    }
    for (i, rule) in cfg.rules.iter().enumerate() {
        if !rule_has_match_criteria(&rule.match_) {
            return Err(PolicyLoadError::Validation(format!(
                "rule {}: match must have at least one criterion (caller, tool_type, operation, path_patterns, or command_patterns)",
                i
            )));
        }
    }
    Ok(())
}

/// Returns true if the rule has at least one restricting criterion.
/// Prevents rules like `match: {} action: allow` that would match everything.
fn rule_has_match_criteria(m: &PolicyMatch) -> bool {
    let has_caller = m.caller.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
    let has_tool = m.tool_type.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
    let has_op = m.operation.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
    let has_path = m.path_patterns.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
    let has_cmd = m.command_patterns.as_ref().map(|v| !v.is_empty()).unwrap_or(false);
    has_caller || has_tool || has_op || has_path || has_cmd
}

/// Returns the secure default policy. Used when file load fails.
/// Hard-coded to avoid recursion; ensures fail-safe (default-deny).
pub fn default_policy() -> PolicyConfig {
    PolicyConfig {
        version: 1,
        default_action: PolicyAction::Deny,
        rules: builtin_default_rules(),
    }
}

/// Merges extra rules (e.g. from plugins) into a base config.
/// Extra rules are prepended so they are evaluated first (plugin-specific rules match before general allow).
pub fn merge_rules(mut base: PolicyConfig, extra: Vec<PolicyRule>) -> PolicyConfig {
    let mut rules = extra;
    rules.append(&mut base.rules);
    base.rules = rules;
    base
}

/// Hard-coded default rules (same as default.yaml). Used when YAML parse fails.
fn builtin_default_rules() -> Vec<PolicyRule> {
    vec![
        PolicyRule {
            match_: PolicyMatch {
                path_patterns: Some(vec![
                    "~/.ssh".into(),
                    "~/.aws".into(),
                    "~/.gnupg".into(),
                    "~/Library/Keychains".into(),
                    "~/Library/Passwords".into(),
                    "~/.env".into(),
                    "~/.env.local".into(),
                    "~/.env.production".into(),
                    "~/.env.development".into(),
                    "~/.netrc".into(),
                    "~/.git-credentials".into(),
                    "~/.docker/config.json".into(),
                    "~/.kube/config".into(),
                    "~/.terraform".into(),
                    "~/.config/gcloud".into(),
                    "~/.azure".into(),
                ]),
                caller: None,
                tool_type: None,
                operation: None,
                command_patterns: None,
            },
            action: PolicyAction::Deny,
            reason_code: Some("FS_PATH_IN_BLOCKLIST".into()),
            notes: Some("Sensitive credential paths".into()),
            risk_level: Some("high".into()),
            data_sensitivity: None,
        },
        PolicyRule {
            match_: PolicyMatch {
                tool_type: Some(vec!["shell".into()]),
                command_patterns: Some(vec![
                    "rm".into(),
                    "sudo".into(),
                    "su".into(),
                    "dd".into(),
                    "mkfs".into(),
                    "fdisk".into(),
                    "rm -rf".into(),
                    " sudo ".into(),
                ]),
                caller: None,
                operation: None,
                path_patterns: None,
            },
            action: PolicyAction::Deny,
            reason_code: Some("SHELL_COMMAND_BLACKLISTED".into()),
            notes: Some("Destructive/privilege commands".into()),
            risk_level: Some("high".into()),
            data_sensitivity: None,
        },
        PolicyRule {
            match_: PolicyMatch {
                tool_type: Some(vec!["fs".into()]),
                operation: Some(vec!["delete".into(), "rename".into(), "move".into(), "chmod".into()]),
                caller: None,
                path_patterns: None,
                command_patterns: None,
            },
            action: PolicyAction::RequireApproval,
            reason_code: Some("DESTRUCTIVE_FS_REQUIRES_APPROVAL".into()),
            notes: None,
            risk_level: Some("high".into()),
            data_sensitivity: None,
        },
        PolicyRule {
            match_: PolicyMatch {
                tool_type: Some(vec!["shell".into()]),
                operation: Some(vec!["exec".into()]),
                command_patterns: Some(vec!["deploy".into(), "push".into(), "migrate".into(), "release".into(), "publish".into()]),
                caller: None,
                path_patterns: None,
            },
            action: PolicyAction::RequireApproval,
            reason_code: Some("HIGH_RISK_TOOL_REQUIRES_APPROVAL".into()),
            notes: None,
            risk_level: Some("high".into()),
            data_sensitivity: None,
        },
        PolicyRule {
            match_: PolicyMatch {
                caller: Some(vec!["internal_system".into()]),
                tool_type: None,
                operation: None,
                path_patterns: None,
                command_patterns: None,
            },
            action: PolicyAction::Allow,
            reason_code: Some("INTERNAL_SYSTEM".into()),
            notes: None,
            risk_level: None,
            data_sensitivity: None,
        },
        PolicyRule {
            match_: PolicyMatch {
                caller: Some(vec!["agent_orchestrator".into()]),
                tool_type: Some(vec!["fs".into()]),
                operation: Some(vec!["read".into()]),
                path_patterns: None,
                command_patterns: None,
            },
            action: PolicyAction::Allow,
            reason_code: Some("EXPLICIT_ALLOW".into()),
            notes: None,
            risk_level: None,
            data_sensitivity: None,
        },
        PolicyRule {
            match_: PolicyMatch {
                caller: Some(vec!["agent_orchestrator".into()]),
                tool_type: Some(vec!["fs".into()]),
                operation: Some(vec!["write".into()]),
                path_patterns: None,
                command_patterns: None,
            },
            action: PolicyAction::RequireApproval,
            reason_code: Some("AGENT_WRITE_REQUIRES_APPROVAL".into()),
            notes: None,
            risk_level: Some("high".into()),
            data_sensitivity: None,
        },
        PolicyRule {
            match_: PolicyMatch {
                caller: Some(vec!["agent_orchestrator".into()]),
                tool_type: Some(vec!["shell".into()]),
                operation: Some(vec!["exec".into()]),
                path_patterns: None,
                command_patterns: None,
            },
            action: PolicyAction::RequireApproval,
            reason_code: Some("AGENT_EXEC_REQUIRES_APPROVAL".into()),
            notes: None,
            risk_level: Some("high".into()),
            data_sensitivity: None,
        },
        PolicyRule {
            match_: PolicyMatch {
                caller: Some(vec!["ui".into()]),
                tool_type: Some(vec!["fs".into()]),
                operation: Some(vec!["read".into(), "write".into()]),
                path_patterns: None,
                command_patterns: None,
            },
            action: PolicyAction::Allow,
            reason_code: Some("EXPLICIT_ALLOW".into()),
            notes: None,
            risk_level: None,
            data_sensitivity: None,
        },
        PolicyRule {
            match_: PolicyMatch {
                caller: Some(vec!["ui".into()]),
                tool_type: Some(vec!["shell".into()]),
                operation: Some(vec!["exec".into()]),
                path_patterns: None,
                command_patterns: None,
            },
            action: PolicyAction::Allow,
            reason_code: Some("EXPLICIT_ALLOW".into()),
            notes: None,
            risk_level: None,
            data_sensitivity: None,
        },
    ]
}

// -----------------------------------------------------------------------------
// Match evaluation
// -----------------------------------------------------------------------------

fn str_to_caller(s: &str) -> Option<PolicyCaller> {
    match s.to_uppercase().as_str() {
        "UI" => Some(PolicyCaller::Ui),
        "AGENT_ORCHESTRATOR" | "AGENT" => Some(PolicyCaller::AgentOrchestrator),
        "INTERNAL_SYSTEM" | "INTERNAL" => Some(PolicyCaller::InternalSystem),
        _ => None,
    }
}

fn str_to_tool_type(s: &str) -> Option<ToolType> {
    match s.to_uppercase().as_str() {
        "FS" => Some(ToolType::Fs),
        "SHELL" => Some(ToolType::Shell),
        "AGENT" => Some(ToolType::Agent),
        "WEB" => Some(ToolType::Web),
        "OTHER" => Some(ToolType::Other),
        _ => None,
    }
}

fn str_to_operation(s: &str) -> Option<Operation> {
    match s.to_lowercase().as_str() {
        "read" => Some(Operation::Read),
        "write" => Some(Operation::Write),
        "delete" => Some(Operation::Delete),
        "rename" => Some(Operation::Rename),
        "move" => Some(Operation::Move),
        "chmod" => Some(Operation::Chmod),
        "exec" => Some(Operation::Exec),
        _ => None,
    }
}

/// Returns true if the rule matches the request.
pub fn rule_matches(
    rule: &PolicyRule,
    caller: PolicyCaller,
    tool_type: ToolType,
    operation: Operation,
    target: &PolicyTarget,
    home_dir: Option<&std::path::Path>,
) -> bool {
    let m = &rule.match_;

    if let Some(ref callers) = m.caller {
        if !callers.iter().any(|c| {
            c == "*" || str_to_caller(c).map(|pc| pc == caller).unwrap_or(false)
        }) {
            return false;
        }
    }

    if let Some(ref tt) = m.tool_type {
        if !tt.iter().any(|t| {
            t == "*" || str_to_tool_type(t).map(|pt| pt == tool_type).unwrap_or(false)
        }) {
            return false;
        }
    }

    if let Some(ref ops) = m.operation {
        if !ops.iter().any(|o| {
            str_to_operation(o).map(|po| po == operation).unwrap_or(false)
        }) {
            return false;
        }
    }

    if let Some(ref patterns) = m.path_patterns {
        let path_str = match target {
            PolicyTarget::FsPath { canonical_path } => canonical_path.as_str(),
            _ => return false,
        };
        let abs = std::path::Path::new(path_str);
        let matches = patterns.iter().any(|p| {
            let expanded = expand_home_pattern(p, home_dir);
            abs.starts_with(&expanded)
        });
        if !matches {
            return false;
        }
    }

    if let Some(ref patterns) = m.command_patterns {
        let (cmd_id, normalized) = match target {
            PolicyTarget::ShellCommand {
                command_id,
                normalized_command,
            } => (command_id.as_str(), normalized_command.as_deref()),
            _ => return false,
        };
        let lower_id = cmd_id.to_lowercase();
        let matches = patterns.iter().any(|p| {
            let pl = p.to_lowercase();
            lower_id == pl || lower_id.contains(&pl)
                || normalized.map(|n| n.to_lowercase().contains(&pl)).unwrap_or(false)
        });
        if !matches {
            return false;
        }
    }

    true
}

fn expand_home_pattern(pattern: &str, home_dir: Option<&std::path::Path>) -> std::path::PathBuf {
    let s = pattern.trim();
    if s.starts_with("~/") {
        if let Some(home) = home_dir {
            return home.join(&s[2..]);
        }
    }
    if s == "~" {
        if let Some(home) = home_dir {
            return home.to_path_buf();
        }
    }
    std::path::PathBuf::from(s)
}

// -----------------------------------------------------------------------------
// Config-driven evaluation
// -----------------------------------------------------------------------------

/// Evaluates a request against a loaded policy config.
/// Rules are checked in order; first match wins.
/// If no rule matches, uses default_action (secure default: deny).
pub fn evaluate_with_config(
    request: &crate::security::policy_engine::PolicyRequest,
    config: &PolicyConfig,
) -> crate::security::policy_engine::PolicyDecision {
    use crate::security::policy_engine::{
        PolicyDecision, PolicyDecisionKind, ReasonCode,
    };

    let home_dir = dirs_next::home_dir();
    let home_path = home_dir.as_deref();

    for rule in &config.rules {
        if rule_matches(
            rule,
            request.caller,
            request.tool_type,
            request.operation,
            &request.target,
            home_path,
        ) {
            if let (Some(ds), Some(sr)) = (&rule.data_sensitivity, &request.content_sensitivity) {
                let max_level = ds.max_level.to_level();
                if sr.level > max_level {
                    return PolicyDecision {
                        decision: PolicyDecisionKind::Deny,
                        reason_code: ReasonCode::DataSensitivityDeny,
                        metadata: Some(serde_json::json!({
                            "data_sensitivity": "level above max_level",
                            "level": format!("{:?}", sr.level),
                            "max_level": format!("{:?}", max_level),
                        })),
                    };
                }
                if ds.require_approval_if
                    .map(|c| sr.level >= c.to_level())
                    .unwrap_or(false)
                {
                    return PolicyDecision {
                        decision: PolicyDecisionKind::RequireApproval,
                        reason_code: ReasonCode::DataSensitivityRequireApproval,
                        metadata: Some(serde_json::json!({
                            "data_sensitivity": "require_approval_if",
                            "level": format!("{:?}", sr.level),
                        })),
                    };
                }
            }
            let decision = match rule.action {
                PolicyAction::Allow => PolicyDecisionKind::Allow,
                PolicyAction::Deny => PolicyDecisionKind::Deny,
                PolicyAction::RequireApproval => PolicyDecisionKind::RequireApproval,
            };
            let reason_code = reason_code_from_str(
                rule.reason_code.as_deref().unwrap_or("CONFIG_RULE"),
            );
            return PolicyDecision {
                decision,
                reason_code,
                metadata: Some(serde_json::json!({
                    "rule_action": format!("{:?}", rule.action),
                    "target": request.target,
                    "risk_level": rule.risk_level,
                    "notes": rule.notes,
                })),
            };
        }
    }

    // No rule matched — use default action (secure: deny)
    PolicyDecision {
        decision: match config.default_action {
            PolicyAction::Allow => PolicyDecisionKind::Allow,
            PolicyAction::Deny => PolicyDecisionKind::Deny,
            PolicyAction::RequireApproval => PolicyDecisionKind::RequireApproval,
        },
        reason_code: ReasonCode::DefaultDeny,
        metadata: Some(serde_json::json!({
            "caller": format!("{:?}", request.caller),
            "tool_type": format!("{:?}", request.tool_type),
            "operation": format!("{:?}", request.operation),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_sensitivity::{SensitivityLevel, SensitivityResult};
    use crate::security::policy_engine::{PolicyCaller, PolicyDecisionKind, PolicyRequest, PolicyTarget, ToolType, Operation};

    fn safe_request() -> PolicyRequest {
        PolicyRequest {
            caller: PolicyCaller::AgentOrchestrator,
            tool_type: ToolType::Fs,
            operation: Operation::Write,
            target: PolicyTarget::FsPath {
                canonical_path: "/tmp/workspace/file.txt".to_string(),
            },
            ..Default::default()
        }
    }

    /// Malformed policy must not weaken security: fallback yields deny or require_approval,
    /// never allow where built-in would deny/require_approval.
    fn assert_not_more_permissive(cfg: &PolicyConfig) {
        let req = safe_request();
        let dec = evaluate_with_config(&req, cfg);
        // Built-in policy returns RequireApproval for agent write. Fallback must not be Allow.
        assert_ne!(
            dec.decision,
            PolicyDecisionKind::Allow,
            "malformed policy fallback must not allow agent write"
        );
    }

    #[test]
    fn invalid_policy_falls_back_to_secure_default() {
        let invalid_yaml = b"rules: not an array";
        let cfg = load_from_yaml(invalid_yaml);
        assert_eq!(cfg.default_action, PolicyAction::Deny);
        assert!(!cfg.rules.is_empty());
    }

    #[test]
    fn empty_rules_validation_fails() {
        let yaml = b"version: 1\ndefault_action: deny\nrules: []";
        let result = load_from_yaml_result(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn default_policy_loads() {
        let cfg = default_policy();
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.default_action, PolicyAction::Deny);
        assert!(!cfg.rules.is_empty());
    }

    /// Well-formed YAML loads and produces a valid PolicyConfig with expected structure.
    #[test]
    fn well_formed_yaml_loads_successfully() {
        let yaml = br#"
version: 1
default_action: deny
rules:
  - match:
      tool_type: [fs]
      operation: [read]
    action: allow
    reason_code: EXPLICIT_ALLOW
  - match:
      path_patterns: ["~/.ssh"]
    action: deny
    reason_code: FS_PATH_IN_BLOCKLIST
"#;
        let result = load_from_yaml_result(yaml);
        let cfg = result.expect("well-formed YAML must load");
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.default_action, PolicyAction::Deny);
        assert_eq!(cfg.rules.len(), 2);
        assert_eq!(cfg.rules[0].action, PolicyAction::Allow);
        assert_eq!(cfg.rules[1].action, PolicyAction::Deny);
    }

    /// Unknown action values (allow_all, typoed approve) cause parse failure -> secure fallback.
    #[test]
    fn unknown_action_falls_back_to_secure_default() {
        let yaml = br#"
version: 1
default_action: deny
rules:
  - match:
      tool_type: [fs]
    action: allow_all
"#;
        let cfg = load_from_yaml(yaml);
        assert_eq!(cfg.default_action, PolicyAction::Deny);
        assert_not_more_permissive(&cfg);
    }

    /// Typoed action (approve instead of require_approval) causes parse failure -> secure fallback.
    #[test]
    fn typoed_action_falls_back_to_secure_default() {
        let yaml = br#"
version: 1
default_action: deny
rules:
  - match:
      tool_type: [fs]
      operation: [write]
    action: approve
"#;
        let cfg = load_from_yaml(yaml);
        assert_eq!(cfg.default_action, PolicyAction::Deny);
        assert_not_more_permissive(&cfg);
    }

    /// Rule with empty match and action allow would match everything; validation rejects.
    #[test]
    fn empty_match_allow_rejected() {
        let yaml = br#"
version: 1
default_action: deny
rules:
  - match: {}
    action: allow
"#;
        let result = load_from_yaml_result(yaml);
        assert!(result.is_err(), "empty match + allow must be rejected");
    }

    /// Missing required fields (no match) -> validation fails or parse fails -> secure fallback.
    #[test]
    fn missing_match_falls_back() {
        let yaml = br#"
version: 1
default_action: deny
rules:
  - action: allow
"#;
        let cfg = load_from_yaml(yaml);
        assert_eq!(cfg.default_action, PolicyAction::Deny);
        assert_not_more_permissive(&cfg);
    }

    /// Invalid path/command patterns (wrong type) cause parse failure -> secure fallback.
    #[test]
    fn invalid_path_patterns_type_falls_back() {
        let yaml = br#"
version: 1
default_action: deny
rules:
  - match:
      path_patterns: 12345
    action: deny
"#;
        let cfg = load_from_yaml(yaml);
        assert_eq!(cfg.default_action, PolicyAction::Deny);
        assert_not_more_permissive(&cfg);
    }

    #[test]
    fn data_sensitivity_rule_deny_when_above_max_level() {
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
        };
        let dec = evaluate_with_config(&req, &cfg);
        assert_eq!(dec.decision, PolicyDecisionKind::Deny);
        assert!(matches!(dec.reason_code, crate::security::policy_engine::ReasonCode::DataSensitivityDeny));
    }

    #[test]
    fn data_sensitivity_rule_require_approval_when_meets_require_approval_if() {
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
        };
        let dec = evaluate_with_config(&req, &cfg);
        assert_eq!(dec.decision, PolicyDecisionKind::RequireApproval);
        assert!(matches!(dec.reason_code, crate::security::policy_engine::ReasonCode::DataSensitivityRequireApproval));
    }

    /// Regression: load_from_yaml never returns policy that allows agent write by default.
    #[test]
    fn regression_malformed_policy_never_weakens_guardrails() {
        let malformed_cases: &[&[u8]] = &[
            b"rules: null",
            b"rules: 42",
            b"{}",
            b"version: 1\ndefault_action: allow\nrules: []",
            br#"
version: 1
default_action: allow
rules:
  - match: {}
    action: allow
"#,
        ];
        for yaml in malformed_cases {
            let cfg = load_from_yaml(yaml);
            assert_not_more_permissive(&cfg);
        }
    }
}

fn reason_code_from_str(s: &str) -> crate::security::policy_engine::ReasonCode {
    use crate::security::policy_engine::ReasonCode;
    match s.to_uppercase().as_str() {
        "FS_PATH_IN_BLOCKLIST" => ReasonCode::FsPathInBlocklist,
        "SHELL_COMMAND_BLACKLISTED" => ReasonCode::ShellCommandBlacklisted,
        "DESTRUCTIVE_FS_REQUIRES_APPROVAL" => ReasonCode::DestructiveFsRequiresApproval,
        "HIGH_RISK_TOOL_REQUIRES_APPROVAL" => ReasonCode::HighRiskToolRequiresApproval,
        "AGENT_WRITE_REQUIRES_APPROVAL" => ReasonCode::AgentWriteRequiresApproval,
        "AGENT_EXEC_REQUIRES_APPROVAL" => ReasonCode::AgentExecRequiresApproval,
        "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL" => ReasonCode::AgentDestructiveRequiresApproval,
        "EXPLICIT_ALLOW" => ReasonCode::ExplicitAllow,
        "INTERNAL_SYSTEM" => ReasonCode::InternalSystem,
        "DATA_SENSITIVITY_DENY" => ReasonCode::DataSensitivityDeny,
        "DATA_SENSITIVITY_REQUIRE_APPROVAL" => ReasonCode::DataSensitivityRequireApproval,
        _ => ReasonCode::DefaultDeny,
    }
}
