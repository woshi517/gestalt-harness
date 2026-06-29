use chrono::Utc;
use gestalt_core::{
    error::HarnessError,
    snapshot::{WorkspaceSnapshot, WorkspaceSnapshotter},
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

pub struct GitWorkspaceSnapshotter;

#[async_trait::async_trait]
impl WorkspaceSnapshotter for GitWorkspaceSnapshotter {
    async fn capture(&self, root: &Path) -> Result<WorkspaceSnapshot, HarnessError> {
        let mut git_sha = None;
        let mut git_dirty = None;
        let mut untracked_count = None;
        let mut files = Vec::new();

        let is_git = run_git_cmd(&["rev-parse", "--is-inside-work-tree"], root).await;
        if is_git.as_deref() == Some("true") {
            git_sha = run_git_cmd(&["rev-parse", "HEAD"], root).await;
            if let Some(status_out) = run_git_cmd(&["status", "--porcelain"], root).await {
                let mut dirty = false;
                let mut untracked = 0;
                for line in status_out.lines() {
                    if line.starts_with("??") {
                        untracked += 1;
                        dirty = true;
                    } else if !line.trim().is_empty() {
                        dirty = true;
                    }
                }
                git_dirty = Some(dirty);
                untracked_count = Some(untracked);
            } else {
                git_dirty = Some(false);
                untracked_count = Some(0);
            }

            if let Some(ls_out) = run_git_cmd(&["ls-files"], root).await {
                for line in ls_out.lines() {
                    let trim_line = line.trim();
                    if !trim_line.is_empty() {
                        files.push(PathBuf::from(trim_line));
                    }
                }
            }
        }

        if files.is_empty() {
            files = list_files_recursive(root, root);
        }

        files.sort();

        Ok(WorkspaceSnapshot {
            workspace_root: root.to_path_buf(),
            git_sha,
            git_dirty,
            untracked_count,
            content_hash: compute_content_hash(root, &files),
            captured_at: Utc::now(),
        })
    }
}

async fn run_git_cmd(args: &[&str], dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn list_files_recursive(dir: &Path, current_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(current_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == ".git" || name == "target" {
                    continue;
                }
                if name == "runs"
                    && path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        == Some(".gestalt")
                {
                    continue;
                }
                files.extend(list_files_recursive(dir, &path));
            } else if path.is_file() {
                if let Ok(rel) = path.strip_prefix(dir) {
                    files.push(rel.to_path_buf());
                }
            }
        }
    }
    files
}

fn is_binary_file(path: &Path) -> bool {
    use std::io::Read;
    if let Ok(mut file) = std::fs::File::open(path) {
        let mut buffer = [0; 1024];
        if let Ok(n) = file.read(&mut buffer) {
            if buffer[..n].contains(&0) {
                return true;
            }
        }
    }
    false
}

fn compute_content_hash(root: &Path, relative_paths: &[PathBuf]) -> String {
    let mut hasher = Sha256::new();
    for rel_path in relative_paths {
        let abs_path = root.join(rel_path);
        if abs_path.is_dir() || is_binary_file(&abs_path) {
            continue;
        }

        let path_str = rel_path.to_string_lossy();
        if path_str.contains(".gestalt/runs") || path_str.contains("target/") {
            continue;
        }

        if let Ok(bytes) = std::fs::read(&abs_path) {
            let mut file_hasher = Sha256::new();
            file_hasher.update(&bytes);
            hasher.update(path_str.as_bytes());
            hasher.update(file_hasher.finalize());
        }
    }
    format!("{:x}", hasher.finalize())
}
