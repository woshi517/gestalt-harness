use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};

use crate::path::validate_write_path;

use super::common::{invalid_input, parse_input, tool_schema};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WriteInput {
    pub path: String,
    pub content: String,
    #[serde(default = "super::common::default_true")]
    pub show_diff: bool,
    #[serde(default = "super::common::default_true")]
    pub create_dirs: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WriteTool;

#[async_trait::async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write full replacement content to a workspace file."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<WriteInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Medium
    }

    fn descriptor(&self) -> gestalt_core::tool_descriptor::ToolDescriptor {
        crate::builtin_descriptors::make_builtin_descriptor(
            self, false, // read_only
            false, // idempotent
            None,  // no retries
            &[],
        )
    }

    fn shape_output(&self, result: &mut gestalt_core::tool::ToolExecutionResult) {
        crate::response_shaping::shape_tool_response(self.name(), result);
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<WriteInput>(self.name(), input)?;
        let path = validate_write_path(&input.path, ctx)?;
        let parent = path
            .parent()
            .ok_or_else(|| invalid_input(self.name(), "write path has no parent"))?;
        if !parent.exists() {
            if input.create_dirs {
                std::fs::create_dir_all(parent).map_err(ToolError::ExecutionFailed)?;
            } else {
                return Err(invalid_input(
                    self.name(),
                    "parent directory does not exist",
                ));
            }
        }

        let old = if path.exists() {
            std::fs::read_to_string(&path).map_err(ToolError::ExecutionFailed)?
        } else {
            String::new()
        };
        std::fs::write(&path, input.content.as_bytes()).map_err(ToolError::ExecutionFailed)?;

        let diff = if input.show_diff {
            make_diff(&input.path, &old, &input.content)
        } else {
            String::new()
        };
        Ok(ToolOutput::Text {
            content: json!({
                "path": input.path,
                "bytes_written": input.content.len(),
                "diff": diff,
            })
            .to_string(),
        })
    }
}

fn make_diff(path: &str, old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let mut diff = format!("--- {path}\n+++ {path}\n");
    for line in old.lines() {
        diff.push('-');
        diff.push_str(line);
        diff.push('\n');
    }
    for line in new.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, temp_workspace};
    use super::WriteTool;
    use gestalt_core::{Tool, ToolError, ToolOutput};
    use serde_json::json;

    #[tokio::test]
    async fn write_should_create_parent_dirs_and_return_diff() {
        let root = temp_workspace("write");
        let output = WriteTool
            .execute(
                json!({"path": "docs/a.md", "content": "new\n", "show_diff": true}),
                &ctx(&root),
            )
            .await
            .expect("write succeeds");

        assert!(root.join("docs/a.md").exists());
        assert!(matches!(output, ToolOutput::Text { content } if content.contains("\"diff\"")));
    }

    #[tokio::test]
    async fn write_should_fail_when_parent_missing_and_create_dirs_false() {
        let root = temp_workspace("write-no-dirs");
        let result = WriteTool
            .execute(
                json!({"path": "docs/a.md", "content": "new\n", "create_dirs": false}),
                &ctx(&root),
            )
            .await;

        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }
}
