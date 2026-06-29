mod primitives;

use gestalt_core::{
    tool::{
        Tool, ToolArtifact, ToolContext, ToolExecutionResult, ToolOutput, ToolOutputMaterializer,
    },
    tool_failure::ToolErrorReport,
};

use primitives::{
    compute_output_hash, extract_exec_truncated_flag, infer_artifact, render_truncation_notice,
    truncate_to_bytes,
};

#[derive(Debug, Default, Clone, Copy)]
pub struct RuntimeToolOutputMaterializer;

pub fn materialize(
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
        _ => infer_artifact(tool.name(), ctx, tool_call_id, &full_content),
    };

    let exec_report_truncated = extract_exec_truncated_flag(&full_content);
    let should_truncate =
        exec_report_truncated.unwrap_or(full_content.len() > ctx.max_output_bytes);
    let original_bytes = if should_truncate {
        Some(full_content.len())
    } else {
        None
    };

    let content = if should_truncate && full_content.len() > ctx.max_output_bytes {
        truncate_to_bytes(&full_content, ctx.max_output_bytes)
    } else {
        full_content.clone()
    };

    let output_hash = compute_output_hash(&content, artifact.as_ref());

    let failure = if is_error {
        Some(ToolErrorReport::new(
            gestalt_core::tool_failure::ToolFailureKind::ExecutionFailed,
            content.clone(),
        ))
    } else {
        None
    };

    let mut result = ToolExecutionResult {
        content,
        is_error,
        artifact,
        truncated: should_truncate,
        original_bytes,
        output_hash,
        metadata: serde_json::Value::Null,
        failure,
        tool_name: Some(tool.name().to_string()),
    };

    shape_tool_response(tool.name(), &mut result);
    result
}

impl ToolOutputMaterializer for RuntimeToolOutputMaterializer {
    fn materialize(
        &self,
        tool: &dyn Tool,
        output: ToolOutput,
        is_error: bool,
        ctx: &ToolContext,
        tool_call_id: &str,
    ) -> ToolExecutionResult {
        crate::tool_output::materialize(tool, output, is_error, ctx, tool_call_id)
    }
}

fn shape_tool_response(tool_name: &str, result: &mut ToolExecutionResult) {
    if result.is_error {
        return;
    }

    match tool_name {
        "read" => {
            let mut prefix = String::new();
            if result.truncated {
                prefix.push_str(&render_truncation_notice(result));
                prefix.push('\n');
            }
            result.content = format!("{}{}", prefix, result.content);
        }
        "search" => {
            let mut prefix = String::new();
            if result.truncated {
                prefix.push_str("[Search results truncated due to size limits]\n");
            }
            result.content = format!("{}{}", prefix, result.content);
        }
        "bash" => {
            let mut prefix = String::new();
            prefix.push_str("[Execution successful]\n");
            if result.truncated {
                prefix.push_str("[Output truncated]\n");
            }
            result.content = format!("{}{}", prefix, result.content);
        }
        "web_fetch" => {
            let mut prefix = String::new();
            prefix.push_str("[Web content fetched successfully]\n");
            if result.truncated {
                prefix.push_str("[Output truncated]\n");
            }
            result.content = format!("{}{}", prefix, result.content);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_core::{tool::ToolArtifact, ToolExecutionResult};
    use std::path::PathBuf;

    #[test]
    fn read_output_should_include_truncation_notice() {
        let mut result = ToolExecutionResult {
            content: "body".to_string(),
            is_error: false,
            artifact: Some(ToolArtifact {
                path: PathBuf::from("/tmp/output.txt"),
                mime_type: "text/plain".to_string(),
                size_bytes: 4,
            }),
            truncated: true,
            original_bytes: Some(42),
            output_hash: None,
            metadata: serde_json::Value::Null,
            failure: None,
            tool_name: Some("read".to_string()),
        };

        shape_tool_response("read", &mut result);

        assert!(
            result
                .content
                .starts_with("[Output truncated. Original: 42 bytes. Full output saved to artifact: ")
        );
    }
}
