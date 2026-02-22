//! Plugin system (Sprint 9): YAML-defined extensions for shell commands, FS indexers, and agent tools.
//!
//! All plugin execution paths go through:
//! - Command Router -> Security Policy Engine -> Approval (where applicable) -> tool
//! - No plugin bypasses data sensitivity filters (Sprint 7).

mod capability_registry;
mod loader;
mod schema;

pub use capability_registry::{CapabilityRegistry, ToolCapability};

use std::collections::{HashMap, HashSet};

/// Returns capability ids whose category_hint or tags appear in the task.
/// Builds a temporary index for efficient lookup when capabilities slice is large.
pub fn matching_ids_for_task(capabilities: &[ToolCapability], task: &str) -> Vec<String> {
    let task_lower = task.trim().to_lowercase();
    let mut by_category: HashMap<String, Vec<&str>> = HashMap::new();
    let mut by_tag: HashMap<String, Vec<&str>> = HashMap::new();
    for (_i, cap) in capabilities.iter().enumerate() {
        let id = cap.id.as_str();
        if let Some(ref hint) = cap.category_hint {
            by_category
                .entry(hint.to_lowercase())
                .or_default()
                .push(id);
        }
        if let Some(ref tags) = cap.tags {
            for tag in tags {
                by_tag
                    .entry(tag.to_lowercase())
                    .or_default()
                    .push(id);
            }
        }
    }
    let mut ids = HashSet::new();
    for (category, id_list) in &by_category {
        if task_lower.contains(category) {
            ids.extend(id_list.iter().copied().map(String::from));
        }
    }
    for (tag, id_list) in &by_tag {
        if task_lower.contains(tag) {
            ids.extend(id_list.iter().copied().map(String::from));
        }
    }
    let mut out: Vec<String> = ids.into_iter().collect();
    out.sort();
    out
}
pub use loader::{
    build_capability_registry, load_plugins_from_dir, load_plugins_from_dir_with_telemetry,
    plugin_rules_from_descriptors, shell_plugins_to_command_specs, LoadResult, PluginLoadError,
};
pub use schema::{PluginArgSpec, PluginDescriptor, PluginSchema, PluginSecurity, PluginType};
