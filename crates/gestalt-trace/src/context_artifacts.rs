use chrono::{DateTime, Utc};
use gestalt_core::{
    context::{CheckpointRef, ClearAction, HistoryRange},
    ContextManagementPolicy, DurabilityMode, TraceError,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageMetadataRef {
    pub role: String,
    pub original_index: Option<usize>,
    pub is_tombstone: bool,
    pub is_checkpoint: bool,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectionManifest {
    pub manifest_id: String,
    pub session_id: String,
    pub run_id: String,
    pub turn_id: usize,
    pub timestamp: DateTime<Utc>,
    pub policy: ContextManagementPolicy,
    pub token_estimate: usize,
    pub stable_prefix_hash: Option<String>,
    pub checkpoint_ref: Option<CheckpointRef>,
    pub cleared_results: Vec<ClearAction>,
    pub messages_metadata: Vec<MessageMetadataRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionCheckpoint {
    pub checkpoint_id: String,
    pub history_range: HistoryRange,
    pub history_range_hash: String,
    pub policy_version: String,
    pub compactor_model: String,
    pub prompt_hash: String,
    pub created_at: DateTime<Utc>,

    pub goal: String,
    pub constraints: Vec<String>,
    pub completed_work: Vec<String>,
    pub in_progress_work: Vec<String>,
    pub blocked_items: Vec<String>,
    pub key_decisions: Vec<String>,
    pub next_steps: Vec<String>,
    pub critical_context: String,
    pub relevant_references: Vec<String>,
}

impl CompactionCheckpoint {
    /// Renders the checkpoint summary as a structured Markdown block
    pub fn render_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str(&format!(
            "### Session Checkpoint Summary (ID: {})\n\n",
            self.checkpoint_id
        ));
        md.push_str(&format!("**Goal:** {}\n\n", self.goal));

        md.push_str("**Constraints:**\n");
        for c in &self.constraints {
            md.push_str(&format!("- {}\n", c));
        }
        md.push_str("\n");

        md.push_str("**Completed Work:**\n");
        for w in &self.completed_work {
            md.push_str(&format!("- {}\n", w));
        }
        md.push_str("\n");

        md.push_str("**In Progress Work:**\n");
        for w in &self.in_progress_work {
            md.push_str(&format!("- {}\n", w));
        }
        md.push_str("\n");

        md.push_str("**Blocked Items:**\n");
        for b in &self.blocked_items {
            md.push_str(&format!("- {}\n", b));
        }
        md.push_str("\n");

        md.push_str("**Key Decisions:**\n");
        for d in &self.key_decisions {
            md.push_str(&format!("- {}\n", d));
        }
        md.push_str("\n");

        md.push_str("**Next Steps:**\n");
        for s in &self.next_steps {
            md.push_str(&format!("- {}\n", s));
        }
        md.push_str("\n");

        md.push_str(&format!(
            "**Critical Context:**\n{}\n\n",
            self.critical_context
        ));

        md.push_str("**Relevant References:**\n");
        for r in &self.relevant_references {
            md.push_str(&format!("- {}\n", r));
        }
        md.push_str("\n");

        md
    }
}

pub fn persist_manifest(
    manifest: &ProjectionManifest,
    artifacts_dir: &Path,
    durability: DurabilityMode,
) -> Result<(), TraceError> {
    if matches!(durability, DurabilityMode::Disabled) {
        return Ok(());
    }

    if !artifacts_dir.exists() {
        fs::create_dir_all(artifacts_dir).map_err(TraceError::WriteFailed)?;
    }

    let file_name = format!("projection_manifest_{}.json", manifest.manifest_id);
    let file_path = artifacts_dir.join(file_name);

    let content = serde_json::to_string_pretty(manifest)
        .map_err(|err| TraceError::WriteFailed(std::io::Error::other(err)))?;

    match fs::write(file_path, content) {
        Ok(_) => Ok(()),
        Err(err) => {
            if matches!(durability, DurabilityMode::Required) {
                Err(TraceError::WriteFailed(err))
            } else {
                // Warning logged or best effort
                eprintln!("Warning: Failed to persist projection manifest: {}", err);
                Ok(())
            }
        }
    }
}

pub fn persist_checkpoint(
    checkpoint: &CompactionCheckpoint,
    artifacts_dir: &Path,
    durability: DurabilityMode,
) -> Result<(), TraceError> {
    if matches!(durability, DurabilityMode::Disabled) {
        return Ok(());
    }

    if !artifacts_dir.exists() {
        fs::create_dir_all(artifacts_dir).map_err(TraceError::WriteFailed)?;
    }

    let file_name = format!("checkpoint_{}.json", checkpoint.checkpoint_id);
    let file_path = artifacts_dir.join(file_name);

    let content = serde_json::to_string_pretty(checkpoint)
        .map_err(|err| TraceError::WriteFailed(std::io::Error::other(err)))?;

    match fs::write(file_path, content) {
        Ok(_) => Ok(()),
        Err(err) => {
            if matches!(durability, DurabilityMode::Required) {
                Err(TraceError::WriteFailed(err))
            } else {
                eprintln!("Warning: Failed to persist compaction checkpoint: {}", err);
                Ok(())
            }
        }
    }
}

pub fn load_manifest(
    manifest_id: &str,
    artifacts_dir: &Path,
) -> Result<ProjectionManifest, TraceError> {
    let file_name = format!("projection_manifest_{}.json", manifest_id);
    let file_path = artifacts_dir.join(file_name);

    let content = fs::read_to_string(&file_path).map_err(|_| TraceError::ReadFailed {
        reason: format!("manifest not found: {}", file_path.display()),
    })?;

    serde_json::from_str(&content).map_err(|err| TraceError::ReadFailed {
        reason: format!("failed to parse manifest: {}", err),
    })
}

pub fn load_checkpoint(
    checkpoint_id: &str,
    artifacts_dir: &Path,
) -> Result<CompactionCheckpoint, TraceError> {
    let file_name = format!("checkpoint_{}.json", checkpoint_id);
    let file_path = artifacts_dir.join(file_name);

    let content = fs::read_to_string(&file_path).map_err(|_| TraceError::ReadFailed {
        reason: format!("checkpoint not found: {}", file_path.display()),
    })?;

    serde_json::from_str(&content).map_err(|err| TraceError::ReadFailed {
        reason: format!("failed to parse checkpoint: {}", err),
    })
}
