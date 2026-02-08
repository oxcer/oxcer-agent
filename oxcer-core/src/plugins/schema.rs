//! Plugin YAML schema types (Sprint 9).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Plugin type: shell, fs_indexer, or agent_tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginType {
    Shell,
    FsIndexer,
    AgentTool,
}

/// Schema for a plugin parameter.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginArgSpec {
    pub name: String,
    #[serde(default)]
    pub required: bool,
    #[serde(rename = "type", default)]
    pub arg_type: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

/// Schema block (description, args).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginSchema {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub args: Vec<PluginArgSpec>,
    /// Hint for Semantic Router (e.g. "deploy", "git", "test").
    #[serde(default, rename = "category_hint")]
    pub category_hint: Option<String>,
    /// Optional tags for indexed lookup (e.g. ["search", "code"]).
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// Security block: maps to policy tool_type, operations, and risk.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginSecurity {
    /// Tool type(s): shell, fs, agent, network.
    #[serde(default, rename = "tool_type")]
    pub tool_types: Vec<String>,
    /// Operations: read, write, exec, etc.
    #[serde(default)]
    pub operations: Vec<String>,
    /// If true, plugin requires approval by default.
    #[serde(default)]
    pub dangerous: bool,
    /// risk_level: low, medium, high (for policy metadata).
    #[serde(default, rename = "risk_level")]
    pub risk_level: Option<String>,
    /// Explicit require_approval override (overrides dangerous default behavior when set).
    #[serde(default, rename = "require_approval")]
    pub require_approval: Option<bool>,
}

/// Raw YAML plugin descriptor (before validation).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PluginDescriptorRaw {
    pub id: String,
    #[serde(rename = "type")]
    pub plugin_type: String,
    #[serde(default, rename = "binary_path")]
    pub binary_path: Option<String>,
    #[serde(default)]
    pub template: Vec<String>,
    #[serde(default)]
    pub schema: PluginSchema,
    #[serde(default)]
    pub security: PluginSecurity,
}

/// Validated plugin descriptor.
#[derive(Clone, Debug)]
pub struct PluginDescriptor {
    pub id: String,
    pub plugin_type: PluginType,
    pub binary_path: Option<PathBuf>,
    pub template: Vec<String>,
    pub schema: PluginSchema,
    pub security: PluginSecurity,
    /// Best-effort: binary exists at load time.
    pub binary_exists: bool,
}

