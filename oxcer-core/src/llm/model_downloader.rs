//! Model download and cache. Ensures required model files are present under `models_dir`.
//!
//! Reads [crate::llm::config::ModelsConfig] from config; supports pluggable sources
//! (Hugging Face first; Azure, local share, etc. can be added without hard-coding).

use std::path::{Path, PathBuf};

use crate::llm::config::{load_models_config, ModelDef};
use crate::llm::LlmError;

/// Progress update during a single file download. Passed to optional callback.
#[derive(Clone, Debug)]
pub struct DownloadProgress {
    pub file_name: String,
    pub bytes_downloaded: u64,
    /// Total size when known (from Content-Length).
    pub total_bytes: Option<u64>,
}

/// Check that path exists, is a file, and has size > 0.
fn file_present_and_non_empty(path: &Path) -> bool {
    path.is_file()
        && std::fs::metadata(path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
}

/// Returns the local model directory path if all required files are present (and non-empty).
/// If any file is missing, runs the appropriate download (e.g. Hugging Face), then returns the path.
///
/// - `config_dir`: directory containing `models.yaml`.
/// - `models_dir`: root for model dirs (e.g. `models_dir/phi3-small`).
/// - `on_progress`: optional callback for progress (file name, bytes, total); called during download.
pub fn ensure_model_present(
    model_id: &str,
    config_dir: &Path,
    models_dir: &Path,
    on_progress: Option<&(dyn Fn(DownloadProgress) + Send)>,
) -> Result<PathBuf, LlmError> {
    let models_config = load_models_config(config_dir)?;
    let def = models_config.models.get(model_id).ok_or_else(|| {
        LlmError::Config(format!("Model '{}' not found in models.yaml", model_id))
    })?;

    let model_root = models_dir.join(model_id);
    std::fs::create_dir_all(&model_root).map_err(|e| {
        LlmError::Config(format!(
            "Failed to create model directory {:?}: {}",
            model_root, e
        ))
    })?;

    let missing: Vec<&String> = def
        .files
        .iter()
        .filter(|f| !file_present_and_non_empty(&model_root.join(*f)))
        .collect();

    if missing.is_empty() {
        log::info!("Model {} already present at {:?}", model_id, model_root);
        return Ok(model_root);
    }

    log::info!(
        "Model {}: downloading {} missing file(s) into {:?}",
        model_id,
        missing.len(),
        model_root
    );

    match def.source.as_str() {
        "huggingface" => download_from_huggingface(def, &model_root, missing, on_progress)?,
        _ => {
            return Err(LlmError::NotAvailable(format!(
                "Unsupported model source '{}' for model '{}'. Supported: huggingface",
                def.source, model_id
            )))
        }
    }

    // Verify all files are now present.
    for f in &def.files {
        let p = model_root.join(f.as_str());
        if !file_present_and_non_empty(&p) {
            return Err(LlmError::Config(format!(
                "After download, file is still missing or empty: {:?}",
                p
            )));
        }
    }

    log::info!("Model {} download complete at {:?}", model_id, model_root);
    Ok(model_root)
}

/// Hugging Face resolve URL: https://huggingface.co/{repo}/resolve/{revision}/{filename}
fn hf_resolve_url(repo: &str, revision: &str, filename: &str) -> String {
    format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        repo, revision, filename
    )
}

/// Download required files from Hugging Face.
fn download_from_huggingface(
    def: &ModelDef,
    model_root: &Path,
    missing: Vec<&String>,
    on_progress: Option<&(dyn Fn(DownloadProgress) + Send)>,
) -> Result<(), LlmError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .map_err(|e| LlmError::Internal(format!("HTTP client build failed: {}", e)))?;

    for file_name in missing {
        let url = hf_resolve_url(&def.repo, &def.revision, file_name);
        let dest = model_root.join(file_name);

        log::info!("Downloading {} -> {:?}", url, dest);

        if let Some(ref cb) = on_progress {
            cb(DownloadProgress {
                file_name: file_name.clone(),
                bytes_downloaded: 0,
                total_bytes: None,
            });
        }

        let resp = client.get(&url).send().map_err(|e| {
            LlmError::NotAvailable(format!("Download failed for {}: {}", file_name, e))
        })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(LlmError::NotAvailable(format!(
                "Download failed for {}: HTTP {} {}",
                file_name, status, body
            )));
        }

        let total_bytes = resp.content_length();
        let bytes = resp.bytes().map_err(|e| {
            LlmError::NotAvailable(format!(
                "Failed to read response body for {}: {}",
                file_name, e
            ))
        })?;

        if bytes.is_empty() {
            return Err(LlmError::Config(format!(
                "Downloaded file is empty: {}",
                file_name
            )));
        }

        std::fs::write(&dest, &bytes)
            .map_err(|e| LlmError::Config(format!("Failed to write {:?}: {}", dest, e)))?;

        let len = bytes.len() as u64;
        if let Some(ref cb) = on_progress {
            cb(DownloadProgress {
                file_name: file_name.clone(),
                bytes_downloaded: len,
                total_bytes: Some(total_bytes.unwrap_or(len)),
            });
        }
        log::info!("Downloaded {} ({} bytes)", file_name, len);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_present_and_non_empty_requires_file() {
        let tmp = std::env::temp_dir();
        assert!(!file_present_and_non_empty(&tmp));
        let missing = tmp.join("nonexistent_oxcer_12345");
        assert!(!file_present_and_non_empty(&missing));
    }

    #[test]
    fn ensure_model_present_unknown_model_id_returns_config_error() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
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
        let r = ensure_model_present("unknown-model", &config_dir, &models_dir, None);
        assert!(r.is_err());
        let e = r.unwrap_err();
        assert!(e.to_string().contains("not found") || e.to_string().contains("Unknown"));
    }

    #[test]
    fn ensure_model_present_when_files_present_returns_path() {
        let tmp = tempfile::tempdir().unwrap();
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
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
        let model_root = models_dir.join("phi3-small");
        std::fs::create_dir_all(&model_root).unwrap();
        std::fs::write(model_root.join("model.gguf"), "gguf").unwrap();
        std::fs::write(model_root.join("tokenizer.json"), "{}").unwrap();

        let r = ensure_model_present("phi3-small", &config_dir, &models_dir, None);
        assert!(r.is_ok());
        assert_eq!(r.unwrap(), model_root);
    }
}
