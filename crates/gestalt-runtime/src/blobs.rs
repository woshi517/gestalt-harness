#![forbid(unsafe_code)]
//! Content-addressed `.gestalt/blobs/<sha256>` store and 32 KB spill-to-blob helper.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use gestalt_kernel::sha256_hex;

use crate::RuntimeError;

/// Default inline payload threshold (`32 KB`) above which outputs spill to `BlobStore`.
pub const DEFAULT_INLINE_SPILL_BYTES: usize = 32 * 1024;

/// Maximum head/tail preview size (`4 KB` each) kept inline when a payload spills.
const PREVIEW_EDGE_BYTES: usize = 4 * 1024;

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Content-addressed blob store backed by `<workspace_root>/.gestalt/blobs/<sha256>`.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
    spill_threshold: usize,
}

impl BlobStore {
    /// Create a [`BlobStore`] rooted at `<workspace_root>/.gestalt/blobs` with the
    /// default `32 KB` inline spill threshold.
    #[must_use]
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            root: workspace_root.join(".gestalt").join("blobs"),
            spill_threshold: DEFAULT_INLINE_SPILL_BYTES,
        }
    }

    /// Create a [`BlobStore`] at an explicit directory path with a custom spill threshold.
    #[must_use]
    pub fn with_root_and_threshold(root: PathBuf, spill_threshold: usize) -> Self {
        Self {
            root,
            spill_threshold,
        }
    }

    /// Return the directory path where blobs are stored.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the configured inline spill threshold in bytes.
    #[must_use]
    pub fn spill_threshold(&self) -> usize {
        self.spill_threshold
    }

    /// Store arbitrary bytes atomically in `.gestalt/blobs/<sha256>` and return
    /// the 64-character lowercase hex SHA-256 digest.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Io`] if directory creation, temp-file write, or
    /// atomic rename fails.
    pub fn put_bytes(&self, data: &[u8]) -> Result<String, RuntimeError> {
        let digest = sha256_hex(data);
        let dest = self.root.join(&digest);
        if dest.exists() {
            return Ok(digest);
        }

        fs::create_dir_all(&self.root)?;
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_name = format!(".tmp_{}_{}_{}", digest, std::process::id(), seq);
        let tmp_path = self.root.join(tmp_name);
        fs::write(&tmp_path, data)?;
        if let Err(err) = fs::rename(&tmp_path, &dest) {
            let _ = fs::remove_file(&tmp_path);
            if !dest.exists() {
                return Err(err.into());
            }
        }
        Ok(digest)
    }

    /// Read and integrity-verify raw bytes for `digest` from `.gestalt/blobs/<sha256>`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ArtifactError`] on invalid digest format or SHA-256
    /// mismatch, or [`RuntimeError::Io`] if the blob file cannot be read.
    pub fn get_bytes(&self, digest: &str) -> Result<Vec<u8>, RuntimeError> {
        validate_sha256_hex(digest)?;
        let path = self.root.join(digest);
        let bytes = fs::read(&path)?;
        let actual = sha256_hex(&bytes);
        if actual != digest {
            return Err(RuntimeError::ArtifactError(format!(
                "blob integrity mismatch for '{digest}': actual sha256 '{actual}'"
            )));
        }
        Ok(bytes)
    }

    /// Read and integrity-verify UTF-8 text for `digest`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if reading, SHA-256 verification, or UTF-8 decoding fails.
    pub fn get_utf8(&self, digest: &str) -> Result<String, RuntimeError> {
        let bytes = self.get_bytes(digest)?;
        String::from_utf8(bytes).map_err(|err| {
            RuntimeError::ArtifactError(format!("blob '{digest}' is not valid UTF-8: {err}"))
        })
    }

    /// Spill `text` to `BlobStore` if its byte length exceeds `spill_threshold`.
    ///
    /// Returns `(text, None)` when within threshold, or `(head_tail_preview, Some(digest))`
    /// when spilled.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if writing the spilled blob fails.
    pub fn spill_if_oversized(&self, text: &str) -> Result<(String, Option<String>), RuntimeError> {
        if text.len() <= self.spill_threshold {
            return Ok((text.to_string(), None));
        }
        let digest = self.put_bytes(text.as_bytes())?;
        let preview = build_utf8_preview(text, &digest);
        Ok((preview, Some(digest)))
    }
}

fn validate_sha256_hex(digest: &str) -> Result<(), RuntimeError> {
    let valid = digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid {
        return Err(RuntimeError::ArtifactError(format!(
            "invalid sha256 blob digest '{digest}'"
        )));
    }
    Ok(())
}

fn build_utf8_preview(text: &str, digest: &str) -> String {
    let edge = PREVIEW_EDGE_BYTES.min(text.len() / 2);
    let head_end = floor_char_boundary(text, edge);
    let tail_start = ceil_char_boundary(text, text.len().saturating_sub(edge));

    let head = &text[..head_end];
    let tail = &text[tail_start..];
    format!(
        "{head}\n... [spilled {} bytes to .gestalt/blobs/{digest}] ...\n{tail}",
        text.len()
    )
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_store_spill_and_integrity_roundtrip() {
        let dir = std::env::temp_dir().join(format!("gestalt_blob_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let store = BlobStore::new(&dir);

        let small = "hello small payload";
        let (inline, digest) = store.spill_if_oversized(small).expect("small inline");
        assert_eq!(inline, small);
        assert!(digest.is_none());

        let large = "🦀".repeat(12_000); // 48 KB of 4-byte UTF-8 chars
        let (preview, maybe_digest) = store.spill_if_oversized(&large).expect("large spill");
        let digest = maybe_digest.expect("spilled digest");
        assert!(preview.contains(&digest));
        assert!(preview.len() < large.len());

        let loaded = store.get_utf8(&digest).expect("load spilled utf8");
        assert_eq!(loaded, large);

        // Tamper detection
        let blob_path = store.root().join(&digest);
        fs::write(&blob_path, b"corrupted").expect("corrupt blob");
        assert!(matches!(
            store.get_bytes(&digest),
            Err(RuntimeError::ArtifactError(_))
        ));

        let _ = fs::remove_dir_all(&dir);
    }
}
