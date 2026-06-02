use serde::{Deserialize, Serialize};
use serde_json::Value;

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};

use crate::path::validate_existing_path;

use super::common::{
    decode_text, invalid_input, limit_tokens, parse_input, tool_schema, DEFAULT_MAX_TOKENS,
};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReadInput {
    pub path: String,
    #[serde(default)]
    pub start_line: Option<usize>,
    #[serde(default)]
    pub end_line: Option<usize>,
    #[serde(default)]
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReadTool;

#[async_trait::async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a workspace file with optional line-range and output limits."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<ReadInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Low
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<ReadInput>(self.name(), input)?;
        let path = validate_existing_path(&input.path, ctx)?;
        let bytes = std::fs::read(&path).map_err(ToolError::ExecutionFailed)?;
        let content = decode_text(self.name(), &bytes)?;
        let selected = select_line_range(&content, input.start_line, input.end_line)?;
        let output = limit_tokens(&selected, input.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS));
        Ok(ToolOutput::Text { content: output })
    }
}

fn select_line_range(
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, ToolError> {
    let start = start_line.unwrap_or(1);
    let end = end_line.unwrap_or(usize::MAX);
    if start == 0 || end < start {
        return Err(invalid_input("read", "invalid line range"));
    }

    let selected = content
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_no = index.saturating_add(1);
            (line_no >= start && line_no <= end).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, temp_workspace};
    use super::ReadTool;
    use gestalt_core::{Tool, ToolError, ToolOutput};
    use serde_json::json;
    use std::fs;

    #[tokio::test]
    async fn read_should_honor_line_ranges() {
        let root = temp_workspace("read-range");
        fs::write(root.join("file.txt"), "one\ntwo\nthree\n").expect("write fixture");

        let output = ReadTool
            .execute(
                json!({"path": "file.txt", "start_line": 2, "end_line": 2}),
                &ctx(&root),
            )
            .await
            .expect("read succeeds");

        assert_eq!(
            output,
            ToolOutput::Text {
                content: "two".to_string()
            }
        );
    }

    #[tokio::test]
    async fn read_should_reject_path_traversal() {
        let root = temp_workspace("read-traversal");
        let result = ReadTool
            .execute(json!({"path": "../outside.txt"}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::ExecutionFailed(_))));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_should_reject_symlink_escape() {
        let root = temp_workspace("read-symlink");
        let outside = temp_workspace("outside");
        fs::write(outside.join("secret.txt"), "secret").expect("write outside");
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt"))
            .expect("create symlink");

        let result = ReadTool
            .execute(json!({"path": "link.txt"}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::PathNotAllowed(_))));
    }
}
