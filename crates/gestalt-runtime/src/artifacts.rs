#![forbid(unsafe_code)]
//! `ArtifactStoreView` snapshotting, `Effect::Save` staging/promotion, and
//! `.gestalt/ledger.jsonl` atomic rollback support.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SAVE_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

use gestalt_kernel::{
    ArtifactEntry, ArtifactPatch, ArtifactStoreView, DiffHunk, StepResult, sha256_hex,
};
use serde::{Deserialize, Serialize};

use crate::RuntimeError;
use crate::blobs::BlobStore;
use crate::sandbox::{WorkspacePathGuard, normalize_rel_prefix};

/// Metadata written to `.gestalt/staging/<stage_id>/manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageManifest {
    /// Deterministic stage identifier.
    pub stage_id: String,
    /// Workspace-relative target artifact path.
    pub artifact: String,
    /// SHA-256 digest of the candidate staged content (or deletion sentinel).
    pub digest: String,
    /// Human-readable diff summary.
    pub diff_summary: String,
    /// Timestamp in milliseconds when staged.
    pub timestamp_ms: u64,
    /// Whether the staged operation deletes the target artifact.
    pub is_delete: bool,
}

/// Audit entry appended atomically to `.gestalt/ledger.jsonl` on promotion or rollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Stage identifier associated with this mutation.
    pub stage_id: String,
    /// Ledger action (`"promote"` or `"rollback"`).
    pub action: String,
    /// Workspace-relative target artifact path.
    pub artifact: String,
    /// SHA-256 blob digest of the file before this action (`None` if newly created).
    pub prev_blob_digest: Option<String>,
    /// SHA-256 blob digest of the file after this action (`None` if deleted).
    pub new_blob_digest: Option<String>,
    /// Human-readable summary of changes.
    pub diff_summary: String,
    /// Deterministic timestamp in milliseconds.
    pub timestamp_ms: u64,
}

/// Manages Turn-start artifact snapshots, `Effect::Save` staging/promotion,
/// and `.gestalt/ledger.jsonl` rollbacks.
#[derive(Debug, Clone)]
pub struct ArtifactManager {
    workspace_root: PathBuf,
    blob_store: BlobStore,
    path_guard: WorkspacePathGuard,
}

impl ArtifactManager {
    /// Construct an [`ArtifactManager`] for canonical `workspace_root`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Io`] if `workspace_root` cannot be canonicalized.
    pub fn new(workspace_root: &Path) -> Result<Self, RuntimeError> {
        let canon_root = fs::canonicalize(workspace_root)?;
        let path_guard = WorkspacePathGuard::new(&canon_root, &BTreeSet::new())?;
        let blob_store = BlobStore::new(&canon_root);
        Ok(Self {
            workspace_root: canon_root,
            blob_store,
            path_guard,
        })
    }

    /// Borrow the underlying [`BlobStore`].
    #[must_use]
    pub fn blob_store(&self) -> &BlobStore {
        &self.blob_store
    }

    /// Configure additional read-only directory roots outside `workspace_root`
    /// permitted for artifact snapshotting and read-only tool access.
    pub fn set_extra_read_roots(&mut self, roots: Vec<PathBuf>) {
        self.path_guard.set_extra_read_roots(roots);
    }

    /// Return configured read-only directory roots outside `workspace_root`.
    #[must_use]
    pub fn extra_read_roots(&self) -> &[PathBuf] {
        self.path_guard.extra_read_roots()
    }

    /// Snapshot existing workspace artifact files into [`BlobStore`] and return
    /// an [`ArtifactStoreView`] ready for `turn_start.artifact_snapshots`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if a path escapes `workspace_root` or I/O fails.
    pub fn snapshot_artifacts(
        &self,
        relative_paths: &[String],
    ) -> Result<ArtifactStoreView, RuntimeError> {
        let mut entries = BTreeMap::new();
        for rel in relative_paths {
            let norm = if Path::new(rel).is_absolute() {
                rel.clone()
            } else {
                normalize_rel_prefix(rel)
            };
            let resolved = self.path_guard.resolve_read(&norm)?;
            if !resolved.is_file() {
                continue;
            }
            let content = fs::read_to_string(&resolved)?;
            let blob_digest = self.blob_store.put_bytes(content.as_bytes())?;
            entries.insert(
                norm.clone(),
                ArtifactEntry {
                    path: norm,
                    content,
                    blob_digest,
                },
            );
        }
        Ok(ArtifactStoreView::from_entries(entries))
    }

    /// Reconstruct an [`ArtifactStoreView`] strictly from `BlobStore` using
    /// `turn_start.artifact_snapshots` for deterministic offline replay.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if any referenced blob is missing or corrupt.
    pub fn restore_view_from_blobs(
        &self,
        snapshots: &BTreeMap<String, String>,
    ) -> Result<ArtifactStoreView, RuntimeError> {
        let mut entries = BTreeMap::new();
        for (path, digest) in snapshots {
            let content = self.blob_store.get_utf8(digest)?;
            entries.insert(
                path.clone(),
                ArtifactEntry {
                    path: path.clone(),
                    content,
                    blob_digest: digest.clone(),
                },
            );
        }
        Ok(ArtifactStoreView::from_entries(entries))
    }

    /// Execute `Effect::Save` with `SaveMode::Stage`, writing candidate changes
    /// into `.gestalt/staging/<stage_id>/` without touching live files.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if the path escapes workspace, the diff fails to
    /// match, or staging I/O fails.
    pub fn stage_save(
        &self,
        effect_id: &str,
        stage_id: &str,
        artifact: &str,
        patch: &ArtifactPatch,
        timestamp_ms: u64,
    ) -> Result<StepResult, RuntimeError> {
        let (norm_rel, live_path) = self.resolve_artifact_target(artifact)?;
        let original = if live_path.is_file() {
            Some(fs::read_to_string(&live_path)?)
        } else {
            None
        };

        let next_content = apply_artifact_patch(original.as_deref(), patch)?;
        let (digest, diff_summary, is_delete) = match &next_content {
            Some(text) => (
                sha256_hex(text.as_bytes()),
                summarize_diff(&norm_rel, original.as_deref(), Some(text.as_str())),
                false,
            ),
            None => (
                sha256_hex(b"__GESTALT_DELETED__"),
                summarize_diff(&norm_rel, original.as_deref(), None),
                true,
            ),
        };

        let stage_dir = self.staging_dir().join(stage_id);
        fs::create_dir_all(&stage_dir)?;
        if let Some(ref text) = next_content {
            let staged_file = stage_dir.join("files").join(&norm_rel);
            if let Some(parent) = staged_file.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&staged_file, text)?;
        }

        let manifest = StageManifest {
            stage_id: stage_id.to_string(),
            artifact: norm_rel,
            digest: digest.clone(),
            diff_summary: diff_summary.clone(),
            timestamp_ms,
            is_delete,
        };
        let manifest_path = stage_dir.join("manifest.json");
        fs::write(manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

        Ok(StepResult::Saved {
            effect_id: effect_id.to_string(),
            stage_id: stage_id.to_string(),
            diff_summary,
            digest,
        })
    }

    /// Execute `Effect::Save` with `SaveMode::Promote`, snapshotting the prior
    /// file into `BlobStore`, appending `.gestalt/ledger.jsonl`, and atomically
    /// updating the live artifact path.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if path validation, patching, blob storage, or
    /// atomic write fails.
    pub fn promote_save(
        &self,
        effect_id: &str,
        stage_id: &str,
        artifact: &str,
        patch: &ArtifactPatch,
        timestamp_ms: u64,
    ) -> Result<StepResult, RuntimeError> {
        let (norm_rel, live_path) = self.resolve_artifact_target(artifact)?;
        let prev_bytes = if live_path.is_file() {
            Some(fs::read(&live_path)?)
        } else {
            None
        };
        let prev_text = match &prev_bytes {
            Some(b) => Some(String::from_utf8(b.clone()).map_err(|err| {
                RuntimeError::ArtifactError(format!(
                    "existing artifact '{norm_rel}' is not UTF-8: {err}"
                ))
            })?),
            None => None,
        };
        let prev_blob_digest = match &prev_bytes {
            Some(b) => Some(self.blob_store.put_bytes(b)?),
            None => None,
        };

        let next_content = apply_artifact_patch(prev_text.as_deref(), patch)?;
        let (new_blob_digest, digest, diff_summary) = match &next_content {
            Some(text) => {
                let d = self.blob_store.put_bytes(text.as_bytes())?;
                let summary = summarize_diff(&norm_rel, prev_text.as_deref(), Some(text.as_str()));
                (Some(d.clone()), d, summary)
            }
            None => (
                None,
                sha256_hex(b"__GESTALT_DELETED__"),
                summarize_diff(&norm_rel, prev_text.as_deref(), None),
            ),
        };

        // Apply live file mutation first so a failed write/delete never leaves
        // a phantom "promote" entry in `.gestalt/ledger.jsonl`.
        match next_content {
            Some(text) => atomic_write_file(&live_path, text.as_bytes())?,
            None => {
                if live_path.exists() {
                    fs::remove_file(&live_path)?;
                }
            }
        }

        let entry = LedgerEntry {
            stage_id: stage_id.to_string(),
            action: "promote".to_string(),
            artifact: norm_rel,
            prev_blob_digest,
            new_blob_digest,
            diff_summary: diff_summary.clone(),
            timestamp_ms,
        };
        self.append_ledger_entry(&entry)?;

        Ok(StepResult::Saved {
            effect_id: effect_id.to_string(),
            stage_id: stage_id.to_string(),
            diff_summary,
            digest,
        })
    }

    /// Revert a promoted `stage_id` using `.gestalt/ledger.jsonl` and `BlobStore`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ArtifactError`] if `stage_id` is missing from the
    /// ledger or if restoring the previous blob fails.
    pub fn rollback(&self, stage_id: &str, timestamp_ms: u64) -> Result<LedgerEntry, RuntimeError> {
        let entries = self.read_ledger()?;
        let target = entries
            .iter()
            .rev()
            .find(|e| e.stage_id == stage_id && e.action == "promote")
            .cloned()
            .ok_or_else(|| {
                RuntimeError::ArtifactError(format!(
                    "no promoted ledger entry found for stage_id '{stage_id}'"
                ))
            })?;

        let (_, live_path) = self.resolve_artifact_target(&target.artifact)?;
        match &target.prev_blob_digest {
            Some(prev_digest) => {
                let prev_bytes = self.blob_store.get_bytes(prev_digest)?;
                atomic_write_file(&live_path, &prev_bytes)?;
            }
            None => {
                if live_path.exists() {
                    fs::remove_file(&live_path)?;
                }
            }
        }

        let rollback_entry = LedgerEntry {
            stage_id: stage_id.to_string(),
            action: "rollback".to_string(),
            artifact: target.artifact,
            prev_blob_digest: target.new_blob_digest,
            new_blob_digest: target.prev_blob_digest,
            diff_summary: format!("rolled back stage '{stage_id}'"),
            timestamp_ms,
        };
        self.append_ledger_entry(&rollback_entry)?;
        Ok(rollback_entry)
    }

    /// Read all recorded entries from `.gestalt/ledger.jsonl`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if reading or JSONL parsing fails.
    pub fn read_ledger(&self) -> Result<Vec<LedgerEntry>, RuntimeError> {
        let path = self.ledger_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(path)?;
        let mut out = Vec::new();
        for line in raw.lines().map(str::trim).filter(|l| !l.is_empty()) {
            out.push(serde_json::from_str(line)?);
        }
        Ok(out)
    }

    fn staging_dir(&self) -> PathBuf {
        self.workspace_root.join(".gestalt").join("staging")
    }

    fn ledger_path(&self) -> PathBuf {
        self.workspace_root.join(".gestalt").join("ledger.jsonl")
    }

    fn append_ledger_entry(&self, entry: &LedgerEntry) -> Result<(), RuntimeError> {
        let path = self.ledger_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let line = serde_json::to_string(entry)?;
        writeln!(file, "{line}")?;
        file.flush()?;
        Ok(())
    }

    fn resolve_artifact_target(&self, raw_path: &str) -> Result<(String, PathBuf), RuntimeError> {
        if raw_path.contains('\0') {
            return Err(RuntimeError::SandboxViolation(
                "null byte in artifact path".to_string(),
            ));
        }
        let p = Path::new(raw_path);
        if p.is_absolute()
            || p.components().any(|c| {
                matches!(
                    c,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(RuntimeError::SandboxViolation(format!(
                "artifact path '{raw_path}' must be workspace-relative without '..'"
            )));
        }
        // Validate symlink confinement via `path_guard.resolve_read` (which does not
        // block protected_paths, since `Effect::Save` is the sanctioned gateway).
        let resolved = self.path_guard.resolve_read(raw_path)?;
        let norm = normalize_rel_prefix(raw_path);
        Ok((norm, resolved))
    }
}

/// Apply an [`ArtifactPatch`] to optional existing UTF-8 content.
///
/// # Errors
///
/// Returns [`RuntimeError::ArtifactError`] if `ApplyDiff` targets a missing file
/// or if any [`DiffHunk`] fails to match cleanly.
pub fn apply_artifact_patch(
    original: Option<&str>,
    patch: &ArtifactPatch,
) -> Result<Option<String>, RuntimeError> {
    match patch {
        ArtifactPatch::WriteFile { content } => Ok(Some(content.clone())),
        ArtifactPatch::Delete => Ok(None),
        ArtifactPatch::ApplyDiff { hunks } => {
            let Some(orig) = original else {
                return Err(RuntimeError::ArtifactError(
                    "cannot apply diff to non-existent artifact".to_string(),
                ));
            };
            let mut text = orig.to_string();
            for hunk in hunks {
                text = apply_single_hunk(&text, hunk)?;
            }
            Ok(Some(text))
        }
    }
}

fn apply_single_hunk(text: &str, hunk: &DiffHunk) -> Result<String, RuntimeError> {
    if hunk.target.is_empty() {
        return Err(RuntimeError::ArtifactError(
            "diff hunk target cannot be empty".to_string(),
        ));
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let start_idx = (hunk.old_start as usize).saturating_sub(1);
    if start_idx < lines.len() {
        let byte_offset: usize = lines[..start_idx].iter().map(|l| l.len()).sum();
        if text[byte_offset..].starts_with(&hunk.target) {
            return Ok(splice_replacement(text, byte_offset, hunk));
        }
    }

    let matches: Vec<usize> = text.match_indices(&hunk.target).map(|(i, _)| i).collect();
    if matches.len() == 1 {
        return Ok(splice_replacement(text, matches[0], hunk));
    }

    Err(RuntimeError::ArtifactError(format!(
        "diff hunk at line {} did not match uniquely (matches={})",
        hunk.old_start,
        matches.len()
    )))
}

fn splice_replacement(text: &str, idx: usize, hunk: &DiffHunk) -> String {
    let mut out = text.to_string();
    out.replace_range(idx..idx + hunk.target.len(), &hunk.replacement);
    out
}

fn summarize_diff(artifact: &str, before: Option<&str>, after: Option<&str>) -> String {
    match (before, after) {
        (None, Some(new)) => format!("created {artifact} ({} lines)", new.lines().count()),
        (Some(old), None) => format!("deleted {artifact} ({} lines)", old.lines().count()),
        (None, None) => format!("no-op delete {artifact}"),
        (Some(old), Some(new)) => {
            let old_lines = old.lines().count();
            let new_lines = new.lines().count();
            format!("updated {artifact} ({old_lines} -> {new_lines} lines)")
        }
    }
}

fn atomic_write_file(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("artifact");
    let seq = SAVE_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = path.with_file_name(format!(".{file_name}.tmp.{}.{seq}", std::process::id()));
    fs::write(&tmp_path, bytes)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_promote_and_rollback_cycle() {
        let dir = std::env::temp_dir().join(format!("gestalt_art_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".gestalt/protected/demo")).expect("mkdir");
        let artifact_path = dir.join(".gestalt/protected/demo/note.md");
        fs::write(&artifact_path, "v1\nline2\n").expect("write");

        let mgr = ArtifactManager::new(&dir).expect("mgr");
        let patch = ArtifactPatch::ApplyDiff {
            hunks: vec![DiffHunk {
                old_start: 2,
                old_lines: 1,
                new_lines: 1,
                target: "line2\n".to_string(),
                replacement: "line2_updated\n".to_string(),
            }],
        };

        let staged = mgr
            .stage_save(
                "eff_1",
                "stg_1",
                ".gestalt/protected/demo/note.md",
                &patch,
                100,
            )
            .expect("stage");
        assert!(matches!(staged, StepResult::Saved { .. }));
        assert_eq!(
            fs::read_to_string(&artifact_path).expect("live unchanged"),
            "v1\nline2\n"
        );

        mgr.promote_save(
            "eff_2",
            "stg_1",
            ".gestalt/protected/demo/note.md",
            &patch,
            200,
        )
        .expect("promote");
        assert_eq!(
            fs::read_to_string(&artifact_path).expect("live updated"),
            "v1\nline2_updated\n"
        );

        mgr.rollback("stg_1", 300).expect("rollback");
        assert_eq!(
            fs::read_to_string(&artifact_path).expect("live restored"),
            "v1\nline2\n"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
