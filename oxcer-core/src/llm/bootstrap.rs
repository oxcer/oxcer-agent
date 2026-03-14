//! Bootstrap: build an [LlmEngine] from a selected profile.
//!
//! Launchers (desktop-tauri, OxcerLauncher, windows-launcher) should:
//! - Choose a profile (e.g. `local-only` or `hybrid`) via CLI flag, env var, or config.
//! - Call [create_engine_for_profile] with that profile name, config dir, and models dir.
//! - Use the returned `Arc<dyn LlmEngine>` for all generation; they never need to know
//!   whether the core is using local or external engines.

use std::path::Path;
use std::sync::Arc;

use crate::llm::config::load_llm_profiles;
use crate::llm::model_downloader::ensure_model_present;
use crate::llm::{
    HttpLlmConfig, HttpLlmEngine, HybridEngine, LlmEngine, LlmError, LocalPhi3Engine,
};

/// Build the [LlmEngine] for the given profile. Uses `config_dir` for llm_profiles.yaml
/// and models.yaml; uses `models_dir` as the root for local model dirs (e.g. models_dir/phi3-small).
///
/// For hybrid profiles, `http_fallback_config` must be provided for the HTTP fallback engine;
/// launchers typically pass this from env (e.g. OPENAI_API_KEY, OPENAI_BASE_URL).
pub fn create_engine_for_profile(
    profile_name: &str,
    config_dir: &Path,
    models_dir: &Path,
    http_fallback_config: Option<HttpLlmConfig>,
) -> Result<Arc<dyn LlmEngine>, LlmError> {
    let profiles = load_llm_profiles(config_dir)?;
    let profile = profiles
        .profiles
        .get(profile_name)
        .ok_or_else(|| LlmError::Config(format!("Unknown LLM profile: {}", profile_name)))?;

    match profile.engine.as_str() {
        "local-phi3" => {
            let model_root = ensure_model_present("phi3-small", config_dir, models_dir, None)?;
            log::info!("Local Phi-3 engine using model root: {:?}", model_root);
            let engine = LocalPhi3Engine::new(&model_root)?;
            Ok(Arc::new(engine))
        }
        "hybrid" => {
            let fallback_config = http_fallback_config.ok_or_else(|| {
                LlmError::Config("Hybrid profile requires http_fallback_config".to_string())
            })?;
            let hybrid_cfg = profile.hybrid.as_ref().ok_or_else(|| {
                LlmError::Config("Hybrid profile missing 'hybrid' config".to_string())
            })?;
            let primary = build_engine_by_name(&hybrid_cfg.primary, config_dir, models_dir, None)?;
            let fallback = Arc::new(HttpLlmEngine::new(fallback_config));
            Ok(Arc::new(HybridEngine::new(primary, fallback)))
        }
        _ => Err(LlmError::Config(format!(
            "Unsupported engine type: {}",
            profile.engine
        ))),
    }
}

/// Build a single engine by logical name (e.g. "local-phi3"). Used for primary in hybrid.
fn build_engine_by_name(
    name: &str,
    config_dir: &Path,
    models_dir: &Path,
    _http_config: Option<HttpLlmConfig>,
) -> Result<Arc<dyn LlmEngine>, LlmError> {
    match name {
        "local-phi3" => {
            let model_root = ensure_model_present("phi3-small", config_dir, models_dir, None)?;
            let engine = LocalPhi3Engine::new(&model_root)?;
            Ok(Arc::new(engine))
        }
        _ => Err(LlmError::Config(format!("Unknown engine name: {}", name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_config_with_profiles() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("llm_profiles.yaml"),
            r#"profiles:
  local-only:
    engine: local-phi3
    allow_external: false
  hybrid:
    engine: hybrid
    hybrid:
      primary: local-phi3
      fallback: http-openai
    allow_external: true
"#,
        )
        .unwrap();
        std::fs::write(
            config_dir.join("models.yaml"),
            r#"models:
  phi3-small:
    source: huggingface
    repo: microsoft/Phi-3-small-128k-instruct
    revision: main
    files: [model.gguf, tokenizer.json]
"#,
        )
        .unwrap();
        let models_dir = tmp.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        (tmp, config_dir, models_dir)
    }

    #[test]
    fn unknown_profile_returns_config_error() {
        let (_tmp, config_dir, models_dir) = temp_config_with_profiles();
        let r = create_engine_for_profile("unknown-profile", &config_dir, &models_dir, None);
        assert!(r.is_err());
        let err = r.err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("Unknown") || msg.contains("profile"),
            "{}",
            msg
        );
    }

    /// POLICY: Core tests are OFFLINE-SAFE. This test verifies that when hybrid mode is configured
    /// WITHOUT fallback config, the system returns a CONFIG ERROR. We require http_fallback_config
    /// before building the primary engine so this path never touches the network or model download.
    #[test]
    fn hybrid_without_fallback_config_returns_config_error() {
        let (_tmp, config_dir, models_dir) = temp_config_with_profiles();
        let r = create_engine_for_profile("hybrid", &config_dir, &models_dir, None);
        assert!(
            r.is_err(),
            "hybrid without fallback must return Config error"
        );
        let err = r.err().unwrap();
        let msg = err.to_string();
        assert!(
            msg.contains("http_fallback_config") || msg.contains("fallback"),
            "error must be configuration-related, not network/404: {}",
            msg
        );
    }
}
