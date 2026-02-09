//! LLM profile and model configuration. Loads from llm_profiles.yaml and models.yaml.

use serde::Deserialize;
use std::path::Path;

use crate::llm::LlmError;

/// Top-level LLM profiles config (llm_profiles.yaml).
#[derive(Debug, Deserialize)]
pub struct LlmProfilesConfig {
    pub profiles: std::collections::HashMap<String, LlmProfile>,
}

#[derive(Debug, Deserialize)]
pub struct LlmProfile {
    pub engine: String,
    #[serde(default)]
    pub allow_external: bool,
    pub hybrid: Option<HybridProfile>,
    pub external_policy: Option<ExternalPolicy>,
}

#[derive(Debug, Deserialize)]
pub struct HybridProfile {
    pub primary: String,
    pub fallback: String,
}

#[derive(Debug, Deserialize)]
pub struct ExternalPolicy {
    #[serde(default)]
    pub never_send: Vec<String>,
}

/// Top-level models config (models.yaml).
#[derive(Debug, Deserialize)]
pub struct ModelsConfig {
    pub models: std::collections::HashMap<String, ModelDef>,
}

#[derive(Debug, Deserialize)]
pub struct ModelDef {
    pub source: String,
    pub repo: String,
    #[serde(default = "default_revision")]
    pub revision: String,
    pub files: Vec<String>,
    #[serde(default)]
    pub license_notice: String,
}

fn default_revision() -> String {
    "main".to_string()
}

/// Load [LlmProfilesConfig] from a config directory (e.g. oxcer-core/config).
pub fn load_llm_profiles(config_dir: &Path) -> Result<LlmProfilesConfig, LlmError> {
    let path = config_dir.join("llm_profiles.yaml");
    let contents = std::fs::read_to_string(&path).map_err(|e| {
        LlmError::Config(format!("Failed to read {:?}: {}", path, e))
    })?;
    serde_yaml::from_str(&contents).map_err(|e| {
        LlmError::Config(format!("Invalid llm_profiles.yaml: {}", e))
    })
}

/// Load [ModelsConfig] from a config directory.
pub fn load_models_config(config_dir: &Path) -> Result<ModelsConfig, LlmError> {
    let path = config_dir.join("models.yaml");
    let contents = std::fs::read_to_string(&path).map_err(|e| {
        LlmError::Config(format!("Failed to read {:?}: {}", path, e))
    })?;
    serde_yaml::from_str(&contents).map_err(|e| {
        LlmError::Config(format!("Invalid models.yaml: {}", e))
    })
}
