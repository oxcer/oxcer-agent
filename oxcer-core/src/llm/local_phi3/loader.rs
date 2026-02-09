//! Weight and tokenizer loading for Phi-3-small.
//!
//! Resolves model root, validates expected files (e.g. `model.gguf`, `tokenizer.json`),
//! loads tokenizer from JSON, and prepares for in-process runtime (llama.cpp or ONNX).
//! No HTTP, no ports.

use std::path::{Path, PathBuf};

use tokenizers::Tokenizer;

use crate::llm::LlmError;

/// Expected files under the model root (configurable via models.yaml later).
pub const DEFAULT_MODEL_GGUF: &str = "model.gguf";
pub const DEFAULT_TOKENIZER_JSON: &str = "tokenizer.json";

/// Resolved paths and metadata for a loaded Phi-3 model.
#[derive(Debug)]
pub struct Phi3ModelPaths {
    #[allow(dead_code)]
    pub model_root: PathBuf,
    pub model_gguf: PathBuf,
    pub tokenizer_json: PathBuf,
}

/// Resolve and validate model root. Returns paths to `model.gguf` and `tokenizer.json`.
/// Fails with [LlmError::Config] if the directory or required files are missing.
pub fn resolve_model_paths(model_root: &Path) -> Result<Phi3ModelPaths, LlmError> {
    let model_root = model_root
        .canonicalize()
        .map_err(|e| LlmError::Config(format!("Invalid model root {:?}: {}", model_root, e)))?;

    if !model_root.is_dir() {
        return Err(LlmError::Config(format!(
            "Model root is not a directory: {:?}",
            model_root
        )));
    }

    let model_gguf = model_root.join(DEFAULT_MODEL_GGUF);
    let tokenizer_json = model_root.join(DEFAULT_TOKENIZER_JSON);

    if !model_gguf.is_file() {
        return Err(LlmError::Config(format!(
            "Model weights not found: {:?}. Place {} in the model root or run model download.",
            model_gguf, DEFAULT_MODEL_GGUF
        )));
    }
    if !tokenizer_json.is_file() {
        return Err(LlmError::Config(format!(
            "Tokenizer not found: {:?}. Place {} in the model root or run model download.",
            tokenizer_json, DEFAULT_TOKENIZER_JSON
        )));
    }

    Ok(Phi3ModelPaths {
        model_root: model_root.clone(),
        model_gguf,
        tokenizer_json,
    })
}

/// Load tokenizer from `tokenizer.json`. Used by [super::LocalPhi3Engine] for encode/decode.
pub fn load_tokenizer(tokenizer_path: &Path) -> Result<Tokenizer, LlmError> {
    if !tokenizer_path.is_file() {
        return Err(LlmError::Config(format!(
            "Tokenizer file not found: {:?}",
            tokenizer_path
        )));
    }
    Tokenizer::from_file(tokenizer_path).map_err(|e| {
        LlmError::Config(format!(
            "Failed to load tokenizer from {:?}: {}",
            tokenizer_path, e
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_model_paths_requires_dir() {
        let tmp = std::env::temp_dir();
        let not_dir = tmp.join("oxcer_llm_nonexistent_phi3_dir_12345");
        let r = resolve_model_paths(&not_dir);
        assert!(r.is_err());
    }
}
