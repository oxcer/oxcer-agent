//! Sprint 9 plugin system tests: invalid YAML, dangerous=true approval, telemetry.

use oxcer_core::plugins::{
    load_plugins_from_dir, load_plugins_from_dir_with_telemetry, plugin_rules_from_descriptors,
};
use oxcer_core::security::policy_config::{
    evaluate_with_config, load_from_yaml, merge_rules, PolicyAction,
};
use oxcer_core::security::policy_engine::{
    evaluate, Operation, PolicyCaller, PolicyDecisionKind, PolicyRequest, PolicyTarget, ToolType,
};
use oxcer_core::telemetry::{load_session_log_from_dir, LogEvent};
use std::fs;

/// Invalid plugin YAML is skipped with logged error; no panic.
#[test]
fn invalid_plugin_yaml_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path();
    fs::write(plugins_dir.join("bad.yaml"), "id: x\ntype: [").unwrap();

    let result = load_plugins_from_dir(plugins_dir);
    assert!(result.is_ok());
    let descriptors = result.unwrap();
    assert!(descriptors.is_empty(), "invalid YAML should be skipped");
}

/// Plugin with dangerous=true generates RequireApproval policy rule.
#[test]
fn dangerous_plugin_requires_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path();
    fs::write(
        plugins_dir.join("deploy.yaml"),
        r#"
id: "shell.deploy"
type: "shell"
binary_path: "/usr/bin/echo"
template: ["deploy"]
schema:
  description: "Deploy tool"
  args: []
security:
  tool_type: [shell]
  operations: [exec]
  dangerous: true
"#,
    )
    .unwrap();

    let descriptors = load_plugins_from_dir(plugins_dir).unwrap();
    assert_eq!(descriptors.len(), 1);
    assert!(descriptors[0].security.dangerous);

    let plugin_rules = plugin_rules_from_descriptors(&descriptors);
    assert!(!plugin_rules.is_empty());
    assert_eq!(plugin_rules[0].action, PolicyAction::RequireApproval);

    let base_yaml = include_str!("../policies/default.yaml");
    let base = load_from_yaml(base_yaml.as_bytes());
    let merged = merge_rules(base, plugin_rules);

    let req = PolicyRequest {
        caller: PolicyCaller::AgentOrchestrator,
        tool_type: ToolType::Shell,
        operation: Operation::Exec,
        target: PolicyTarget::ShellCommand {
            command_id: "shell.deploy".to_string(),
            normalized_command: None,
        },
        ..Default::default()
    };
    let dec = evaluate_with_config(&req, &merged);
    assert_eq!(
        dec.decision,
        PolicyDecisionKind::RequireApproval,
        "dangerous plugin should require approval"
    );
}

/// git_status plugin: loaded, present in catalog, policy allows, surfaces in tool_hints.
#[test]
fn git_status_plugin_e2e() {
    use oxcer_core::plugins::{build_capability_registry, shell_plugins_to_command_specs};
    use oxcer_core::shell;

    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path();
    let git_status_yaml = r#"
id: "shell.git_status"
type: "shell"
binary_path: "/usr/bin/git"
template: ["-C", "{{workspace}}", "status", "--short"]
schema:
  description: "Show git status in the current workspace"
  category_hint: "git"
  args:
    - name: "workspace_id"
      type: "string"
      required: true
security:
  tool_type: [shell]
  operations: [read]
  dangerous: false
"#;
    fs::write(plugins_dir.join("git_status.yaml"), git_status_yaml).unwrap();

    let descriptors = load_plugins_from_dir(plugins_dir).unwrap();
    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].id, "shell.git_status");
    assert!(!descriptors[0].security.dangerous);

    let mut catalog = shell::default_catalog();
    let specs = shell_plugins_to_command_specs(&descriptors);
    catalog.merge_plugin_commands(specs);
    assert!(
        catalog.get("shell.git_status").is_some(),
        "git_status must be in catalog"
    );

    let plugin_rules = plugin_rules_from_descriptors(&descriptors);
    let base = load_from_yaml(include_str!("../policies/default.yaml").as_bytes());
    let merged = merge_rules(base, plugin_rules);
    let req = PolicyRequest {
        caller: PolicyCaller::AgentOrchestrator,
        tool_type: ToolType::Shell,
        operation: Operation::Exec,
        target: PolicyTarget::ShellCommand {
            command_id: "shell.git_status".to_string(),
            normalized_command: None,
        },
        ..Default::default()
    };
    let dec = evaluate_with_config(&req, &merged);
    assert_eq!(
        dec.decision,
        PolicyDecisionKind::Allow,
        "git_status (read, not dangerous) should be allowed"
    );

    let registry = build_capability_registry(&descriptors);
    assert!(!registry.list().is_empty());
    let hints = registry.matching_ids_for_task("show me the git status");
    assert!(
        hints.contains(&"shell.git_status".to_string()),
        "task mentioning git should surface shell.git_status in tool_hints"
    );
}

/// load_plugins_from_dir_with_telemetry emits plugin_start and plugin_end.
#[test]
fn plugin_telemetry_emits_start_and_end() {
    let tmp = tempfile::tempdir().unwrap();
    let plugins_dir = tmp.path();
    let app_config_dir = tmp.path().join("config");
    fs::create_dir_all(&app_config_dir).unwrap();
    fs::create_dir_all(app_config_dir.join("logs")).unwrap();

    fs::write(
        plugins_dir.join("ok.yaml"),
        r#"
id: "shell.echo"
type: "shell"
binary_path: "/usr/bin/echo"
template: ["hello"]
schema:
  description: "Echo"
  args: []
security:
  tool_type: [shell]
  operations: [read]
  dangerous: false
"#,
    )
    .unwrap();

    let _ = load_plugins_from_dir_with_telemetry(plugins_dir, &app_config_dir, "sprint9-telemetry");

    let events = load_session_log_from_dir(&app_config_dir, "sprint9-telemetry").unwrap();
    let plugin_events: Vec<&LogEvent> = events
        .iter()
        .filter(|e| {
            e.component == "plugin" && (e.action == "plugin_start" || e.action == "plugin_end")
        })
        .collect();

    assert!(
        plugin_events.iter().any(|e| e.action == "plugin_start"),
        "expected plugin_start event"
    );
    assert!(
        plugin_events.iter().any(|e| e.action == "plugin_end"),
        "expected plugin_end event"
    );
}
