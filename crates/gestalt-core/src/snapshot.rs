use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub workspace_root: PathBuf,
    pub git_sha: Option<String>,
    pub git_dirty: Option<bool>,
    pub untracked_count: Option<usize>,
    pub content_hash: String,
    pub captured_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait WorkspaceSnapshotter: Send + Sync {
    async fn capture(&self, root: &Path) -> Result<WorkspaceSnapshot, crate::error::HarnessError>;
}
