#![forbid(unsafe_code)]
//! Detached git worktree lifecycle for `WorkspaceMode::Worktree` subagent
//! isolation (`.gestalt/worktrees/<spawn_id>/`).

use std::fs;
use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::RuntimeError;
use crate::blobs::BlobStore;

/// Create a detached git worktree under `<workspace_root>/.gestalt/worktrees/<spawn_id>`.
///
/// # Errors
///
/// Returns [`RuntimeError::WorktreeError`] if `workspace_root` is not a git
/// repository or `git worktree add` fails.
pub async fn create_worktree(
    workspace_root: &Path,
    spawn_id: &str,
) -> Result<PathBuf, RuntimeError> {
    let check = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(workspace_root)
        .output()
        .await
        .map_err(|err| RuntimeError::WorktreeError(format!("failed to invoke git: {err}")))?;
    if !check.status.success() || String::from_utf8_lossy(&check.stdout).trim() != "true" {
        return Err(RuntimeError::WorktreeError(
            "workspace is not a git repository; WorkspaceMode::Worktree unavailable".to_string(),
        ));
    }

    let worktrees_dir = workspace_root.join(".gestalt").join("worktrees");
    fs::create_dir_all(&worktrees_dir)?;
    let wt_path = worktrees_dir.join(spawn_id);

    let add = Command::new("git")
        .args([
            "worktree",
            "add",
            "--detach",
            &wt_path.to_string_lossy(),
            "HEAD",
        ])
        .current_dir(workspace_root)
        .output()
        .await
        .map_err(|err| RuntimeError::WorktreeError(format!("git worktree add failed: {err}")))?;
    if !add.status.success() {
        return Err(RuntimeError::WorktreeError(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        )));
    }
    Ok(wt_path)
}

/// Capture a unified diff of all changes made inside `worktree_path` into [`BlobStore`].
///
/// # Errors
///
/// Returns [`RuntimeError`] if git diff or blob storage fails.
pub async fn capture_worktree_diff_blob(
    worktree_path: &Path,
    blob_store: &BlobStore,
) -> Result<Option<String>, RuntimeError> {
    let _ = Command::new("git")
        .args(["add", "-N", "."])
        .current_dir(worktree_path)
        .output()
        .await;

    let diff = Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(worktree_path)
        .output()
        .await
        .map_err(|err| RuntimeError::WorktreeError(format!("git diff HEAD failed: {err}")))?;
    if !diff.status.success() {
        return Err(RuntimeError::WorktreeError(format!(
            "git diff HEAD failed: {}",
            String::from_utf8_lossy(&diff.stderr).trim()
        )));
    }
    if diff.stdout.is_empty() {
        return Ok(None);
    }
    let digest = blob_store.put_bytes(&diff.stdout)?;
    Ok(Some(digest))
}

/// Prune clean temporary worktrees under `<workspace_root>/.gestalt/worktrees/`
/// on session close.
pub async fn prune_clean_worktrees(workspace_root: &Path) -> usize {
    let wt_root = workspace_root.join(".gestalt").join("worktrees");
    let Ok(entries) = fs::read_dir(&wt_root) else {
        return 0;
    };
    let mut pruned = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&path)
            .output()
            .await;
        let is_clean = status.is_ok_and(|out| out.status.success() && out.stdout.is_empty());
        if is_clean {
            let rm = Command::new("git")
                .args(["worktree", "remove", "--force", &path.to_string_lossy()])
                .current_dir(workspace_root)
                .output()
                .await;
            if rm.is_ok_and(|o| o.status.success()) {
                pruned += 1;
            }
        }
    }
    let _ = Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(workspace_root)
        .output()
        .await;
    pruned
}
