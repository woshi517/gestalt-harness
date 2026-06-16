use serde::{Deserialize, Serialize};
use serde_json::Value;

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};

use crate::path::validate_existing_path;

use super::common::{invalid_input, parse_input, tool_schema};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PatchInput {
    /// The path to the file to patch.
    pub path: String,
    /// The unified diff patch to apply.
    pub patch: String,
    /// Verify the file's current SHA-256 matches this hash before patching.
    #[serde(default)]
    pub expected_hash: Option<String>,
    /// Validate inputs and compute the patched results without writing to the file. Defaults to false.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PatchTool;

#[async_trait::async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff patch to a workspace file."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<PatchInput>(self.name(), self.description())
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
        let input = parse_input::<PatchInput>(self.name(), input)?;
        let path = validate_existing_path(&input.path, ctx)?;
        let old = std::fs::read_to_string(&path).map_err(ToolError::ExecutionFailed)?;

        super::common::check_expected_hash(self.name(), &old, input.expected_hash.as_deref())?;

        let patched = apply_unified_patch(&old, &input.patch)?;

        if !input.dry_run {
            super::common::atomic_write(&path, &patched).map_err(ToolError::ExecutionFailed)?;
        }

        Ok(ToolOutput::Text {
            content: format!(
                "patch applied: {}{}",
                input.path,
                if input.dry_run { " (dry_run)" } else { "" }
            ),
        })
    }
}

fn apply_unified_patch(original: &str, patch: &str) -> Result<String, ToolError> {
    let mut lines = original.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let patch_lines = patch.lines().collect::<Vec<_>>();
    let mut index = 0;

    while index < patch_lines.len() {
        let line = patch_lines[index];
        if !line.starts_with("@@") {
            index += 1;
            continue;
        }

        let old_start = parse_hunk_start(line)?;
        let mut cursor = old_start.saturating_sub(1);
        index += 1;

        while index < patch_lines.len() && !patch_lines[index].starts_with("@@") {
            let patch_line = patch_lines[index];
            if patch_line.starts_with("---") || patch_line.starts_with("+++") {
                index += 1;
                continue;
            }
            let (prefix, value) = patch_line.split_at(1);
            match prefix {
                " " => {
                    ensure_line_matches(&lines, cursor, value)?;
                    cursor = cursor.saturating_add(1);
                }
                "-" => {
                    ensure_line_matches(&lines, cursor, value)?;
                    lines.remove(cursor);
                }
                "+" => {
                    lines.insert(cursor, value.to_string());
                    cursor = cursor.saturating_add(1);
                }
                _ => return Err(invalid_input("patch", "invalid unified diff line")),
            }
            index += 1;
        }
    }

    let mut output = lines.join("\n");
    if original.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}

fn parse_hunk_start(header: &str) -> Result<usize, ToolError> {
    let start = header
        .split_whitespace()
        .find(|part| part.starts_with('-'))
        .ok_or_else(|| invalid_input("patch", "missing hunk header"))?;
    let number = start
        .trim_start_matches('-')
        .split(',')
        .next()
        .unwrap_or_default();
    number
        .parse::<usize>()
        .map_err(|err| invalid_input("patch", err.to_string()))
}

fn ensure_line_matches(lines: &[String], cursor: usize, expected: &str) -> Result<(), ToolError> {
    if lines.get(cursor).is_some_and(|line| line == expected) {
        return Ok(());
    }
    Err(invalid_input("patch", "patch context mismatch"))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, temp_workspace};
    use super::PatchTool;
    use gestalt_core::{Tool, ToolError, ToolOutput};
    use serde_json::json;
    use std::fs;

    #[tokio::test]
    async fn patch_should_apply_unified_diff() {
        let root = temp_workspace("patch");
        fs::write(root.join("a.txt"), "one\ntwo\nthree\n").expect("write fixture");
        let patch = "--- a.txt\n+++ a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three";

        PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await
            .expect("patch succeeds");

        assert_eq!(
            fs::read_to_string(root.join("a.txt")).expect("read patched"),
            "one\nTWO\nthree\n"
        );
    }

    #[tokio::test]
    async fn patch_should_fail_on_context_mismatch() {
        let root = temp_workspace("patch-fail");
        fs::write(root.join("a.txt"), "one\ntwo\n").expect("write fixture");
        let patch = "--- a.txt\n+++ a.txt\n@@ -1,2 +1,2 @@\n missing\n-two\n+TWO";

        let result = PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn patch_with_matching_expected_hash_should_succeed() {
        let root = temp_workspace("patch-matching-hash");
        let path = root.join("a.txt");
        fs::write(&path, "one\ntwo\nthree\n").expect("write fixture");
        let hash = super::super::common::calculate_sha256("one\ntwo\nthree\n");
        let patch = "--- a.txt\n+++ a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three";

        PatchTool
            .execute(
                json!({
                    "path": "a.txt",
                    "patch": patch,
                    "expected_hash": hash,
                }),
                &ctx(&root),
            )
            .await
            .expect("patch succeeds");

        assert_eq!(
            fs::read_to_string(&path).expect("read patched"),
            "one\nTWO\nthree\n"
        );
    }

    #[tokio::test]
    async fn patch_with_mismatched_expected_hash_should_fail() {
        let root = temp_workspace("patch-mismatched-hash");
        let path = root.join("a.txt");
        fs::write(&path, "one\ntwo\nthree\n").expect("write fixture");
        let patch = "--- a.txt\n+++ a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three";

        let result = PatchTool
            .execute(
                json!({
                    "path": "a.txt",
                    "patch": patch,
                    "expected_hash": "wronghash",
                }),
                &ctx(&root),
            )
            .await;

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&path).expect("read patched"),
            "one\ntwo\nthree\n"
        );
    }

    #[tokio::test]
    async fn patch_dry_run_should_not_modify_file() {
        let root = temp_workspace("patch-dry-run");
        let path = root.join("a.txt");
        fs::write(&path, "one\ntwo\nthree\n").expect("write fixture");
        let patch = "--- a.txt\n+++ a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three";

        let output = PatchTool
            .execute(
                json!({
                    "path": "a.txt",
                    "patch": patch,
                    "dry_run": true,
                }),
                &ctx(&root),
            )
            .await
            .expect("dry run succeeds");

        assert_eq!(
            fs::read_to_string(&path).expect("read patched"),
            "one\ntwo\nthree\n"
        );

        match output {
            ToolOutput::Text { content } => {
                assert!(content.contains("dry_run"));
                assert!(content.contains("a.txt"));
            }
            _ => panic!("Expected text output"),
        }
    }
}
