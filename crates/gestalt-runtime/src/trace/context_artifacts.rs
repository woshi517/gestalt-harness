pub use crate::context::projection::{
    CompactionCheckpoint, MessageMetadataRef, ProjectionManifest,
};
use gestalt_core::{DurabilityMode, TraceError};
use std::fs;
use std::path::Path;

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
