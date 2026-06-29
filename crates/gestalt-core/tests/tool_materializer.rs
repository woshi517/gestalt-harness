use gestalt_core::{
    artifact_path,
    tool::{
        Tool, ToolArtifact, ToolContext, ToolExecutionResult, ToolOutput, ToolOutputMaterializer,
    },
    tool_failure::{ToolErrorReport, ToolFailureKind},
};
use sha2::{Digest, Sha256};

#[derive(Debug, Default, Clone, Copy)]
pub struct TestToolOutputMaterializer;

impl ToolOutputMaterializer for TestToolOutputMaterializer {
    fn materialize(
        &self,
        tool: &dyn Tool,
        output: ToolOutput,
        is_error: bool,
        ctx: &ToolContext,
        tool_call_id: &str,
    ) -> ToolExecutionResult {
        let full_content = match &output {
            ToolOutput::Text { content } => content.clone(),
            ToolOutput::Json { value } => value.to_string(),
            ToolOutput::Artifact { path, .. } => format!("artifact saved: {}", path.display()),
        };

        let artifact = match output {
            ToolOutput::Artifact {
                path,
                mime_type,
                size_bytes,
            } => Some(ToolArtifact {
                path,
                mime_type,
                size_bytes,
            }),
            _ => None,
        };

        let truncated = extract_exec_truncated_flag(&full_content)
            .unwrap_or(full_content.len() > ctx.max_output_bytes);
        let original_bytes = if truncated {
            Some(full_content.len())
        } else {
            None
        };
        let content = if truncated && full_content.len() > ctx.max_output_bytes {
            truncate_to_bytes(&full_content, ctx.max_output_bytes)
        } else {
            full_content.clone()
        };
        let output_hash = if content.is_empty() {
            None
        } else {
            let mut hasher = Sha256::new();
            hasher.update(content.as_bytes());
            Some(format!("{:x}", hasher.finalize()))
        };
        let failure = if is_error {
            Some(ToolErrorReport::new(
                ToolFailureKind::ExecutionFailed,
                content.clone(),
            ))
        } else {
            None
        };

        ToolExecutionResult {
            content,
            is_error,
            artifact: artifact
                .or_else(|| infer_artifact(tool.name(), ctx, tool_call_id, &full_content)),
            truncated,
            original_bytes,
            output_hash,
            metadata: serde_json::Value::Null,
            failure,
            tool_name: Some(tool.name().to_string()),
        }
    }
}

fn infer_artifact(
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

fn extract_exec_truncated_flag(content: &str) -> Option<bool> {
    let mut lines = content.lines();
    let _ = lines.next()?;
    let _ = lines.next()?;
    let line = lines.next()?;
    line.strip_prefix("truncated: ")?.parse().ok()
}

fn truncate_to_bytes(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }

    let mut end = max_bytes;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    content[..end].to_string()
}
