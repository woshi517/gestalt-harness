use serde::{Deserialize, Serialize};
use serde_json::Value;

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};

use crate::backends::{default_file_search_backend, FileSearchRequest};
use crate::path::{validate_child_dir, PathFilter};

use super::common::{parse_input, tool_schema};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FindFilesInput {
    /// Fuzzy query pattern to filter file paths.
    pub query: String,
    /// Scopes the file search to a specific subdirectory. Defaults to the workspace root.
    #[serde(default)]
    pub path: Option<String>,
    /// Optional glob pattern to filter file paths.
    #[serde(default)]
    pub file_glob: Option<String>,
    /// Include hidden files in the search results. Defaults to false.
    #[serde(default)]
    pub include_hidden: bool,
    /// Respect the workspace's `.gitignore` rules. Defaults to true.
    #[serde(default = "super::common::default_true")]
    pub respect_gitignore: bool,
    /// Maximum number of search results to return. Defaults to 50.
    #[serde(default = "super::common::default_find_files_max_results")]
    pub max_results: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FindFilesTool;

impl FindFilesTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Tool for FindFilesTool {
    fn name(&self) -> &str {
        "find_files"
    }

    fn description(&self) -> &str {
        "Fuzzy find files inside the workspace relative to a directory root."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<FindFilesInput>(self.name(), self.description())
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Low
    }

    fn descriptor(&self) -> gestalt_core::tool_descriptor::ToolDescriptor {
        crate::builtin_descriptors::make_builtin_descriptor(
            self,
            true, // read_only
            true, // idempotent
            Some(gestalt_core::tool_descriptor::ToolRetryPolicy {
                max_retries: 2,
                backoff_ms: 100,
            }),
            &[("backend", "walkdir")],
        )
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<FindFilesInput>(self.name(), input)?;
        let root = validate_child_dir(input.path.as_deref(), ctx)?;

        let include_hidden = input.include_hidden;
        let respect_gitignore = input.respect_gitignore;
        let max_results = input.max_results;

        let backend = default_file_search_backend();
        let request = FileSearchRequest {
            query: input.query.clone(),
            root: root.clone(),
            max_results,
            file_glob: input.file_glob.clone(),
        };

        let raw_results = backend.search(&request).await.map_err(|e| {
            ToolError::ExecutionFailed(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            ))
        })?;

        let filter = PathFilter::new(ctx, &root, include_hidden, respect_gitignore);
        let mut filtered_lines = Vec::new();

        for r in raw_results {
            if filter.is_visible(&r.path) {
                let rel_path = if let Some(ref ws_root) = ctx.workspace_root {
                    match r.path.strip_prefix(ws_root) {
                        Ok(p) => p.to_string_lossy().to_string(),
                        Err(_) => r.path.to_string_lossy().to_string(),
                    }
                } else {
                    match r.path.strip_prefix(&root) {
                        Ok(p) => p.to_string_lossy().to_string(),
                        Err(_) => r.path.to_string_lossy().to_string(),
                    }
                };
                let kind = if r.is_dir { "dir" } else { "file" };
                let mut details = vec![kind.to_string()];
                if let Some(size) = r.file_size {
                    details.push(format!("size: {} bytes", size));
                }
                if let Some(score) = r.score {
                    details.push(format!("score: {:.2}", score));
                }
                filtered_lines.push(format!("{} ({})", rel_path, details.join(", ")));
            }
        }

        filtered_lines.truncate(max_results);

        Ok(ToolOutput::Text {
            content: filtered_lines.join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, temp_workspace};
    use super::FindFilesTool;
    use gestalt_core::{Tool, ToolOutput};
    use serde_json::json;
    use std::fs;

    #[tokio::test]
    async fn find_files_finds_nested_files() {
        let root = temp_workspace("find_files");
        fs::create_dir_all(root.join("src/utils")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/utils/helper.rs"), "").unwrap();

        let tool = FindFilesTool::new();
        let output = tool
            .execute(
                json!({
                    "query": "helper",
                    "path": "src"
                }),
                &ctx(&root),
            )
            .await
            .unwrap();

        match output {
            ToolOutput::Text { content } => {
                assert!(content.contains("utils/helper.rs"));
                assert!(!content.contains("main.rs"));
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn find_files_respects_glob() {
        let root = temp_workspace("find_files_glob");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/README.md"), "").unwrap();

        let tool = FindFilesTool::new();
        let output = tool
            .execute(
                json!({
                    "query": "main",
                    "file_glob": "*.rs"
                }),
                &ctx(&root),
            )
            .await
            .unwrap();

        match output {
            ToolOutput::Text { content } => {
                assert!(content.contains("src/main.rs"));
                assert!(!content.contains("README.md"));
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn find_files_excludes_hidden_by_default() {
        let root = temp_workspace("find_files_hidden_default");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/.hidden.rs"), "").unwrap();

        let tool = FindFilesTool::new();
        let output = tool
            .execute(
                json!({
                    "query": "hidden",
                }),
                &ctx(&root),
            )
            .await
            .unwrap();

        match output {
            ToolOutput::Text { content } => {
                assert!(!content.contains(".hidden.rs"));
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn find_files_includes_hidden_when_flag_enabled() {
        let root = temp_workspace("find_files_hidden_enabled");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/.hidden.rs"), "").unwrap();

        let tool = FindFilesTool::new();
        let output = tool
            .execute(
                json!({
                    "query": "hidden",
                    "include_hidden": true
                }),
                &ctx(&root),
            )
            .await
            .unwrap();

        match output {
            ToolOutput::Text { content } => {
                assert!(content.contains("src/.hidden.rs"));
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn find_files_always_excludes_secrets() {
        let root = temp_workspace("find_files_sec");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/.env"), "").unwrap();
        fs::write(root.join("src/secret.key"), "").unwrap();

        let tool = FindFilesTool::new();
        let output = tool
            .execute(
                json!({
                    "query": "",
                    "include_hidden": true
                }),
                &ctx(&root),
            )
            .await
            .unwrap();

        match output {
            ToolOutput::Text { content } => {
                assert!(
                    content.contains("src/main.rs"),
                    "Expected main.rs in results: {}",
                    content
                );
                assert!(
                    !content.contains(".env"),
                    "Expected .env to be filtered out: {}",
                    content
                );
                assert!(
                    !content.contains("secret.key"),
                    "Expected secret.key to be filtered out: {}",
                    content
                );
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn find_files_respects_gitignore() {
        let root = temp_workspace("find_files_gitignore");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/ignored.rs"), "").unwrap();
        fs::write(root.join(".gitignore"), "src/ignored.rs\n").unwrap();

        let tool = FindFilesTool::new();
        let output = tool
            .execute(
                json!({
                    "query": "ignored",
                }),
                &ctx(&root),
            )
            .await
            .unwrap();

        match output {
            ToolOutput::Text { content } => {
                assert!(!content.contains("ignored.rs"));
            }
            _ => panic!("Expected text output"),
        }

        let output2 = tool
            .execute(
                json!({
                    "query": "ignored",
                    "respect_gitignore": false
                }),
                &ctx(&root),
            )
            .await
            .unwrap();

        match output2 {
            ToolOutput::Text { content } => {
                assert!(content.contains("src/ignored.rs"));
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn find_files_respects_custom_ignore() {
        let root = temp_workspace("find_files_custom_ignore");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/custom_ignored.rs"), "").unwrap();

        let mut custom_ctx = ctx(&root);
        custom_ctx.ignore_patterns = vec!["custom_ignored.rs".to_string()];

        let tool = FindFilesTool::new();
        let output = tool
            .execute(
                json!({
                    "query": "custom_ignored",
                }),
                &custom_ctx,
            )
            .await
            .unwrap();

        match output {
            ToolOutput::Text { content } => {
                assert!(!content.contains("custom_ignored.rs"));
            }
            _ => panic!("Expected text output"),
        }
    }
}
