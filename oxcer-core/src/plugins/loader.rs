//! Plugin loading and validation (Sprint 9).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::plugins::CapabilityRegistry;
use crate::shell::CommandParamType;
use crate::telemetry::{log_event, LogMetrics};

use super::schema::{PluginDescriptor, PluginDescriptorRaw, PluginType};

pub type LoadResult = Result<Vec<PluginDescriptor>, PluginLoadError>;

#[derive(Debug)]
pub enum PluginLoadError {
    Io(std::io::Error),
    Parse { path: PathBuf, message: String },
    Validation(String),
}

impl std::fmt::Display for PluginLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginLoadError::Io(e) => write!(f, "plugin load io: {}", e),
            PluginLoadError::Parse { path, message } => {
                write!(f, "plugin parse {:?}: {}", path, message)
            }
            PluginLoadError::Validation(s) => write!(f, "plugin validation: {}", s),
        }
    }
}

impl std::error::Error for PluginLoadError {}

const VALID_PLUGIN_TYPES: &[&str] = &["shell", "fs_indexer", "agent_tool"];
const VALID_TOOL_TYPES: &[&str] = &["shell", "fs", "agent", "network", "web", "other"];
const VALID_OPERATIONS: &[&str] = &[
    "read", "write", "delete", "rename", "move", "chmod", "exec", "execute",
];

/// Loads plugins with telemetry emission (plugin_start at start, plugin_end at finish).
pub fn load_plugins_from_dir_with_telemetry(
    plugins_dir: &Path,
    app_config_dir: &Path,
    session_id: &str,
) -> LoadResult {
    let _ = log_event(
        app_config_dir,
        session_id,
        None,
        "system",
        "plugin",
        "plugin_start",
        None,
        LogMetrics::default(),
        serde_json::json!({ "plugins_dir": plugins_dir.display().to_string() }),
    );

    let result = load_plugins_from_dir(plugins_dir);

    let (status, details) = match &result {
        Ok(descriptors) => (
            "ok",
            serde_json::json!({ "loaded_count": descriptors.len() }),
        ),
        Err(e) => ("error", serde_json::json!({ "error": e.to_string() })),
    };

    let _ = log_event(
        app_config_dir,
        session_id,
        None,
        "system",
        "plugin",
        "plugin_end",
        Some(status),
        LogMetrics::default(),
        details,
    );

    result
}

/// Scans `plugins_dir` recursively for `.yaml` files, parses and validates each.
/// Invalid plugins are skipped with logged errors; valid ones are returned.
pub fn load_plugins_from_dir(plugins_dir: &Path) -> LoadResult {
    let mut descriptors = Vec::new();
    let mut seen_ids = HashSet::new();

    let entries = match std::fs::read_dir(plugins_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(PluginLoadError::Io(e)),
    };

    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            if p.is_file() && p.extension().map_or(false, |e| e == "yaml" || e == "yml") {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    files.sort();

    for path in files {
        let raw = match parse_plugin_file(&path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[plugins] skip invalid {}: {}", path.display(), e);
                continue;
            }
        };

        let desc = match validate_and_convert(&raw, plugins_dir, &mut seen_ids) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[plugins] skip {} (id={}): {}", path.display(), raw.id, e);
                continue;
            }
        };

        descriptors.push(desc);
    }

    Ok(descriptors)
}

fn parse_plugin_file(path: &Path) -> Result<PluginDescriptorRaw, PluginLoadError> {
    let bytes = std::fs::read(path).map_err(PluginLoadError::Io)?;
    serde_yaml::from_slice(&bytes).map_err(|e| PluginLoadError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

fn validate_and_convert(
    raw: &PluginDescriptorRaw,
    base_dir: &Path,
    seen_ids: &mut HashSet<String>,
) -> Result<PluginDescriptor, PluginLoadError> {
    if raw.id.is_empty() {
        return Err(PluginLoadError::Validation("id must be non-empty".into()));
    }
    if !seen_ids.insert(raw.id.clone()) {
        return Err(PluginLoadError::Validation(format!(
            "duplicate plugin id: {}",
            raw.id
        )));
    }

    let plugin_type = match raw.plugin_type.to_lowercase().as_str() {
        "shell" => PluginType::Shell,
        "fs_indexer" => PluginType::FsIndexer,
        "agent_tool" => PluginType::AgentTool,
        other => {
            return Err(PluginLoadError::Validation(format!(
                "type must be one of {:?}, got: {}",
                VALID_PLUGIN_TYPES, other
            )))
        }
    };

    for tt in &raw.security.tool_types {
        let t = tt.to_lowercase();
        if !VALID_TOOL_TYPES.iter().any(|v| *v == t) {
            return Err(PluginLoadError::Validation(format!(
                "security.tool_type must be one of {:?}, got: {}",
                VALID_TOOL_TYPES, tt
            )));
        }
    }
    for op in &raw.security.operations {
        let o = op.to_lowercase();
        let normalized = if o == "execute" { "exec" } else { o.as_str() };
        if !VALID_OPERATIONS.iter().any(|v| *v == normalized) {
            return Err(PluginLoadError::Validation(format!(
                "security.operations must be from {:?}, got: {}",
                VALID_OPERATIONS, op
            )));
        }
    }

    let (binary_path, binary_exists) = if let Some(ref bp) = raw.binary_path {
        let path = if Path::new(bp).is_absolute() {
            PathBuf::from(bp)
        } else {
            base_dir.join(bp)
        };
        let exists = path.exists();
        (Some(path), exists)
    } else {
        (None, true) // no binary = N/A, treat as "ok"
    };

    // Shell plugins require binary_path and template
    if plugin_type == PluginType::Shell {
        if raw.binary_path.is_none()
            || raw
                .binary_path
                .as_ref()
                .map(|s| s.is_empty())
                .unwrap_or(true)
        {
            return Err(PluginLoadError::Validation(
                "shell plugin requires non-empty binary_path".into(),
            ));
        }
        if raw.template.is_empty() {
            return Err(PluginLoadError::Validation(
                "shell plugin requires non-empty template".into(),
            ));
        }
    }

    Ok(PluginDescriptor {
        id: raw.id.clone(),
        plugin_type,
        binary_path,
        template: raw.template.clone(),
        schema: raw.schema.clone(),
        security: raw.security.clone(),
        binary_exists,
    })
}

/// Generates policy rules from plugin security blocks.
/// Caller merges these into PolicyConfig before evaluation.
pub fn plugin_rules_from_descriptors(
    descriptors: &[PluginDescriptor],
) -> Vec<crate::security::policy_config::PolicyRule> {
    use crate::security::policy_config::{PolicyAction, PolicyMatch, PolicyRule};

    let mut rules = Vec::new();
    for d in descriptors {
        let tool_types: Vec<String> = if d.security.tool_types.is_empty() {
            vec!["shell".to_string()]
        } else {
            d.security.tool_types.clone()
        };
        // Shell invocations always use Operation::Exec; map plugin "read" (semantic) to "exec" for match.
        let ops: Vec<String> = if tool_types.iter().any(|t| t.eq_ignore_ascii_case("shell")) {
            vec!["exec".to_string()]
        } else if d.security.operations.is_empty() {
            vec!["exec".to_string()]
        } else {
            d.security
                .operations
                .iter()
                .map(|s| {
                    let o = s.to_lowercase();
                    if o == "execute" || o == "read" {
                        "exec".to_string()
                    } else {
                        o
                    }
                })
                .collect()
        };

        let requires_approval = d.security.require_approval.unwrap_or(d.security.dangerous);

        let action = if requires_approval {
            PolicyAction::RequireApproval
        } else {
            PolicyAction::Allow
        };

        let risk_level = d
            .security
            .risk_level
            .clone()
            .unwrap_or_else(|| if d.security.dangerous { "high" } else { "low" }.to_string());

        // Match by command_id (plugin id) for shell plugins
        let match_ = PolicyMatch {
            tool_type: Some(tool_types),
            operation: Some(ops),
            command_patterns: Some(vec![d.id.clone()]),
            caller: None,
            path_patterns: None,
        };

        rules.push(PolicyRule {
            match_,
            action,
            reason_code: Some(format!("PLUGIN_{}", d.id.replace('.', "_").to_uppercase())),
            notes: Some(d.schema.description.clone()),
            risk_level: Some(risk_level),
            data_sensitivity: None,
        });
    }
    rules
}

/// Converts shell and fs_indexer (with binary_path) descriptors to CommandSpec.
pub fn shell_plugins_to_command_specs(
    descriptors: &[PluginDescriptor],
) -> Vec<(String, crate::shell::CommandSpec)> {
    use crate::shell::{CommandParamSpec, CommandParamType, CommandSpec};
    use std::path::Path;

    let mut out = Vec::new();
    for d in descriptors {
        if d.plugin_type != PluginType::Shell && d.plugin_type != PluginType::FsIndexer {
            continue;
        }
        if d.binary_path.is_none() {
            continue; // fs_indexer without binary_path cannot run as shell command
        }
        let binary = d
            .binary_path
            .clone()
            .unwrap_or_else(|| Path::new("").to_path_buf());
        let args_template = if d.template.is_empty() && d.plugin_type == PluginType::FsIndexer {
            vec!["{{workspace}}".to_string()] // default: index workspace root
        } else {
            d.template.clone()
        };
        let params: Vec<CommandParamSpec> = d
            .schema
            .args
            .iter()
            .map(|a| CommandParamSpec {
                name: a.name.clone(),
                required: a.required,
                param_type: parse_param_type(&a.arg_type),
            })
            .collect();
        // Ensure workspace_id if template has {{workspace}}
        let has_workspace = args_template.iter().any(|t| t.contains("{{workspace}}"));
        let mut params = params;
        if has_workspace {
            let has_wid = params.iter().any(|p| p.name == "workspace_id");
            if !has_wid {
                params.insert(
                    0,
                    CommandParamSpec {
                        name: "workspace_id".to_string(),
                        required: true,
                        param_type: CommandParamType::String,
                    },
                );
            }
        }
        let spec = CommandSpec {
            id: d.id.clone(),
            binary,
            args_template,
            params,
            description: d.schema.description.clone(),
        };
        out.push((d.id.clone(), spec));
    }
    out
}

/// Builds a capability registry from plugin descriptors.
/// Includes agent_tool plugins and shell/fs_indexer plugins that have schema.category_hint
/// (so they can surface as tool_hints when the Semantic Router matches the task).
pub fn build_capability_registry(descriptors: &[PluginDescriptor]) -> CapabilityRegistry {
    use super::capability_registry::{CapabilityRegistry, ToolCapability};

    let mut reg = CapabilityRegistry::new();
    for d in descriptors {
        let has_hint = d.schema.category_hint.is_some();
        let is_agent_tool = d.plugin_type == PluginType::AgentTool;
        let is_shell_with_hint = (d.plugin_type == PluginType::Shell
            || d.plugin_type == PluginType::FsIndexer)
            && has_hint;

        if is_agent_tool || is_shell_with_hint {
            reg.register(ToolCapability {
                id: d.id.clone(),
                description: d.schema.description.clone(),
                category_hint: d.schema.category_hint.clone(),
                tags: d.schema.tags.clone(),
                dangerous: d.security.dangerous,
            });
        }
    }
    reg
}

fn parse_param_type(s: &str) -> CommandParamType {
    match s.to_lowercase().as_str() {
        "string" => CommandParamType::String,
        "bool" | "boolean" => CommandParamType::Bool,
        "path" | "path_relative_to_workspace" => CommandParamType::PathRelativeToWorkspace,
        "int" | "integer" => CommandParamType::Integer {
            min: None,
            max: None,
        },
        other if other.starts_with("enum") => CommandParamType::String, // simplified
        _ => CommandParamType::String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn invalid_yaml_skipped_with_error() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path();
        fs::write(
            plugins_dir.join("bad.yaml"),
            "id: test\ntype: shell\ninvalid: [",
        )
        .unwrap();
        let result = load_plugins_from_dir(plugins_dir);
        assert!(result.is_ok());
        let descriptors = result.unwrap();
        assert!(descriptors.is_empty(), "invalid YAML should be skipped");
    }

    #[test]
    fn invalid_type_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path();
        fs::write(
            plugins_dir.join("bad.yaml"),
            r#"
id: "shell.foo"
type: "invalid_type"
binary_path: "/usr/bin/true"
template: []
schema:
  description: "bad"
  args: []
security:
  tool_type: [shell]
  operations: [read]
  dangerous: false
"#,
        )
        .unwrap();
        let result = load_plugins_from_dir(plugins_dir);
        assert!(result.is_ok());
        let descriptors = result.unwrap();
        assert!(descriptors.is_empty());
    }

    #[test]
    fn shell_plugin_requires_binary_path_and_template() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path();
        fs::write(
            plugins_dir.join("no_binary.yaml"),
            r#"
id: "shell.missing"
type: "shell"
schema:
  description: "no binary"
  args: []
security:
  tool_type: [shell]
  operations: [read]
  dangerous: false
"#,
        )
        .unwrap();
        let result = load_plugins_from_dir(plugins_dir);
        assert!(result.is_ok());
        let descriptors = result.unwrap();
        assert!(descriptors.is_empty());
    }

    #[test]
    fn valid_shell_plugin_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path();
        fs::write(
            plugins_dir.join("git_status.yaml"),
            r#"
id: "shell.git_status"
type: "shell"
binary_path: "/usr/bin/git"
template: ["status", "--short"]
schema:
  description: "Show git status in the current workspace"
  args:
    - name: "path"
      type: "string"
      required: false
      default: "."
security:
  tool_type: [shell]
  operations: [read]
  dangerous: false
"#,
        )
        .unwrap();
        let result = load_plugins_from_dir(plugins_dir);
        assert!(result.is_ok(), "{:?}", result.err());
        let descriptors = result.unwrap();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].id, "shell.git_status");
        assert_eq!(descriptors[0].plugin_type, PluginType::Shell);
    }

    #[test]
    fn dangerous_plugin_generates_require_approval_rule() {
        use crate::security::policy_config::PolicyAction;

        let tmp = tempfile::tempdir().unwrap();
        let plugins_dir = tmp.path();
        fs::write(
            plugins_dir.join("deploy.yaml"),
            r#"
id: "agent.deploy_tool"
type: "agent_tool"
schema:
  description: "Deploy current project"
  category_hint: "deploy"
security:
  tool_type: [shell, network]
  operations: [exec]
  dangerous: true
"#,
        )
        .unwrap();
        let result = load_plugins_from_dir(plugins_dir);
        assert!(result.is_ok());
        let descriptors = result.unwrap();
        assert_eq!(descriptors.len(), 1);
        let rules = plugin_rules_from_descriptors(&descriptors);
        assert!(!rules.is_empty());
        assert_eq!(rules[0].action, PolicyAction::RequireApproval);
    }
}
