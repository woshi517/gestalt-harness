use glob::Pattern;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use gestalt_core::{RiskLevel, Tool, ToolContext, ToolError, ToolOutput, ToolSchema};

use crate::path::validate_child_dir;

use super::common::{decode_text, invalid_input, parse_input, tool_schema};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchInput {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub file_glob: Option<String>,
    #[serde(default)]
    pub case_insensitive: Option<bool>,
    #[serde(default)]
    pub max_results: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SearchTool;

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

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let input = parse_input::<SearchInput>(self.name(), input)?;
        let root = validate_child_dir(input.path.as_deref(), ctx)?;
        let glob = input
            .file_glob
            .as_deref()
            .map(Pattern::new)
            .transpose()
            .map_err(|err| invalid_input(self.name(), err.to_string()))?;
        let needle = if input.case_insensitive.unwrap_or(false) {
            input.pattern.to_ascii_lowercase()
        } else {
            input.pattern.clone()
        };
        let max_results = input.max_results.unwrap_or(100);
        let mut results = Vec::new();
        search_dir(
            &root,
            &root,
            glob.as_ref(),
            &needle,
            input.case_insensitive.unwrap_or(false),
            max_results,
            &mut results,
        )?;

        Ok(ToolOutput::Text {
            content: results.join("\n"),
        })
    }
}

fn search_dir(
    root: &std::path::Path,
    current: &std::path::Path,
    glob: Option<&Pattern>,
    needle: &str,
    case_insensitive: bool,
    max_results: usize,
    results: &mut Vec<String>,
) -> Result<(), ToolError> {
    if results.len() >= max_results {
        return Ok(());
    }

    let entries = std::fs::read_dir(current).map_err(ToolError::ExecutionFailed)?;
    for entry in entries {
        let entry = entry.map_err(ToolError::ExecutionFailed)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(ToolError::ExecutionFailed)?;
        if file_type.is_dir() {
            search_dir(
                root,
                &path,
                glob,
                needle,
                case_insensitive,
                max_results,
                results,
            )?;
        } else if file_type.is_file() && glob_matches(root, &path, glob) {
            search_file(root, &path, needle, case_insensitive, max_results, results)?;
        }
        if results.len() >= max_results {
            break;
        }
    }
    Ok(())
}

fn glob_matches(root: &std::path::Path, path: &std::path::Path, glob: Option<&Pattern>) -> bool {
    glob.map_or(true, |pattern| {
        path.strip_prefix(root)
            .ok()
            .and_then(std::path::Path::to_str)
            .is_some_and(|relative| pattern.matches(relative))
    })
}

fn search_file(
    root: &std::path::Path,
    path: &std::path::Path,
    needle: &str,
    case_insensitive: bool,
    max_results: usize,
    results: &mut Vec<String>,
) -> Result<(), ToolError> {
    let bytes = std::fs::read(path).map_err(ToolError::ExecutionFailed)?;
    let Ok(content) = decode_text("search", &bytes) else {
        return Ok(());
    };
    let relative = path.strip_prefix(root).unwrap_or(path);
    for (line_index, line) in content.lines().enumerate() {
        let haystack = if case_insensitive {
            line.to_ascii_lowercase()
        } else {
            line.to_string()
        };
        if haystack.contains(needle) {
            results.push(format!(
                "{}:{}:{}",
                relative.display(),
                line_index + 1,
                line
            ));
        }
        if results.len() >= max_results {
            break;
        }
    }
    Ok(())
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
}
