//! Streaming file download with progress callback.
//! Used by the FFI layer to download large model files (e.g. Phi-3 GGUF) while
//! reporting progress to Swift.

use std::path::Path;
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::llm::LlmError;

/// Callback for download progress. Implementations (e.g. from FFI) receive
/// progress from 0.0 to 1.0 and a user-friendly status message.
pub trait DownloadProgressCallback: Send + Sync {
    fn on_progress(&self, progress: f64, message: String);
}

/// Download a file from URL to destination path, streaming chunk-by-chunk
/// and invoking the callback with progress (0.0 .. 1.0) when total size is known.
pub async fn download_file(
    url: &str,
    dest: &Path,
    callback: Arc<dyn DownloadProgressCallback>,
) -> Result<(), LlmError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3600))
        .build()
        .map_err(|e| LlmError::Internal(format!("HTTP client build failed: {}", e)))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| LlmError::NotAvailable(format!("Download request failed: {}", e)))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(LlmError::NotAvailable(format!(
            "Download failed: HTTP {} {}",
            status, body
        )));
    }

    let total_bytes = resp.content_length();
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| LlmError::Config(format!("Failed to create dir {:?}: {}", parent, e)))?;
    }

    let mut stream = resp.bytes_stream();
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|e| LlmError::Config(format!("Failed to create file {:?}: {}", dest, e)))?;

    let mut downloaded: u64 = 0;
    let callback_clone = Arc::clone(&callback);

    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|e| LlmError::NotAvailable(format!("Stream error: {}", e)))?;

        let len = chunk.len() as u64;
        file.write_all(&chunk)
            .await
            .map_err(|e| LlmError::Config(format!("Failed to write to {:?}: {}", dest, e)))?;

        downloaded += len;

        if let Some(total) = total_bytes {
            let progress = (downloaded as f64) / (total as f64);
            let pct = (progress * 100.0) as u32;
            callback_clone.on_progress(
                progress,
                format!("Downloading... {}%", pct),
            );
        } else {
            // Total unknown; report "downloading" with indeterminate progress
            callback_clone.on_progress(
                0.0,
                format!("Downloading... {} MB", downloaded / 1_048_576),
            );
        }
    }

    file.flush()
        .await
        .map_err(|e| LlmError::Config(format!("Failed to flush {:?}: {}", dest, e)))?;

    let meta = tokio::fs::metadata(dest)
        .await
        .map_err(|e| LlmError::Config(format!("Failed to stat {:?}: {}", dest, e)))?;

    if meta.len() == 0 {
        return Err(LlmError::Config(
            "Downloaded file is empty".to_string(),
        ));
    }

    Ok(())
}
