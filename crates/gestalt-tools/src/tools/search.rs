use serde::{Deserialize, Serialize};
use serde_json::Value;

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};

use crate::path::{validate_child_dir, PathFilter};
use crate::backends::{default_text_search_backend, TextSearchRequest};

use super::common::{parse_input, tool_schema};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchInput {
    /// The search pattern (literal string or regex).
    pub pattern: String,
    /// Scopes the search to a specific subdirectory. Defaults to the workspace root.
    #[serde(default)]
    pub path: Option<String>,
    /// Optional glob pattern to filter files.
    #[serde(default)]
    pub file_glob: Option<String>,
    /// Whether to perform case-insensitive matching. Defaults to false.
    #[serde(default)]
    pub case_insensitive: bool,
    /// Treat the pattern as a regular expression. Defaults to false.
    #[serde(default)]
    pub is_regex: bool,
    /// Number of context lines to return before each match. Defaults to 0.
    #[serde(default)]
    pub context_before: usize,
    /// Number of context lines to return after each match. Defaults to 0.
    #[serde(default)]
    pub context_after: usize,
    /// Include hidden files in the search. Defaults to false.
    #[serde(default)]
    pub include_hidden: bool,
    /// Respect the workspace's `.gitignore` rules. Defaults to true.
    #[serde(default = "super::common::default_true")]
    pub respect_gitignore: bool,
    /// Maximum number of search results to return. Defaults to 100.
    #[serde(default = "super::common::default_search_max_results")]
    pub max_results: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SearchTool;

impl SearchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search workspace text files with local, path-scoped semantics."
    }

    fn schema(&self) -> ToolSchema {
        tool_schema::<SearchInput>(self.name(), self.description())
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
            &[
                ("backend", "walkdir-grep"),
                ("replacement_tool", "search_text"),
            ],
        )
    }

    fn shape_output(&self, result: &mut gestalt_core::tool::ToolExecutionResult) {
        crate::response_shaping::shape_tool_response(self.name(), result);
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<SearchInput>(self.name(), input)?;
        let root = validate_child_dir(input.path.as_deref(), ctx)?;
        
        let case_insensitive = input.case_insensitive;
        let is_regex = input.is_regex;
        let context_before = input.context_before;
        let context_after = input.context_after;
        let include_hidden = input.include_hidden;
        let respect_gitignore = input.respect_gitignore;
        let max_results = input.max_results;

        let backend = default_text_search_backend();
        let request = TextSearchRequest {
            pattern: input.pattern.clone(),
            root: root.clone(),
            is_regex,
            case_insensitive,
            context_before,
            context_after,
            max_results,
            file_glob: input.file_glob.clone(),
        };

        let raw_results = backend.search(&request).await
            .map_err(|e| ToolError::ExecutionFailed(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        let filter = PathFilter::new(ctx, &root, include_hidden, respect_gitignore);
        let mut results = Vec::new();

        let mut match_count = 0;
        for r in raw_results {
            if filter.is_visible(&r.path) {
                if match_count >= max_results {
                    break;
                }
                match_count += 1;

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

                let start_ctx_line = r.line_number.saturating_sub(r.context_before.len());
                for (idx, ctx_line) in r.context_before.iter().enumerate() {
                    results.push(format!(
                        "{}-{}-{}",
                        rel_path,
                        start_ctx_line + idx,
                        ctx_line
                    ));
                }

                results.push(format!(
                    "{}:{}:{}",
                    rel_path,
                    r.line_number,
                    r.line_content
                ));

                for (idx, ctx_line) in r.context_after.iter().enumerate() {
                    results.push(format!(
                        "{}-{}-{}",
                        rel_path,
                        r.line_number + 1 + idx,
                        ctx_line
                    ));
                }
            }
        }

        Ok(ToolOutput::Text {
            content: results.join("\n"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ctx, temp_workspace};
    use super::SearchTool;
    use gestalt_core::{Tool, ToolOutput};
    use serde_json::json;
    use std::fs;

    #[tokio::test]
    async fn search_should_find_matches_with_glob() {
        let root = temp_workspace("search");
        fs::write(root.join("a.md"), "Alpha\nBeta").expect("write md");
        fs::write(root.join("a.txt"), "Alpha").expect("write txt");

        let output = SearchTool
            .execute(
                json!({"pattern": "alpha", "file_glob": "*.md", "case_insensitive": true}),
                &ctx(&root),
            )
            .await
            .expect("search succeeds");

        assert!(matches!(output, ToolOutput::Text { content } if content == "a.md:1:Alpha"));
    }

    #[tokio::test]
    async fn search_should_find_matches_with_context() {
        let root = temp_workspace("search_context");
        fs::write(root.join("a.txt"), "Line1\nTargetLine\nLine3").expect("write txt");

        let output = SearchTool
            .execute(
                json!({
                    "pattern": "TargetLine",
                    "context_before": 1,
                    "context_after": 1
                }),
                &ctx(&root),
            )
            .await
            .expect("search succeeds");

        match output {
            ToolOutput::Text { content } => {
                let expected = "a.txt-1-Line1\na.txt:2:TargetLine\na.txt-3-Line3";
                assert_eq!(content, expected);
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn search_excludes_hidden_by_default() {
        let root = temp_workspace("search_hidden_default");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "TargetContent").unwrap();
        fs::write(root.join("src/.hidden.rs"), "TargetContent").unwrap();

        let output = SearchTool
            .execute(
                json!({"pattern": "TargetContent"}),
                &ctx(&root),
            )
            .await
            .expect("search succeeds");

        match output {
            ToolOutput::Text { content } => {
                assert!(content.contains("src/main.rs"));
                assert!(!content.contains(".hidden.rs"));
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn search_includes_hidden_when_flag_enabled() {
        let root = temp_workspace("search_hidden_enabled");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "TargetContent").unwrap();
        fs::write(root.join("src/.hidden.rs"), "TargetContent").unwrap();

        let output = SearchTool
            .execute(
                json!({
                    "pattern": "TargetContent",
                    "include_hidden": true
                }),
                &ctx(&root),
            )
            .await
            .expect("search succeeds");

        match output {
            ToolOutput::Text { content } => {
                assert!(content.contains("src/main.rs"));
                assert!(content.contains("src/.hidden.rs"));
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn search_always_excludes_secrets() {
        let root = temp_workspace("search_sec");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "TargetContent").unwrap();
        fs::write(root.join("src/.env"), "DATABASE_URL=TargetContent").unwrap();
        fs::write(root.join("src/secret.key"), "TargetContent").unwrap();

        let output = SearchTool
            .execute(
                json!({
                    "pattern": "TargetContent",
                    "include_hidden": true
                }),
                &ctx(&root),
            )
            .await
            .expect("search succeeds");

        match output {
            ToolOutput::Text { content } => {
                assert!(content.contains("src/main.rs"), "Expected main.rs match: {}", content);
                assert!(!content.contains(".env"), "Expected no .env match: {}", content);
                assert!(!content.contains("secret.key"), "Expected no secret.key match: {}", content);
            }
            _ => panic!("Expected text output"),
        }
    }

    #[tokio::test]
    async fn search_respects_gitignore() {
        let root = temp_workspace("search_gitignore");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "TargetContent").unwrap();
        fs::write(root.join("src/ignored.rs"), "TargetContent").unwrap();
        fs::write(root.join(".gitignore"), "src/ignored.rs\n").unwrap();

        let output = SearchTool
            .execute(
                json!({"pattern": "TargetContent"}),
                &ctx(&root),
            )
            .await
            .expect("search succeeds");

        match output {
            ToolOutput::Text { content } => {
                assert!(content.contains("src/main.rs"));
                assert!(!content.contains("ignored.rs"));
            }
            _ => panic!("Expected text output"),
        }

        let output2 = SearchTool
            .execute(
                json!({
                    "pattern": "TargetContent",
                    "respect_gitignore": false
                }),
                &ctx(&root),
            )
            .await
            .expect("search succeeds");

        match output2 {
            ToolOutput::Text { content } => {
                assert!(content.contains("src/main.rs"));
                assert!(content.contains("src/ignored.rs"));
            }
            _ => panic!("Expected text output"),
        }
    }
}
