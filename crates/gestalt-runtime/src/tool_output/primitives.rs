use gestalt_core::{
    artifact_path,
    tool::{ToolArtifact, ToolContext, ToolExecutionResult},
};
use sha2::{Digest, Sha256};

pub(super) fn compute_output_hash(
    content: &str,
    artifact: Option<&ToolArtifact>,
) -> Option<String> {
    if content.is_empty() {
        return None;
    }

    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    if let Some(artifact) = artifact {
        if let Ok(bytes) = std::fs::read(&artifact.path) {
            hasher.update(bytes);
        } else {
            hasher.update(artifact.path.as_os_str().as_encoded_bytes());
            hasher.update(artifact.mime_type.as_bytes());
            hasher.update(artifact.size_bytes.to_le_bytes());
        }
    }
    Some(format!("{:x}", hasher.finalize()))
}

pub(super) fn infer_artifact(
    tool_name: &str,
    ctx: &ToolContext,
    tool_call_id: &str,
    content: &str,
) -> Option<ToolArtifact> {
    if tool_name == "bash" && content.contains("truncated: true") {
        if let Some(artifact_dir) = ctx.artifact_dir.as_ref() {
            let path = artifact_path(artifact_dir, tool_call_id, ".txt");
            let size_bytes = std::fs::metadata(&path)
                .ok()
                .and_then(|meta| usize::try_from(meta.len()).ok())
                .unwrap_or(content.len());
            return Some(ToolArtifact {
                path,
                mime_type: "text/plain".to_string(),
                size_bytes,
            });
        }
    }

    None
}

pub(super) fn extract_exec_truncated_flag(content: &str) -> Option<bool> {
    let mut lines = content.lines();
    let _ = lines.next()?;
    let _ = lines.next()?;
    let line = lines.next()?;
    line.strip_prefix("truncated: ")?.parse().ok()
}

pub(super) fn render_truncation_notice(result: &ToolExecutionResult) -> String {
    format!(
        "[Output truncated. Original: {} bytes. Full output saved to artifact: {}]",
        result.original_bytes.unwrap_or(0),
        result.artifact.as_ref().map_or_else(
            || "unavailable".to_string(),
            |artifact| artifact.path.display().to_string(),
        )
    )
}

pub(super) fn truncate_to_bytes(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }

    let mut end = max_bytes;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    content[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::truncate_to_bytes;

    #[test]
    fn truncate_to_bytes_should_stop_on_utf8_boundary() {
        assert_eq!(truncate_to_bytes("aé", 2), "a");
    }
}
