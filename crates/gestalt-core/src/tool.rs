use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ToolError;

pub type ToolSchema = Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    fn risk(&self, input: &Value) -> RiskLevel;

    fn can_run_in_parallel(&self, input: &Value) -> bool {
        matches!(self.risk(input), RiskLevel::Low)
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
}

#[async_trait]
pub trait ToolCatalog: Send + Sync {
    fn schemas(&self) -> Vec<ToolSchema>;
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub workspace_root: Option<PathBuf>,
    pub timeout: Duration,
    pub allow_network: bool,
    pub environment: HashMap<String, String>,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutput {
    Text {
        content: String,
    },
    Json {
        value: Value,
    },
    Artifact {
        path: PathBuf,
        mime_type: String,
        size_bytes: usize,
    },
}

impl ToolOutput {
    pub fn into_execution_result(self, is_error: bool, max_bytes: usize) -> ToolExecutionResult {
        let content = match &self {
            Self::Text { content } => content.clone(),
            Self::Json { value } => value.to_string(),
            Self::Artifact { path, .. } => format!("artifact saved: {}", path.display()),
        };

        let artifact = match self {
            Self::Artifact {
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

        let original_len = content.len();
        let (content, truncated, original_bytes) = if original_len > max_bytes {
            let truncated_content = content.chars().take(max_bytes).collect::<String>();
            (truncated_content, true, Some(original_len))
        } else {
            (content, false, None)
        };

        ToolExecutionResult {
            content,
            is_error,
            artifact,
            truncated,
            original_bytes,
            metadata: Value::Null,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    pub content: String,
    pub is_error: bool,
    pub artifact: Option<ToolArtifact>,
    pub truncated: bool,
    pub original_bytes: Option<usize>,
    pub metadata: Value,
}

impl ToolExecutionResult {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            is_error: true,
            artifact: None,
            truncated: false,
            original_bytes: None,
            metadata: Value::Null,
        }
    }

    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            artifact: None,
            truncated: false,
            original_bytes: None,
            metadata: Value::Null,
        }
    }

    pub fn truncation_notice(&self) -> String {
        format!(
            "[Output truncated. Original: {} bytes. Full output saved to artifact: {}]",
            self.original_bytes.unwrap_or(0),
            self.artifact.as_ref().map_or_else(
                || "unavailable".to_string(),
                |artifact| artifact.path.display().to_string(),
            )
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolArtifact {
    pub path: PathBuf,
    pub mime_type: String,
    pub size_bytes: usize,
}
