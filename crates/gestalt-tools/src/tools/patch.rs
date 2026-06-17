use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};

use crate::path::{validate_existing_path, validate_write_path};

use super::common::{invalid_input, parse_input, tool_schema};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PatchInput {
    /// The primary path associated with the patch operation.
    pub path: String,
    /// The high-level patch document containing operations to apply.
    pub patch: String,
    /// Verify the primary file's current SHA-256 matches this hash before patching.
    #[serde(default)]
    pub expected_hash: Option<String>,
    /// Validate inputs and compute the patched results without writing to the files. Defaults to false.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PatchTool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOperation {
    Add { path: String, content: String },
    Update { path: String, replacements: Vec<SearchReplace> },
    Delete { path: String },
    Move { from: String, to: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchReplace {
    pub search: String,
    pub replace: String,
}

#[async_trait::async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn description(&self) -> &str {
        "Apply a high-level patch document to workspace files, expressing operations like Add, Update, Delete, or Move directly."
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
        
        let operations = parse_patch(self.name(), &input.patch)?;
        if operations.is_empty() {
            return Err(invalid_input(self.name(), "patch document contains no valid operations"));
        }

        // Validate the expected_hash of the primary target file if provided
        if let Some(ref expected_hash) = input.expected_hash {
            let primary_path = validate_existing_path(&input.path, ctx)?;
            let primary_content = std::fs::read_to_string(&primary_path).map_err(ToolError::ExecutionFailed)?;
            super::common::check_expected_hash(self.name(), &primary_content, Some(expected_hash))?;
        }

        // In-memory representation of proposed changes.
        // key: absolute canonicalized/resolved path, value: (display_path, content)
        let mut writes: std::collections::HashMap<std::path::PathBuf, (String, String)> = std::collections::HashMap::new();
        let mut deletes: std::collections::HashMap<std::path::PathBuf, String> = std::collections::HashMap::new();

        for op in &operations {
            match op {
                PatchOperation::Add { path, content } => {
                    let abs_path = validate_write_path(path, ctx)?;
                    let exists_and_not_deleted = abs_path.exists() && !deletes.contains_key(&abs_path);
                    if exists_and_not_deleted || writes.contains_key(&abs_path) {
                        return Err(invalid_input(self.name(), format!("destination file already exists: {}", path)));
                    }
                    writes.insert(abs_path, (path.clone(), content.clone()));
                }
                PatchOperation::Update { path, replacements } => {
                    let abs_path = validate_write_path(path, ctx)?;
                    if deletes.contains_key(&abs_path) {
                        return Err(invalid_input(self.name(), format!("cannot update deleted path: {}", path)));
                    }

                    let current_content = if let Some((_, content)) = writes.get(&abs_path) {
                        content.clone()
                    } else {
                        if !abs_path.exists() {
                            return Err(invalid_input(self.name(), format!("cannot update non-existent file: {}", path)));
                        }
                        std::fs::read_to_string(&abs_path).map_err(ToolError::ExecutionFailed)?
                    };

                    let new_content = apply_replacements(self.name(), &current_content, replacements)?;
                    writes.insert(abs_path, (path.clone(), new_content));
                }
                PatchOperation::Delete { path } => {
                    let abs_path = validate_write_path(path, ctx)?;
                    if deletes.contains_key(&abs_path) {
                        return Err(invalid_input(self.name(), format!("cannot delete already deleted path: {}", path)));
                    }

                    let was_written = writes.remove(&abs_path).is_some();
                    if !was_written && !abs_path.exists() {
                        return Err(invalid_input(self.name(), format!("cannot delete non-existent file: {}", path)));
                    }
                    deletes.insert(abs_path, path.clone());
                }
                PatchOperation::Move { from, to } => {
                    let abs_from = validate_write_path(from, ctx)?;
                    let abs_to = validate_write_path(to, ctx)?;

                    let exists_and_not_deleted = abs_to.exists() && !deletes.contains_key(&abs_to);
                    if exists_and_not_deleted || writes.contains_key(&abs_to) {
                        return Err(invalid_input(self.name(), format!("destination file already exists: {}", to)));
                    }
                    if deletes.contains_key(&abs_from) {
                        return Err(invalid_input(self.name(), format!("cannot move deleted path: {}", from)));
                    }

                    let content = if let Some((_, c)) = writes.remove(&abs_from) {
                        c
                    } else {
                        if !abs_from.exists() {
                            return Err(invalid_input(self.name(), format!("source file does not exist: {}", from)));
                        }
                        std::fs::read_to_string(&abs_from).map_err(ToolError::ExecutionFailed)?
                    };

                    deletes.insert(abs_from, from.clone());
                    writes.insert(abs_to, (to.clone(), content));
                }
            }
        }

        // Apply mutations if not a dry run.
        if !input.dry_run {
            let mut temp_writes = Vec::new();
            
            struct TempCleanup {
                paths: Vec<(std::path::PathBuf, std::path::PathBuf)>,
            }
            impl Drop for TempCleanup {
                fn drop(&mut self) {
                    for (temp_path, _) in &self.paths {
                        let _ = std::fs::remove_file(temp_path);
                    }
                }
            }

            let mut cleanup = TempCleanup { paths: Vec::new() };

            for (abs_path, (_, content)) in &writes {
                if let Some(parent) = abs_path.parent() {
                    if !parent.exists() {
                        std::fs::create_dir_all(parent).map_err(ToolError::ExecutionFailed)?;
                    }
                }
                let parent = abs_path.parent().ok_or_else(|| {
                    ToolError::ExecutionFailed(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "path has no parent",
                    ))
                })?;

                use std::time::{SystemTime, UNIX_EPOCH};
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                let temp_name = format!(
                    ".{}.{}.tmp",
                    abs_path.file_name()
                        .map_or_else(|| "temp".to_string(), |n| n.to_string_lossy().to_string()),
                    now
                );
                let temp_path = parent.join(temp_name);
                std::fs::write(&temp_path, content.as_bytes()).map_err(ToolError::ExecutionFailed)?;
                cleanup.paths.push((temp_path, abs_path.clone()));
            }

            // All writes succeeded, transfer temp paths and disable automatic cleanup
            temp_writes = std::mem::take(&mut cleanup.paths);
            std::mem::forget(cleanup);

            // Process deletions first.
            for abs_path in deletes.keys() {
                if abs_path.exists() {
                    std::fs::remove_file(abs_path).map_err(ToolError::ExecutionFailed)?;
                }
            }
            // Process renames next.
            for (temp_path, abs_path) in temp_writes {
                std::fs::rename(temp_path, abs_path).map_err(ToolError::ExecutionFailed)?;
            }
        }

        let mut operations_summary = serde_json::json!([]);
        for op in &operations {
            let val = match op {
                PatchOperation::Add { path, .. } => serde_json::json!({ "op": "add", "path": path }),
                PatchOperation::Update { path, .. } => serde_json::json!({ "op": "update", "path": path }),
                PatchOperation::Delete { path } => serde_json::json!({ "op": "delete", "path": path }),
                PatchOperation::Move { from, to } => serde_json::json!({ "op": "move", "from": from, "to": to }),
            };
            operations_summary.as_array_mut().unwrap().push(val);
        }

        Ok(ToolOutput::Text {
            content: serde_json::json!({
                "path": input.path,
                "patch_applied": !input.dry_run,
                "dry_run": input.dry_run,
                "operations": operations_summary,
            })
            .to_string(),
        })
    }
}

pub fn parse_patch(tool_name: &str, patch_str: &str) -> Result<Vec<PatchOperation>, ToolError> {
    let mut operations = Vec::new();
    let lines: Vec<&str> = patch_str.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() {
            i += 1;
            continue;
        }

        if let Some(rest) = line.strip_prefix("<<< ADD FILE:") {
            let path = rest.trim_end_matches(">>>").trim().trim_matches('"').to_string();
            if path.is_empty() {
                return Err(invalid_input(tool_name, "empty path in ADD FILE"));
            }
            i += 1;
            let mut content_lines = Vec::new();
            let mut found_end = false;
            while i < lines.len() {
                let inner_line = lines[i];
                let trimmed_inner = inner_line.trim();
                if trimmed_inner.starts_with("<<< END ADD FILE") {
                    found_end = true;
                    i += 1;
                    break;
                }
                content_lines.push(inner_line);
                i += 1;
            }
            if !found_end {
                return Err(invalid_input(tool_name, format!("missing END ADD FILE for {}", path)));
            }
            let content = content_lines.join("\n");
            operations.push(PatchOperation::Add { path, content });
        } else if let Some(rest) = line.strip_prefix("<<< UPDATE FILE:") {
            let path = rest.trim_end_matches(">>>").trim().trim_matches('"').to_string();
            if path.is_empty() {
                return Err(invalid_input(tool_name, "empty path in UPDATE FILE"));
            }
            i += 1;
            let mut replacements = Vec::new();
            let mut found_end = false;
            while i < lines.len() {
                let inner_line = lines[i];
                let trimmed_inner = inner_line.trim();
                if trimmed_inner.starts_with("<<< END UPDATE FILE") {
                    found_end = true;
                    i += 1;
                    break;
                }
                if trimmed_inner == "<<<<<<< SEARCH" {
                    i += 1;
                    let mut search_lines = Vec::new();
                    let mut found_sep = false;
                    while i < lines.len() {
                        let search_line = lines[i];
                        let trimmed_search = search_line.trim();
                        if trimmed_search == "=======" {
                            found_sep = true;
                            i += 1;
                            break;
                        }
                        search_lines.push(search_line);
                        i += 1;
                    }
                    if !found_sep {
                        return Err(invalid_input(tool_name, "missing ======= separator in SEARCH block"));
                    }
                    let mut replace_lines = Vec::new();
                    let mut found_close = false;
                    while i < lines.len() {
                        let replace_line = lines[i];
                        let trimmed_replace = replace_line.trim();
                        if trimmed_replace == ">>>>>>>" {
                            found_close = true;
                            i += 1;
                            break;
                        }
                        replace_lines.push(replace_line);
                        i += 1;
                    }
                    if !found_close {
                        return Err(invalid_input(tool_name, "missing >>>>>>> terminator in SEARCH block"));
                    }
                    replacements.push(SearchReplace {
                        search: search_lines.join("\n"),
                        replace: replace_lines.join("\n"),
                    });
                } else {
                    if !trimmed_inner.is_empty() && !trimmed_inner.starts_with('#') {
                        return Err(invalid_input(tool_name, format!("unexpected content inside UPDATE FILE block: {}", trimmed_inner)));
                    }
                    i += 1;
                }
            }
            if !found_end {
                return Err(invalid_input(tool_name, format!("missing END UPDATE FILE for {}", path)));
            }
            operations.push(PatchOperation::Update { path, replacements });
        } else if let Some(rest) = line.strip_prefix("<<< DELETE FILE:") {
            let path = rest.trim_end_matches(">>>").trim().trim_matches('"').to_string();
            if path.is_empty() {
                return Err(invalid_input(tool_name, "empty path in DELETE FILE"));
            }
            i += 1;
            let mut found_end = false;
            while i < lines.len() {
                let inner_line = lines[i];
                let trimmed_inner = inner_line.trim();
                if trimmed_inner.starts_with("<<< END DELETE FILE") {
                    found_end = true;
                    i += 1;
                    break;
                }
                i += 1;
            }
            if !found_end {
                return Err(invalid_input(tool_name, format!("missing END DELETE FILE for {}", path)));
            }
            operations.push(PatchOperation::Delete { path });
        } else if let Some(rest) = line.strip_prefix("<<< MOVE FILE:") {
            let from = rest.trim_end_matches(">>>").trim().trim_matches('"').to_string();
            if from.is_empty() {
                return Err(invalid_input(tool_name, "empty from-path in MOVE FILE"));
            }
            i += 1;
            let mut to = None;
            let mut found_end = false;
            while i < lines.len() {
                let inner_line = lines[i];
                let trimmed_inner = inner_line.trim();
                if trimmed_inner.starts_with("<<< END MOVE FILE") {
                    found_end = true;
                    i += 1;
                    break;
                }
                if let Some(to_rest) = trimmed_inner.strip_prefix("<<< TO:") {
                    to = Some(to_rest.trim_end_matches(">>>").trim().trim_matches('"').to_string());
                }
                i += 1;
            }
            if !found_end {
                return Err(invalid_input(tool_name, format!("missing END MOVE FILE for {}", from)));
            }
            let to = to.ok_or_else(|| invalid_input(tool_name, format!("missing TO target in MOVE FILE for {}", from)))?;
            if to.is_empty() {
                return Err(invalid_input(tool_name, format!("empty TO target in MOVE FILE for {}", from)));
            }
            operations.push(PatchOperation::Move { from, to });
        } else {
            if line.contains("<<<") {
                return Err(invalid_input(tool_name, format!("malformed operation line: {}", line)));
            }
            i += 1;
        }
    }

    Ok(operations)
}

fn apply_replacements(
    tool_name: &str,
    content: &str,
    replacements: &[SearchReplace],
) -> Result<String, ToolError> {
    let mut current = content.to_string();
    for rep in replacements {
        let occurrences = current.matches(&rep.search).count();
        if occurrences == 0 {
            return Err(invalid_input(
                tool_name,
                format!("search block not found:\n{}", rep.search),
            ));
        } else if occurrences > 1 {
            return Err(invalid_input(
                tool_name,
                format!(
                    "search block is ambiguous (found {} times):\n{}",
                    occurrences, rep.search
                ),
            ));
        }
        current = current.replace(&rep.search, &rep.replace);
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, temp_workspace};
    use super::{PatchOperation, PatchTool, SearchReplace};
    use gestalt_core::{Tool, ToolError, ToolOutput};
    use serde_json::json;
    use std::fs;

    #[tokio::test]
    async fn patch_should_apply_add_operation() {
        let root = temp_workspace("patch-add");
        let patch = "<<< ADD FILE: a.txt >>>\nhello world\n<<< END ADD FILE >>>";

        PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await
            .expect("patch succeeds");

        assert_eq!(
            fs::read_to_string(root.join("a.txt")).expect("read added"),
            "hello world"
        );
    }

    #[tokio::test]
    async fn patch_should_apply_update_operation() {
        let root = temp_workspace("patch-update");
        let path = root.join("a.txt");
        fs::write(&path, "one\ntwo\nthree\n").expect("write fixture");

        let patch = "<<< UPDATE FILE: a.txt >>>\n<<<<<<< SEARCH\ntwo\n=======\nTWO\n>>>>>>>\n<<< END UPDATE FILE >>>";

        PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await
            .expect("patch succeeds");

        assert_eq!(
            fs::read_to_string(&path).expect("read updated"),
            "one\nTWO\nthree\n"
        );
    }

    #[tokio::test]
    async fn patch_should_apply_delete_operation() {
        let root = temp_workspace("patch-delete");
        let path = root.join("a.txt");
        fs::write(&path, "one\n").expect("write fixture");

        let patch = "<<< DELETE FILE: a.txt >>>\n<<< END DELETE FILE >>>";

        PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await
            .expect("patch succeeds");

        assert!(!path.exists());
    }

    #[tokio::test]
    async fn patch_should_apply_move_operation() {
        let root = temp_workspace("patch-move");
        let from_path = root.join("a.txt");
        let to_path = root.join("b.txt");
        fs::write(&from_path, "one\n").expect("write fixture");

        let patch = "<<< MOVE FILE: a.txt >>>\n<<< TO: b.txt >>>\n<<< END MOVE FILE >>>";

        PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await
            .expect("patch succeeds");

        assert!(!from_path.exists());
        assert_eq!(
            fs::read_to_string(&to_path).expect("read moved"),
            "one\n"
        );
    }

    #[tokio::test]
    async fn patch_should_fail_on_malformed_envelope() {
        let root = temp_workspace("patch-malformed");
        let patch = "<<< ADD FILE: a.txt >>>\nno end";

        let result = PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn patch_should_fail_on_duplicated_directives() {
        let root = temp_workspace("patch-duplicate");
        let patch = "<<< DELETE FILE: a.txt >>>\n<<< END DELETE FILE >>>\n<<< DELETE FILE: a.txt >>>\n<<< END DELETE FILE >>>";

        let result = PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await;

        assert!(matches!(result, Err(ToolError::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn patch_with_matching_expected_hash_should_succeed() {
        let root = temp_workspace("patch-hash-success");
        let path = root.join("a.txt");
        fs::write(&path, "one\n").expect("write fixture");
        let hash = super::super::common::calculate_sha256("one\n");

        let patch = "<<< UPDATE FILE: a.txt >>>\n<<<<<<< SEARCH\none\n=======\nONE\n>>>>>>>\n<<< END UPDATE FILE >>>";

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
            fs::read_to_string(&path).expect("read updated"),
            "ONE\n"
        );
    }

    #[tokio::test]
    async fn patch_with_mismatched_expected_hash_should_fail() {
        let root = temp_workspace("patch-hash-fail");
        let path = root.join("a.txt");
        fs::write(&path, "one\n").expect("write fixture");

        let patch = "<<< UPDATE FILE: a.txt >>>\n<<<<<<< SEARCH\none\n=======\nONE\n>>>>>>>\n<<< END UPDATE FILE >>>";

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
            fs::read_to_string(&path).expect("read unchanged"),
            "one\n"
        );
    }

    #[tokio::test]
    async fn patch_dry_run_should_not_modify_files() {
        let root = temp_workspace("patch-dry-run");
        let path = root.join("a.txt");
        fs::write(&path, "one\n").expect("write fixture");

        let patch = "<<< UPDATE FILE: a.txt >>>\n<<<<<<< SEARCH\none\n=======\nONE\n>>>>>>>\n<<< END UPDATE FILE >>>";

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
            fs::read_to_string(&path).expect("read unchanged"),
            "one\n"
        );

        match output {
            ToolOutput::Text { content } => {
                let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
                assert_eq!(parsed["path"], "a.txt");
                assert_eq!(parsed["patch_applied"], false);
                assert_eq!(parsed["dry_run"], true);
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn patch_with_both_rename_and_content_update_should_work() {
        let root = temp_workspace("patch-rename-and-update");
        let from_path = root.join("a.txt");
        let to_path = root.join("b.txt");
        fs::write(&from_path, "one\n").expect("write fixture");

        let patch = "<<< MOVE FILE: a.txt >>>\n<<< TO: b.txt >>>\n<<< END MOVE FILE >>>\n<<< UPDATE FILE: b.txt >>>\n<<<<<<< SEARCH\none\n=======\nONE\n>>>>>>>\n<<< END UPDATE FILE >>>";

        PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await
            .expect("patch succeeds");

        assert!(!from_path.exists());
        assert_eq!(
            fs::read_to_string(&to_path).expect("read moved and updated"),
            "ONE\n"
        );
    }

    #[tokio::test]
    async fn patch_expected_hash_should_protect_delete_and_move() {
        let root = temp_workspace("patch-hash-destructive");
        let path = root.join("a.txt");
        fs::write(&path, "original\n").expect("write fixture");

        // Stale hash for delete
        let patch_delete = "<<< DELETE FILE: a.txt >>>\n<<< END DELETE FILE >>>";
        let result_delete = PatchTool
            .execute(
                json!({
                    "path": "a.txt",
                    "patch": patch_delete,
                    "expected_hash": "wronghash",
                }),
                &ctx(&root),
            )
            .await;
        assert!(result_delete.is_err());
        assert!(path.exists());

        // Stale hash for move
        let patch_move = "<<< MOVE FILE: a.txt >>>\n<<< TO: b.txt >>>\n<<< END MOVE FILE >>>";
        let result_move = PatchTool
            .execute(
                json!({
                    "path": "a.txt",
                    "patch": patch_move,
                    "expected_hash": "wronghash",
                }),
                &ctx(&root),
            )
            .await;
        assert!(result_move.is_err());
        assert!(path.exists());
    }

    #[tokio::test]
    async fn patch_multiple_updates_on_primary_with_expected_hash_should_succeed() {
        let root = temp_workspace("patch-hash-multi-update");
        let path = root.join("a.txt");
        fs::write(&path, "one\n").expect("write fixture");
        let hash = super::super::common::calculate_sha256("one\n");

        let patch = "<<< UPDATE FILE: a.txt >>>\n<<<<<<< SEARCH\none\n=======\nONE\n>>>>>>>\n<<< END UPDATE FILE >>>\n<<< UPDATE FILE: a.txt >>>\n<<<<<<< SEARCH\nONE\n=======\nUNIFIED\n>>>>>>>\n<<< END UPDATE FILE >>>";

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
            .expect("multiple updates succeed");

        assert_eq!(
            fs::read_to_string(&path).expect("read updated"),
            "UNIFIED\n"
        );
    }

    #[tokio::test]
    async fn patch_delete_then_move_to_same_destination_should_work() {
        let root = temp_workspace("patch-delete-then-move");
        let a_path = root.join("a.txt");
        let b_path = root.join("b.txt");
        fs::write(&a_path, "from_a\n").expect("write fixture");
        fs::write(&b_path, "from_b\n").expect("write fixture");

        let patch = "<<< DELETE FILE: b.txt >>>\n<<< END DELETE FILE >>>\n<<< MOVE FILE: a.txt >>>\n<<< TO: b.txt >>>\n<<< END MOVE FILE >>>";

        PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await
            .expect("delete then move succeeds");

        assert!(!a_path.exists());
        assert_eq!(
            fs::read_to_string(&b_path).expect("read final"),
            "from_a\n"
        );
    }

    #[tokio::test]
    async fn patch_should_be_failure_atomic() {
        let root = temp_workspace("patch-failure-atomic");
        let a_path = root.join("a.txt");
        let bad_dir = root.join("parent.txt");
        fs::write(&a_path, "survivor\n").expect("write fixture");
        fs::write(&bad_dir, "not a directory\n").expect("write fixture");

        // Try to delete a.txt, and write into a subpath of parent.txt (which is a file, so it fails to write)
        let patch = "<<< DELETE FILE: a.txt >>>\n<<< END DELETE FILE >>>\n<<< ADD FILE: parent.txt/child.txt >>>\nhello\n<<< END ADD FILE >>>";

        let result = PatchTool
            .execute(json!({"path": "a.txt", "patch": patch}), &ctx(&root))
            .await;

        assert!(result.is_err());
        // a.txt must STILL exist because the patch should have rolled back atomicly!
        assert!(a_path.exists());
        assert_eq!(fs::read_to_string(&a_path).unwrap(), "survivor\n");
    }
}
