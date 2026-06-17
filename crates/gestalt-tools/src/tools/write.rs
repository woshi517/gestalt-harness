use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};

use crate::path::validate_write_path;

use super::common::{invalid_input, parse_input, tool_schema};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WriteInput {
    /// The path to the file to write.
    pub path: String,
    /// The content to write to the file.
    pub content: String,
    /// Return a change preview diff. Defaults to true.
    #[serde(default = "super::common::default_true")]
    pub show_diff: bool,
    /// Automatically create parent directories if they do not exist. Defaults to true.
    #[serde(default = "super::common::default_true")]
    pub create_dirs: bool,
    /// Verify the file's current SHA-256 matches this hash before writing.
    #[serde(default)]
    pub expected_hash: Option<String>,
    /// Validate inputs and compute the diff preview without making any actual changes. Defaults to false.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WriteTool;

#[async_trait::async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write full replacement content to a workspace file, returning a bounded change preview."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<WriteInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Medium
    }

    fn descriptor(&self) -> gestalt_core::tool_descriptor::ToolDescriptor {
        crate::builtin_descriptors::make_builtin_descriptor(
            self,
            false, // read_only
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
                if !input.dry_run {
                    std::fs::create_dir_all(parent).map_err(ToolError::ExecutionFailed)?;
                }
            } else {
                return Err(invalid_input(
                    self.name(),
                    "parent directory does not exist",
                ));
            }
        }

        let old = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => {
                    return Err(invalid_input(
                        self.name(),
                        "existing file is not valid UTF-8 text",
                    ));
                }
            }
        } else {
            String::new()
        };

        super::common::check_expected_hash(self.name(), &old, input.expected_hash.as_deref())?;

        let status = if !path.exists() {
            "created"
        } else if old == input.content {
            "unchanged"
        } else {
            "updated"
        };

        let (diff_str, diff_truncated, lines_added, lines_removed) =
            make_diff(&input.path, &old, &input.content);

        let diff = if input.show_diff {
            diff_str
        } else {
            String::new()
        };

        let bytes_written = if input.dry_run || status == "unchanged" {
            0
        } else {
            input.content.len()
        };

        if !input.dry_run && status != "unchanged" {
            super::common::atomic_write(&path, &input.content)
                .map_err(ToolError::ExecutionFailed)?;
        }

        Ok(ToolOutput::Text {
            content: json!({
                "path": input.path,
                "bytes_written": bytes_written,
                "status": status,
                "diff": diff,
                "diff_truncated": diff_truncated,
                "lines_added": lines_added,
                "lines_removed": lines_removed,
                "dry_run": input.dry_run,
            })
            .to_string(),
        })
    }
}

fn make_diff(path: &str, old: &str, new: &str) -> (String, bool, usize, usize) {
    if old == new {
        return (String::new(), false, 0, 0);
    }

    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(old, new);
    let mut lines_added = 0;
    let mut lines_removed = 0;

    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => lines_removed += 1,
            ChangeTag::Insert => lines_added += 1,
            ChangeTag::Equal => {}
        }
    }

    let raw_diff = diff
        .unified_diff()
        .context_radius(3)
        .header(path, path)
        .to_string();
    let mut diff_str = String::new();
    let mut diff_truncated = false;
    const MAX_PREVIEW_LINES: usize = 200;
    const MAX_PREVIEW_BYTES: usize = 16384;

    for (printed_lines, line) in raw_diff.lines().enumerate() {
        if printed_lines >= MAX_PREVIEW_LINES
            || diff_str.len() + line.len() + 1 >= MAX_PREVIEW_BYTES
        {
            diff_truncated = true;
            break;
        }
        diff_str.push_str(line);
        diff_str.push('\n');
    }

    if diff_truncated {
        diff_str.push_str("\n... [Diff truncated] ...\n");
    }

    (diff_str, diff_truncated, lines_added, lines_removed)
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

    #[tokio::test]
    async fn write_with_matching_expected_hash_should_succeed() {
        let root = temp_workspace("write-matching-hash");
        let path = root.join("a.txt");
        std::fs::write(&path, "original\n").expect("setup");

        let hash = super::super::common::calculate_sha256("original\n");

        let _output = WriteTool
            .execute(
                json!({
                    "path": "a.txt",
                    "content": "new\n",
                    "expected_hash": hash,
                }),
                &ctx(&root),
            )
            .await
            .expect("matching hash succeeds");

        assert_eq!(std::fs::read_to_string(&path).expect("read"), "new\n");
    }

    #[tokio::test]
    async fn write_with_mismatched_expected_hash_should_fail() {
        let root = temp_workspace("write-mismatched-hash");
        let path = root.join("a.txt");
        std::fs::write(&path, "original\n").expect("setup");

        let result = WriteTool
            .execute(
                json!({
                    "path": "a.txt",
                    "content": "new\n",
                    "expected_hash": "wronghash",
                }),
                &ctx(&root),
            )
            .await;

        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original\n");
    }

    #[tokio::test]
    async fn write_dry_run_should_not_modify_file() {
        let root = temp_workspace("write-dry-run");
        let path = root.join("a.txt");
        std::fs::write(&path, "original\n").expect("setup");

        let output = WriteTool
            .execute(
                json!({
                    "path": "a.txt",
                    "content": "new\n",
                    "dry_run": true,
                }),
                &ctx(&root),
            )
            .await
            .expect("dry run succeeds");

        assert_eq!(std::fs::read_to_string(&path).expect("read"), "original\n");

        match output {
            ToolOutput::Text { content } => {
                let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
                assert_eq!(parsed["path"], "a.txt");
                assert!(parsed["diff"].as_str().unwrap().contains("+new"));
            }
            _ => panic!("Expected text response"),
        }
    }

    #[tokio::test]
    async fn write_should_return_bounded_minimal_diff() {
        let root = temp_workspace("write-minimal-diff");
        let path = root.join("a.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").expect("setup");

        let output = WriteTool
            .execute(
                json!({
                    "path": "a.txt",
                    "content": "one\nTWO\nthree\n",
                    "show_diff": true,
                }),
                &ctx(&root),
            )
            .await
            .expect("write succeeds");

        match output {
            ToolOutput::Text { content } => {
                let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
                assert_eq!(parsed["status"], "updated");
                assert_eq!(parsed["lines_added"], 1);
                assert_eq!(parsed["lines_removed"], 1);
                let diff = parsed["diff"].as_str().unwrap();
                assert!(diff.contains("@@ -1,3 +1,3 @@"));
                assert!(diff.contains("-two"));
                assert!(diff.contains("+TWO"));
            }
            _ => panic!("Expected text response"),
        }
    }

    #[tokio::test]
    async fn write_unchanged_should_return_empty_diff_and_zero_bytes_written() {
        let root = temp_workspace("write-unchanged");
        let path = root.join("a.txt");
        std::fs::write(&path, "same\n").expect("setup");

        let output = WriteTool
            .execute(
                json!({
                    "path": "a.txt",
                    "content": "same\n",
                }),
                &ctx(&root),
            )
            .await
            .expect("write succeeds");

        match output {
            ToolOutput::Text { content } => {
                let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
                assert_eq!(parsed["status"], "unchanged");
                assert_eq!(parsed["bytes_written"], 0);
                assert_eq!(parsed["diff"], "");
            }
            _ => panic!("Expected text response"),
        }
    }

    #[tokio::test]
    async fn write_new_file_should_show_only_additions() {
        let root = temp_workspace("write-new-file");
        let output = WriteTool
            .execute(
                json!({
                    "path": "a.txt",
                    "content": "hello world\n",
                }),
                &ctx(&root),
            )
            .await
            .expect("write succeeds");

        match output {
            ToolOutput::Text { content } => {
                let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
                assert_eq!(parsed["status"], "created");
                assert_eq!(parsed["lines_removed"], 0);
                assert_eq!(parsed["lines_added"], 1);
            }
            _ => panic!("Expected text response"),
        }
    }

    #[tokio::test]
    async fn write_large_content_should_truncate_diff() {
        let root = temp_workspace("write-truncate");
        let path = root.join("a.txt");
        std::fs::write(&path, "old\n").expect("setup");

        let mut large_content = String::new();
        for i in 0..300 {
            large_content.push_str(&format!("line {}\n", i));
        }

        let output = WriteTool
            .execute(
                json!({
                    "path": "a.txt",
                    "content": large_content,
                }),
                &ctx(&root),
            )
            .await
            .expect("write succeeds");

        match output {
            ToolOutput::Text { content } => {
                let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
                assert_eq!(parsed["diff_truncated"], true);
                assert!(parsed["diff"]
                    .as_str()
                    .unwrap()
                    .contains("[Diff truncated]"));
            }
            _ => panic!("Expected text response"),
        }
    }

    #[tokio::test]
    async fn write_should_honor_show_diff_false() {
        let root = temp_workspace("write-show-diff-false");
        let path = root.join("a.txt");
        std::fs::write(&path, "old\n").expect("setup");

        let output = WriteTool
            .execute(
                json!({
                    "path": "a.txt",
                    "content": "new\n",
                    "show_diff": false,
                }),
                &ctx(&root),
            )
            .await
            .expect("write succeeds");

        match output {
            ToolOutput::Text { content } => {
                let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
                assert_eq!(parsed["diff"], "");
                assert_eq!(parsed["lines_added"], 1);
                assert_eq!(parsed["lines_removed"], 1);
            }
            _ => panic!("Expected text response"),
        }
    }
}
