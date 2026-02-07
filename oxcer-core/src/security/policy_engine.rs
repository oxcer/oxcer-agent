//! Security Policy Engine: three-valued decisions (allow, deny, require_approval).
//!
//! Acts as the single source of truth for all privileged actions. Both the UI
//! and Agent Orchestrator must pass through this layer.

use serde::{Deserialize, Serialize};

// -----------------------------------------------------------------------------
// Policy Request
// -----------------------------------------------------------------------------

/// Logical caller of the requested operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PolicyCaller {
    /// Human-driven UI (trusted; still subject to path/command blocklists).
    Ui,
    /// AI Agent Orchestrator (untrusted; stricter policy).
    AgentOrchestrator,
    /// Internal system (e.g. app setup; limited use).
    InternalSystem,
}

/// Type of tool being invoked.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolType {
    Fs,
    Shell,
    Agent,
    Web,
    Other,
}

/// Operation being performed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Write,
    Delete,
    Rename,
    Move,
    Chmod,
    Exec,
}

/// Target of the operation; type depends on ToolType.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicyTarget {
    /// For FS: canonical path being touched.
    FsPath { canonical_path: String },
    /// For Shell: command id and optionally normalized command name.
    ShellCommand {
        command_id: String,
        normalized_command: Option<String>,
    },
    /// For other tools: resource id / API name.
    Resource {
        resource_id: String,
        api_name: Option<String>,
    },
}

/// Request passed to the Policy Engine.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyRequest {
    pub caller: PolicyCaller,
    pub tool_type: ToolType,
    pub operation: Operation,
    pub target: PolicyTarget,
}

// -----------------------------------------------------------------------------
// Policy Decision
// -----------------------------------------------------------------------------

/// Three-valued decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PolicyDecisionKind {
    Allow,
    Deny,
    RequireApproval,
}

/// Stable reason codes for logging and telemetry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReasonCode {
    FsPathInBlocklist,
    ShellCommandBlacklisted,
    DestructiveFsRequiresApproval,
    HighRiskToolRequiresApproval,
    AgentWriteRequiresApproval,
    AgentExecRequiresApproval,
    AgentDestructiveRequiresApproval,
    ExplicitAllow,
    InternalSystem,
    /// Least-privilege fallback: no explicit allow rule matched.
    DefaultDeny,
}

impl ReasonCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasonCode::FsPathInBlocklist => "FS_PATH_IN_BLOCKLIST",
            ReasonCode::ShellCommandBlacklisted => "SHELL_COMMAND_BLACKLISTED",
            ReasonCode::DestructiveFsRequiresApproval => "DESTRUCTIVE_FS_REQUIRES_APPROVAL",
            ReasonCode::HighRiskToolRequiresApproval => "HIGH_RISK_TOOL_REQUIRES_APPROVAL",
            ReasonCode::AgentWriteRequiresApproval => "AGENT_WRITE_REQUIRES_APPROVAL",
            ReasonCode::AgentExecRequiresApproval => "AGENT_EXEC_REQUIRES_APPROVAL",
            ReasonCode::AgentDestructiveRequiresApproval => "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL",
            ReasonCode::ExplicitAllow => "EXPLICIT_ALLOW",
            ReasonCode::InternalSystem => "INTERNAL_SYSTEM",
            ReasonCode::DefaultDeny => "DEFAULT_DENY",
        }
    }
}

impl std::fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Output of the Policy Engine.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub decision: PolicyDecisionKind,
    pub reason_code: ReasonCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// When no explicit allow rule matches, fall back to deny.
/// This is the cornerstone of Zero Trust: never allow by default.
pub const DEFAULT_DENY: bool = true;

/// Checks if canonical path touches a blocklisted location.
/// Uses evaluate() with a synthetic request (config-driven).
#[doc(hidden)]
pub fn is_path_blocklisted(canonical_path: &str) -> bool {
    let req = PolicyRequest {
        caller: PolicyCaller::Ui,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: canonical_path.to_string(),
        },
    };
    let dec = evaluate(req);
    matches!(dec.decision, PolicyDecisionKind::Deny)
        && matches!(dec.reason_code, ReasonCode::FsPathInBlocklist)
}

// -----------------------------------------------------------------------------
// Policy evaluation — config-driven (YAML/JSON)
// -----------------------------------------------------------------------------

/// Cached default policy (loaded once; invalid → secure default).
use std::sync::OnceLock;
static DEFAULT_POLICY: OnceLock<crate::security::policy_config::PolicyConfig> = OnceLock::new();

fn loaded_policy() -> &'static crate::security::policy_config::PolicyConfig {
    DEFAULT_POLICY.get_or_init(|| {
        let yaml = include_str!("../../policies/default.yaml");
        crate::security::policy_config::load_from_yaml(yaml.as_bytes())
    })
}

/// Main entry point: evaluates a policy request and returns a decision.
///
/// Uses config-driven policy (see `policy_config` and `policies/default.yaml`).
/// On load/parse failure, falls back to secure default (default-deny).
pub fn evaluate(request: PolicyRequest) -> PolicyDecision {
    let config = loaded_policy();
    crate::security::policy_config::evaluate_with_config(&request, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table-driven policy tests: (caller, tool_type, operation, target) -> (decision, reason_code).
    /// Covers allow, deny, require_approval, and Agent vs UI asymmetry.
    #[derive(Debug)]
    struct PolicyTestCase {
        caller: PolicyCaller,
        tool_type: ToolType,
        operation: Operation,
        target: PolicyTarget,
        expected_decision: PolicyDecisionKind,
        expected_reason: fn(&ReasonCode) -> bool,
    }

    fn run_table_test(cases: &[PolicyTestCase]) {
        for (i, tc) in cases.iter().enumerate() {
            let req = PolicyRequest {
                caller: tc.caller,
                tool_type: tc.tool_type,
                operation: tc.operation,
                target: tc.target.clone(),
            };
            let dec = evaluate(req);
            assert_eq!(
                dec.decision,
                tc.expected_decision,
                "case {}: expected {:?}, got {:?}",
                i,
                tc.expected_decision,
                dec.decision
            );
            assert!(
                (tc.expected_reason)(&dec.reason_code),
                "case {}: unexpected reason {:?}",
                i,
                dec.reason_code
            );
        }
    }

    #[test]
    fn table_driven_policy_decisions() {
        let home = dirs_next::home_dir().unwrap();
        let blocklisted_path = home.join(".ssh/id_rsa").display().to_string();
        let safe_path = "/tmp/workspace/file.txt".to_string();

        run_table_test(&[
            // ─── DENY (path blocklist, command blacklist) ─────────────────────
            PolicyTestCase {
                caller: PolicyCaller::Ui,
                tool_type: ToolType::Fs,
                operation: Operation::Read,
                target: PolicyTarget::FsPath {
                    canonical_path: blocklisted_path.clone(),
                },
                expected_decision: PolicyDecisionKind::Deny,
                expected_reason: |r| matches!(r, ReasonCode::FsPathInBlocklist),
            },
            PolicyTestCase {
                caller: PolicyCaller::AgentOrchestrator,
                tool_type: ToolType::Fs,
                operation: Operation::Write,
                target: PolicyTarget::FsPath {
                    canonical_path: blocklisted_path.clone(),
                },
                expected_decision: PolicyDecisionKind::Deny,
                expected_reason: |r| matches!(r, ReasonCode::FsPathInBlocklist),
            },
            PolicyTestCase {
                caller: PolicyCaller::Ui,
                tool_type: ToolType::Shell,
                operation: Operation::Exec,
                target: PolicyTarget::ShellCommand {
                    command_id: "rm".to_string(),
                    normalized_command: Some("rm -rf /".to_string()),
                },
                expected_decision: PolicyDecisionKind::Deny,
                expected_reason: |r| matches!(r, ReasonCode::ShellCommandBlacklisted),
            },
            PolicyTestCase {
                caller: PolicyCaller::Ui,
                tool_type: ToolType::Shell,
                operation: Operation::Exec,
                target: PolicyTarget::ShellCommand {
                    command_id: "sudo".to_string(),
                    normalized_command: None,
                },
                expected_decision: PolicyDecisionKind::Deny,
                expected_reason: |r| matches!(r, ReasonCode::ShellCommandBlacklisted),
            },
            // ─── REQUIRE_APPROVAL (risk-based, agent) ──────────────────────────
            PolicyTestCase {
                caller: PolicyCaller::Ui,
                tool_type: ToolType::Fs,
                operation: Operation::Delete,
                target: PolicyTarget::FsPath {
                    canonical_path: safe_path.clone(),
                },
                expected_decision: PolicyDecisionKind::RequireApproval,
                expected_reason: |r| matches!(r, ReasonCode::DestructiveFsRequiresApproval),
            },
            PolicyTestCase {
                caller: PolicyCaller::Ui,
                tool_type: ToolType::Shell,
                operation: Operation::Exec,
                target: PolicyTarget::ShellCommand {
                    command_id: "deploy".to_string(),
                    normalized_command: None,
                },
                expected_decision: PolicyDecisionKind::RequireApproval,
                expected_reason: |r| matches!(r, ReasonCode::HighRiskToolRequiresApproval),
            },
            PolicyTestCase {
                caller: PolicyCaller::AgentOrchestrator,
                tool_type: ToolType::Fs,
                operation: Operation::Write,
                target: PolicyTarget::FsPath {
                    canonical_path: safe_path.clone(),
                },
                expected_decision: PolicyDecisionKind::RequireApproval,
                expected_reason: |r| matches!(r, ReasonCode::AgentWriteRequiresApproval),
            },
            PolicyTestCase {
                caller: PolicyCaller::AgentOrchestrator,
                tool_type: ToolType::Shell,
                operation: Operation::Exec,
                target: PolicyTarget::ShellCommand {
                    command_id: "run_tests".to_string(),
                    normalized_command: None,
                },
                expected_decision: PolicyDecisionKind::RequireApproval,
                expected_reason: |r| matches!(r, ReasonCode::AgentExecRequiresApproval),
            },
            // ─── ALLOW (UI low-risk, Agent read) ───────────────────────────────
            PolicyTestCase {
                caller: PolicyCaller::Ui,
                tool_type: ToolType::Fs,
                operation: Operation::Read,
                target: PolicyTarget::FsPath {
                    canonical_path: safe_path.clone(),
                },
                expected_decision: PolicyDecisionKind::Allow,
                expected_reason: |r| matches!(r, ReasonCode::ExplicitAllow),
            },
            PolicyTestCase {
                caller: PolicyCaller::Ui,
                tool_type: ToolType::Fs,
                operation: Operation::Write,
                target: PolicyTarget::FsPath {
                    canonical_path: safe_path.clone(),
                },
                expected_decision: PolicyDecisionKind::Allow,
                expected_reason: |r| matches!(r, ReasonCode::ExplicitAllow),
            },
            PolicyTestCase {
                caller: PolicyCaller::Ui,
                tool_type: ToolType::Shell,
                operation: Operation::Exec,
                target: PolicyTarget::ShellCommand {
                    command_id: "run_tests".to_string(),
                    normalized_command: None,
                },
                expected_decision: PolicyDecisionKind::Allow,
                expected_reason: |r| matches!(r, ReasonCode::ExplicitAllow),
            },
            PolicyTestCase {
                caller: PolicyCaller::AgentOrchestrator,
                tool_type: ToolType::Fs,
                operation: Operation::Read,
                target: PolicyTarget::FsPath {
                    canonical_path: safe_path.clone(),
                },
                expected_decision: PolicyDecisionKind::Allow,
                expected_reason: |r| matches!(r, ReasonCode::ExplicitAllow),
            },
            // ─── Agent vs UI asymmetry: same op, different callers ──────────────
            PolicyTestCase {
                caller: PolicyCaller::InternalSystem,
                tool_type: ToolType::Fs,
                operation: Operation::Read,
                target: PolicyTarget::FsPath {
                    canonical_path: safe_path.clone(),
                },
                expected_decision: PolicyDecisionKind::Allow,
                expected_reason: |r| matches!(r, ReasonCode::InternalSystem),
            },
            // ─── DEFAULT_DENY ──────────────────────────────────────────────────
            PolicyTestCase {
                caller: PolicyCaller::AgentOrchestrator,
                tool_type: ToolType::Other,
                operation: Operation::Exec,
                target: PolicyTarget::Resource {
                    resource_id: "unknown".to_string(),
                    api_name: None,
                },
                expected_decision: PolicyDecisionKind::Deny,
                expected_reason: |r| matches!(r, ReasonCode::DefaultDeny),
            },
        ]);
    }

    // ─── Static deny rules ───────────────────────────────────────────────────

    #[test]
    fn path_blocklist_ssh_denied() {
        let home = dirs_next::home_dir().unwrap();
        assert!(is_path_blocklisted(&home.join(".ssh/id_rsa").display().to_string()));
    }

    #[test]
    fn path_blocklist_aws_denied() {
        let home = dirs_next::home_dir().unwrap();
        assert!(is_path_blocklisted(&home.join(".aws/credentials").display().to_string()));
    }

    #[test]
    fn path_blocklist_gnupg_denied() {
        let home = dirs_next::home_dir().unwrap();
        assert!(is_path_blocklisted(&home.join(".gnupg/pubring.kbx").display().to_string()));
    }

    #[test]
    fn path_blocklist_env_denied() {
        let home = dirs_next::home_dir().unwrap();
        assert!(is_path_blocklisted(&home.join(".env.local").display().to_string()));
    }

    #[test]
    fn workspace_path_allowed() {
        assert!(!is_path_blocklisted("/tmp/workspace/src/main.rs"));
    }

    #[test]
    fn shell_command_rm_denied() {
        let req = PolicyRequest {
            caller: PolicyCaller::Ui,
            tool_type: ToolType::Shell,
            operation: Operation::Exec,
            target: PolicyTarget::ShellCommand {
                command_id: "rm".to_string(),
                normalized_command: Some("rm -rf /".to_string()),
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::Deny);
        assert!(matches!(dec.reason_code, ReasonCode::ShellCommandBlacklisted));
    }

    #[test]
    fn shell_command_sudo_denied() {
        let req = PolicyRequest {
            caller: PolicyCaller::Ui,
            tool_type: ToolType::Shell,
            operation: Operation::Exec,
            target: PolicyTarget::ShellCommand {
                command_id: "sudo".to_string(),
                normalized_command: None,
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::Deny);
        assert!(matches!(dec.reason_code, ReasonCode::ShellCommandBlacklisted));
    }

    // ─── Risk-based rules ────────────────────────────────────────────────────

    #[test]
    fn destructive_fs_delete_requires_approval() {
        let req = PolicyRequest {
            caller: PolicyCaller::Ui,
            tool_type: ToolType::Fs,
            operation: Operation::Delete,
            target: PolicyTarget::FsPath {
                canonical_path: "/tmp/workspace/file.txt".to_string(),
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::RequireApproval);
        assert!(matches!(
            dec.reason_code,
            ReasonCode::DestructiveFsRequiresApproval
        ));
    }

    #[test]
    fn high_risk_shell_deploy_requires_approval() {
        let req = PolicyRequest {
            caller: PolicyCaller::Ui,
            tool_type: ToolType::Shell,
            operation: Operation::Exec,
            target: PolicyTarget::ShellCommand {
                command_id: "deploy".to_string(),
                normalized_command: None,
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::RequireApproval);
        assert!(matches!(
            dec.reason_code,
            ReasonCode::HighRiskToolRequiresApproval
        ));
    }

    // ─── Caller-sensitive policies (Agent = untrusted client) ─────────────────

    /// Agent trying a destructive FS operation must get REQUIRE_APPROVAL.
    /// Verifies the "Agent = untrusted client" invariant: agents never bypass
    /// the policy engine; destructive ops require HITL approval.
    #[test]
    fn agent_destructive_operation_requires_approval() {
        let req = PolicyRequest {
            caller: PolicyCaller::AgentOrchestrator,
            tool_type: ToolType::Fs,
            operation: Operation::Delete,
            target: PolicyTarget::FsPath {
                canonical_path: "/tmp/workspace/to_delete.txt".to_string(),
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::RequireApproval);
        assert!(
            matches!(dec.reason_code, ReasonCode::DestructiveFsRequiresApproval)
                || matches!(dec.reason_code, ReasonCode::AgentDestructiveRequiresApproval),
            "agent destructive op must require approval, got {:?}",
            dec.reason_code
        );
    }

    /// Agent trying to execute shell command must get REQUIRE_APPROVAL.
    #[test]
    fn agent_exec_requires_approval() {
        let req = PolicyRequest {
            caller: PolicyCaller::AgentOrchestrator,
            tool_type: ToolType::Shell,
            operation: Operation::Exec,
            target: PolicyTarget::ShellCommand {
                command_id: "run_tests".to_string(),
                normalized_command: None,
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::RequireApproval);
        assert!(matches!(dec.reason_code, ReasonCode::AgentExecRequiresApproval));
    }

    #[test]
    fn agent_write_requires_approval() {
        let req = PolicyRequest {
            caller: PolicyCaller::AgentOrchestrator,
            tool_type: ToolType::Fs,
            operation: Operation::Write,
            target: PolicyTarget::FsPath {
                canonical_path: "/tmp/workspace/file.txt".to_string(),
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::RequireApproval);
        assert!(matches!(dec.reason_code, ReasonCode::AgentWriteRequiresApproval));
    }

    #[test]
    fn agent_read_allowed() {
        let req = PolicyRequest {
            caller: PolicyCaller::AgentOrchestrator,
            tool_type: ToolType::Fs,
            operation: Operation::Read,
            target: PolicyTarget::FsPath {
                canonical_path: "/tmp/workspace/file.txt".to_string(),
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::Allow);
    }

    #[test]
    fn ui_write_allowed() {
        let req = PolicyRequest {
            caller: PolicyCaller::Ui,
            tool_type: ToolType::Fs,
            operation: Operation::Write,
            target: PolicyTarget::FsPath {
                canonical_path: "/tmp/workspace/file.txt".to_string(),
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::Allow);
    }

    #[test]
    fn blocklisted_path_denied_regardless_of_caller() {
        let home = dirs_next::home_dir().unwrap();
        let path = home.join(".ssh/id_rsa").display().to_string();
        for caller in [PolicyCaller::Ui, PolicyCaller::AgentOrchestrator] {
            let req = PolicyRequest {
                caller,
                tool_type: ToolType::Fs,
                operation: Operation::Read,
                target: PolicyTarget::FsPath {
                    canonical_path: path.clone(),
                },
            };
            let dec = evaluate(req);
            assert_eq!(dec.decision, PolicyDecisionKind::Deny);
            assert!(matches!(dec.reason_code, ReasonCode::FsPathInBlocklist));
        }
    }

    // ─── Default-deny ───────────────────────────────────────────────────────

    #[test]
    fn default_deny_unknown_operation() {
        let req = PolicyRequest {
            caller: PolicyCaller::AgentOrchestrator,
            tool_type: ToolType::Other,
            operation: Operation::Exec,
            target: PolicyTarget::Resource {
                resource_id: "unknown".to_string(),
                api_name: None,
            },
        };
        let dec = evaluate(req);
        assert_eq!(dec.decision, PolicyDecisionKind::Deny);
        assert!(matches!(dec.reason_code, ReasonCode::DefaultDeny));
    }

    #[test]
    fn default_deny_is_true() {
        assert!(DEFAULT_DENY);
    }
}

// -----------------------------------------------------------------------------
// Property-based tests (proptest)
// -----------------------------------------------------------------------------

#[cfg(test)]
mod proptest_tests {
    use super::*;
    use crate::security::policy_config::{default_policy, evaluate_with_config};
    use proptest::prelude::*;

    /// Restrictiveness: Deny > RequireApproval > Allow.
    /// Agent must never be less restrictive than UI for same (tool_type, op, target).
    fn restrictiveness(d: PolicyDecisionKind) -> u8 {
        match d {
            PolicyDecisionKind::Deny => 2,
            PolicyDecisionKind::RequireApproval => 1,
            PolicyDecisionKind::Allow => 0,
        }
    }

    fn caller_strat() -> impl Strategy<Value = PolicyCaller> {
        prop_oneof![
            Just(PolicyCaller::Ui),
            Just(PolicyCaller::AgentOrchestrator),
            Just(PolicyCaller::InternalSystem),
        ]
    }

    fn tool_type_strat() -> impl Strategy<Value = ToolType> {
        prop_oneof![
            Just(ToolType::Fs),
            Just(ToolType::Shell),
            Just(ToolType::Agent),
            Just(ToolType::Web),
            Just(ToolType::Other),
        ]
    }

    fn operation_strat() -> impl Strategy<Value = Operation> {
        prop_oneof![
            Just(Operation::Read),
            Just(Operation::Write),
            Just(Operation::Delete),
            Just(Operation::Rename),
            Just(Operation::Move),
            Just(Operation::Chmod),
            Just(Operation::Exec),
        ]
    }

    /// Path strings: safe paths, blocklisted-like (home/.ssh, home/.aws), noise.
    fn path_strat() -> impl Strategy<Value = String> {
        prop_oneof![
            "[a-zA-Z0-9_/-]{1,30}\\.(rs|ts|txt|json|yaml)".prop_map(|s| format!("/tmp/workspace/{}", s)),
            "[a-zA-Z0-9_/-]{1,20}".prop_map(|s| format!("/var/tmp/{}", s)),
            "[a-zA-Z0-9_/-]{1,20}".prop_map(|s| format!("/safe/{}", s)),
            "[a-zA-Z0-9_\\./-]{1,60}".prop_map(|s| {
                dirs_next::home_dir()
                    .map(|h| h.join(".ssh").join(&s).display().to_string())
                    .unwrap_or_else(|| format!("/tmp/.ssh/{}", s))
            }),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        /// Never panics; always returns well-formed PolicyDecision.
        #[test]
        fn prop_evaluate_never_panics_and_returns_valid_decision(
            caller in caller_strat(),
            tool_type in tool_type_strat(),
            operation in operation_strat(),
            path in path_strat(),
        ) {
            let target = match tool_type {
                ToolType::Fs => PolicyTarget::FsPath { canonical_path: path },
                ToolType::Shell => PolicyTarget::ShellCommand {
                    command_id: "run_tests".to_string(),
                    normalized_command: Some("cargo test".to_string()),
                },
                _ => PolicyTarget::Resource { resource_id: "r1".to_string(), api_name: None },
            };
            let req = PolicyRequest { caller, tool_type, operation, target };
            let config = default_policy();
            let dec = evaluate_with_config(&req, &config);
            assert!(matches!(
                dec.decision,
                PolicyDecisionKind::Allow | PolicyDecisionKind::Deny | PolicyDecisionKind::RequireApproval
            ));
        }

        /// Blocklisted path always DENY regardless of caller.
        #[test]
        fn prop_blocklisted_path_always_deny(caller in caller_strat()) {
            let home = dirs_next::home_dir().unwrap();
            let blocklisted = home.join(".ssh/id_rsa").display().to_string();
            let req = PolicyRequest {
                caller,
                tool_type: ToolType::Fs,
                operation: Operation::Read,
                target: PolicyTarget::FsPath { canonical_path: blocklisted },
            };
            let config = default_policy();
            let dec = evaluate_with_config(&req, &config);
            prop_assert_eq!(dec.decision, PolicyDecisionKind::Deny);
        }

        /// Blocklisted command (rm, sudo) always DENY regardless of caller.
        #[test]
        fn prop_blocklisted_command_always_deny(caller in caller_strat()) {
            let req = PolicyRequest {
                caller,
                tool_type: ToolType::Shell,
                operation: Operation::Exec,
                target: PolicyTarget::ShellCommand {
                    command_id: "rm".to_string(),
                    normalized_command: Some("rm -rf /".to_string()),
                },
            };
            let config = default_policy();
            let dec = evaluate_with_config(&req, &config);
            prop_assert_eq!(dec.decision, PolicyDecisionKind::Deny);
        }

        /// Agent is never less restrictive than UI for same (tool_type, operation, target).
        #[test]
        fn prop_agent_never_less_restrictive_than_ui(
            tool_type in tool_type_strat(),
            operation in operation_strat(),
            path in path_strat(),
        ) {
            let target = match tool_type {
                ToolType::Fs => PolicyTarget::FsPath { canonical_path: path },
                ToolType::Shell => PolicyTarget::ShellCommand {
                    command_id: "run_tests".to_string(),
                    normalized_command: None,
                },
                _ => PolicyTarget::Resource { resource_id: "r1".to_string(), api_name: None },
            };
            let config = default_policy();
            let req_ui = PolicyRequest {
                caller: PolicyCaller::Ui,
                tool_type,
                operation,
                target: target.clone(),
            };
            let req_agent = PolicyRequest {
                caller: PolicyCaller::AgentOrchestrator,
                tool_type,
                operation,
                target,
            };
            let dec_ui = evaluate_with_config(&req_ui, &config);
            let dec_agent = evaluate_with_config(&req_agent, &config);
            prop_assert!(
                restrictiveness(dec_agent.decision) >= restrictiveness(dec_ui.decision),
                "agent must not be less restrictive than UI: ui={:?} agent={:?}",
                dec_ui.decision,
                dec_agent.decision
            );
        }
    }
}
