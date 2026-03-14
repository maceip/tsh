//! # tsh-model-manager
//!
//! Handles downloading, caching, and verifying LLM model files for tsh.
//!
//! Models are stored in a platform-specific cache directory:
//! - Linux/macOS: `~/.cache/tsh/models/`
//! - Windows: `%LOCALAPPDATA%\tsh\models\`
//!
//! On first use, the model is downloaded from HuggingFace with progress
//! reporting and SHA-256 verification. Subsequent runs use the cached file.

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

// ---------------------------------------------------------------------------
// Model registry
// ---------------------------------------------------------------------------

/// A known model with its download URL and expected hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Human-readable name
    pub name: String,
    /// HuggingFace download URL (resolve/ not blob/)
    pub url: String,
    /// Expected SHA-256 hex digest (empty string to skip verification)
    pub sha256: String,
    /// Expected file size in bytes (0 to skip size check)
    pub size_bytes: u64,
    /// Local filename in the cache directory
    pub filename: String,
}

/// Returns the built-in model registry.
///
/// Add new models here as they become available. The registry is compiled
/// into the binary so tsh can resolve model names without network access.
pub fn default_registry() -> HashMap<String, ModelEntry> {
    let mut registry = HashMap::new();

    registry.insert(
        "gemma3-1b-q4".to_string(),
        ModelEntry {
            name: "Gemma 3 1B IT (Q4, 4096 context)".to_string(),
            url: "https://huggingface.co/litert-community/Gemma3-1B-IT/resolve/main/Gemma3-1B-IT_multi-prefill-seq_q4_ekv4096.litertlm".to_string(),
            sha256: String::new(), // TODO: pin hash after first verified download
            size_bytes: 0,         // TODO: pin size after first verified download
            filename: "Gemma3-1B-IT_q4_ekv4096.litertlm".to_string(),
        },
    );

    registry
}

// ---------------------------------------------------------------------------
// Cache directory
// ---------------------------------------------------------------------------

/// Returns the platform-specific model cache directory.
///
/// - Linux/macOS: `~/.cache/tsh/models/`
/// - Windows: `%LOCALAPPDATA%\tsh\models\`
///
/// Respects `TSH_MODEL_DIR` env var as an override.
pub fn model_cache_dir() -> Result<PathBuf> {
    if let Ok(override_dir) = std::env::var("TSH_MODEL_DIR") {
        return Ok(PathBuf::from(override_dir));
    }

    let base = dirs::cache_dir().context(
        "Could not determine cache directory. Set TSH_MODEL_DIR env var as a fallback.",
    )?;

    Ok(base.join("tsh").join("models"))
}

/// Returns the full path where a model file would be cached.
pub fn model_path(entry: &ModelEntry) -> Result<PathBuf> {
    Ok(model_cache_dir()?.join(&entry.filename))
}

// ---------------------------------------------------------------------------
// Download + verify
// ---------------------------------------------------------------------------

/// Ensures a model is available locally, downloading it if necessary.
///
/// Returns the absolute path to the cached model file.
///
/// If `progress_callback` is provided, it is called with `(bytes_downloaded, total_bytes)`
/// during the download. `total_bytes` may be 0 if the server doesn't send Content-Length.
pub async fn ensure_model<F>(
    entry: &ModelEntry,
    progress_callback: Option<F>,
) -> Result<PathBuf>
where
    F: Fn(u64, u64),
{
    let cache_dir = model_cache_dir()?;
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .with_context(|| format!("Failed to create cache directory: {}", cache_dir.display()))?;

    let dest = cache_dir.join(&entry.filename);

    // If already cached, verify and return.
    if dest.exists() {
        if !entry.sha256.is_empty() {
            let hash = sha256_file(&dest).await?;
            if hash == entry.sha256 {
                return Ok(dest);
            }
            eprintln!(
                "Cached model hash mismatch (expected {}, got {}). Re-downloading.",
                &entry.sha256[..12],
                &hash[..12]
            );
        } else {
            return Ok(dest);
        }
    }

    eprintln!("Downloading model: {}", entry.name);
    eprintln!("  URL: {}", entry.url);
    eprintln!("  Destination: {}", dest.display());

    download_file(&entry.url, &dest, progress_callback).await?;

    // Verify hash if specified.
    if !entry.sha256.is_empty() {
        let hash = sha256_file(&dest).await?;
        if hash != entry.sha256 {
            // Remove the corrupt download.
            let _ = tokio::fs::remove_file(&dest).await;
            anyhow::bail!(
                "SHA-256 mismatch after download. Expected: {}, Got: {}",
                entry.sha256,
                hash
            );
        }
        eprintln!("  SHA-256 verified.");
    }

    eprintln!("  Download complete.");
    Ok(dest)
}

/// Downloads a file from `url` to `dest` with optional progress reporting.
async fn download_file<F>(url: &str, dest: &Path, progress_callback: Option<F>) -> Result<()>
where
    F: Fn(u64, u64),
{
    let client = reqwest::Client::builder()
        .user_agent("tsh-model-manager/0.1")
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to start download from {}", url))?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Download failed with HTTP {}: {}", status, url);
    }

    let total = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    // Write to a temp file first, then rename. This prevents partial files
    // from being mistaken for complete downloads.
    let tmp_dest = dest.with_extension("downloading");
    let mut file = tokio::fs::File::create(&tmp_dest)
        .await
        .with_context(|| format!("Failed to create temp file: {}", tmp_dest.display()))?;

    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Error reading download stream")?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if let Some(ref cb) = progress_callback {
            cb(downloaded, total);
        }
    }

    file.flush().await?;
    drop(file);

    // Atomic rename (same filesystem).
    tokio::fs::rename(&tmp_dest, dest)
        .await
        .with_context(|| format!("Failed to rename {} to {}", tmp_dest.display(), dest.display()))?;

    Ok(())
}

/// Computes the SHA-256 hex digest of a file.
async fn sha256_file(path: &Path) -> Result<String> {
    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("Failed to read file for hashing: {}", path.display()))?;

    let mut hasher = Sha256::new();
    hasher.update(&data);
    let hash = hasher.finalize();
    Ok(hex_encode(&hash))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

// ---------------------------------------------------------------------------
// Listing + cleanup
// ---------------------------------------------------------------------------

/// Lists all cached model files with their sizes.
pub async fn list_cached_models() -> Result<Vec<(String, u64)>> {
    let cache_dir = model_cache_dir()?;
    if !cache_dir.exists() {
        return Ok(vec![]);
    }

    let mut entries = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&cache_dir).await?;

    while let Some(entry) = read_dir.next_entry().await? {
        let metadata = entry.metadata().await?;
        if metadata.is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            entries.push((name, metadata.len()));
        }
    }

    Ok(entries)
}

/// Removes a cached model file.
pub async fn remove_cached_model(filename: &str) -> Result<()> {
    let path = model_cache_dir()?.join(filename);
    if path.exists() {
        tokio::fs::remove_file(&path)
            .await
            .with_context(|| format!("Failed to remove {}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_gemma() {
        let reg = default_registry();
        assert!(reg.contains_key("gemma3-1b-q4"));
        let entry = &reg["gemma3-1b-q4"];
        assert!(entry.url.contains("huggingface.co"));
        assert!(entry.filename.ends_with(".litertlm"));
    }

    #[test]
    fn cache_dir_resolves() {
        // Should not panic on any platform.
        let dir = model_cache_dir().unwrap();
        assert!(dir.to_string_lossy().contains("tsh"));
    }

    #[test]
    fn hex_encode_works() {
        assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }
}
