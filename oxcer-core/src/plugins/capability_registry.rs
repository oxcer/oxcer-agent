//! Capability registry for agent tools (Sprint 9 Pass 2).
//!
//! Agent tools register here so the Semantic Router and Orchestrator can
//! see available capabilities (e.g. deploy_tool, test_runner).
//! Indexed by category_hint and tags for O(1) category/tag lookups at scale.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// One agent tool capability (from plugin descriptor).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCapability {
    pub id: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    pub dangerous: bool,
}

/// Registry of agent tool capabilities with category and tag indexes.
#[derive(Clone, Debug, Default)]
pub struct CapabilityRegistry {
    tools: Vec<ToolCapability>,
    /// category_hint (lowercase) -> indices into tools
    by_category: HashMap<String, Vec<usize>>,
    /// tag (lowercase) -> indices into tools
    by_tag: HashMap<String, Vec<usize>>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            by_category: HashMap::new(),
            by_tag: HashMap::new(),
        }
    }

    pub fn register(&mut self, cap: ToolCapability) {
        if self.tools.iter().any(|t| t.id == cap.id) {
            return;
        }
        let idx = self.tools.len();
        self.tools.push(cap);
        let added = self.tools.last().unwrap();

        if let Some(ref hint) = added.category_hint {
            let key = hint.to_lowercase();
            self.by_category.entry(key).or_default().push(idx);
        }
        if let Some(ref tags) = added.tags {
            for tag in tags {
                let key = tag.to_lowercase();
                self.by_tag.entry(key).or_default().push(idx);
            }
        }
    }

    pub fn list(&self) -> &[ToolCapability] {
        &self.tools
    }

    pub fn get(&self, id: &str) -> Option<&ToolCapability> {
        self.tools.iter().find(|t| t.id == id)
    }

    /// O(1) lookup: capabilities whose category_hint matches (case-insensitive).
    pub fn for_category(&self, category: &str) -> Vec<&ToolCapability> {
        let key = category.to_lowercase();
        self.by_category
            .get(&key)
            .map(|indices| indices.iter().filter_map(|&i| self.tools.get(i)).collect())
            .unwrap_or_default()
    }

    /// O(1) lookup: capabilities that have the given tag (case-insensitive).
    pub fn for_tag(&self, tag: &str) -> Vec<&ToolCapability> {
        let key = tag.to_lowercase();
        self.by_tag
            .get(&key)
            .map(|indices| indices.iter().filter_map(|&i| self.tools.get(i)).collect())
            .unwrap_or_default()
    }

    /// Returns capability ids whose category_hint or tags appear in the task (case-insensitive).
    /// Uses the index for efficient lookup when there are many capabilities.
    pub fn matching_ids_for_task(&self, task: &str) -> Vec<String> {
        let task_lower = task.trim().to_lowercase();
        let mut ids = HashSet::new();
        for (category, indices) in &self.by_category {
            if task_lower.contains(category) {
                for &i in indices {
                    if let Some(cap) = self.tools.get(i) {
                        ids.insert(cap.id.clone());
                    }
                }
            }
        }
        for (tag, indices) in &self.by_tag {
            if task_lower.contains(tag) {
                for &i in indices {
                    if let Some(cap) = self.tools.get(i) {
                        ids.insert(cap.id.clone());
                    }
                }
            }
        }
        let mut out: Vec<String> = ids.into_iter().collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_category_returns_matching_capabilities() {
        let mut reg = CapabilityRegistry::new();
        reg.register(ToolCapability {
            id: "agent.deploy".to_string(),
            description: "Deploy".to_string(),
            category_hint: Some("deploy".to_string()),
            tags: None,
            dangerous: true,
        });
        reg.register(ToolCapability {
            id: "shell.git_status".to_string(),
            description: "Git status".to_string(),
            category_hint: Some("git".to_string()),
            tags: None,
            dangerous: false,
        });
        let deploy = reg.for_category("deploy");
        assert_eq!(deploy.len(), 1);
        assert_eq!(deploy[0].id, "agent.deploy");
        let git = reg.for_category("git");
        assert_eq!(git.len(), 1);
        assert_eq!(git[0].id, "shell.git_status");
    }

    #[test]
    fn for_tag_returns_matching_capabilities() {
        let mut reg = CapabilityRegistry::new();
        reg.register(ToolCapability {
            id: "tool.a".to_string(),
            description: "A".to_string(),
            category_hint: None,
            tags: Some(vec!["search".to_string(), "code".to_string()]),
            dangerous: false,
        });
        let search = reg.for_tag("search");
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].id, "tool.a");
    }

    #[test]
    fn matching_ids_for_task_uses_index() {
        let mut reg = CapabilityRegistry::new();
        reg.register(ToolCapability {
            id: "shell.git_status".to_string(),
            description: "Git status".to_string(),
            category_hint: Some("git".to_string()),
            tags: None,
            dangerous: false,
        });
        let hints = reg.matching_ids_for_task("show me the git status");
        assert!(hints.contains(&"shell.git_status".to_string()));
    }
}
