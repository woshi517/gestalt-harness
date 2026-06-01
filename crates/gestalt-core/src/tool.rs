use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

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
    pub artifact_dir: Option<PathBuf>,
    pub current_tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    pub fn into_execution_result(
        self,
        is_error: bool,
        max_bytes: usize,
        ctx: &ToolContext,
        tool_call_id: &str,
    ) -> Result<ToolExecutionResult, ToolError> {
        let full_content = match &self {
            Self::Text { content } => content.clone(),
            Self::Json { value } => value.to_string(),
            Self::Artifact { path, .. } => format!("artifact saved: {}", path.display()),
        };

        let mut artifact = match &self {
            Self::Artifact {
                path,
                mime_type,
                size_bytes,
            } => Some(ToolArtifact {
                path: path.clone(),
                mime_type: mime_type.clone(),
                size_bytes: *size_bytes,
            }),
            _ => None,
        };

        let artifact_path = ctx
            .artifact_dir
            .as_ref()
            .map(|artifact_dir| artifact_path(artifact_dir, tool_call_id, ".txt"));

        let exec_report_truncated = extract_exec_truncated_flag(&full_content);
        let original_len = full_content.len();
        let truncated = exec_report_truncated.unwrap_or(original_len > max_bytes);
        let mut original_bytes = if truncated { Some(original_len) } else { None };
        let mut output_hash = None;

        let content = if truncated {
            let truncated_content = full_content.chars().take(max_bytes).collect::<String>();
            if let Some(path) = artifact_path {
                if path.exists() {
                    let bytes = std::fs::read(&path).map_err(ToolError::ExecutionFailed)?;
                    artifact = Some(ToolArtifact {
                        path: path.clone(),
                        mime_type: "text/plain".to_string(),
                        size_bytes: bytes.len(),
                    });
                    let mut hasher = Sha256::new();
                    hasher.update(&bytes);
                    output_hash = Some(format!("{:x}", hasher.finalize()));
                    original_bytes = Some(bytes.len());
                } else {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(ToolError::ExecutionFailed)?;
                    }
                    std::fs::write(&path, &full_content).map_err(ToolError::ExecutionFailed)?;
                    artifact = Some(ToolArtifact {
                        path: path.clone(),
                        mime_type: "text/plain".to_string(),
                        size_bytes: original_len,
                    });
                    let mut hasher = Sha256::new();
                    hasher.update(full_content.as_bytes());
                    output_hash = Some(format!("{:x}", hasher.finalize()));
                }
            } else {
                let mut hasher = Sha256::new();
                hasher.update(full_content.as_bytes());
                output_hash = Some(format!("{:x}", hasher.finalize()));
            }

            truncated_content
        } else {
            if let Some(path) = artifact_path {
                if path.exists() {
                    let metadata = std::fs::metadata(&path).map_err(ToolError::ExecutionFailed)?;
                    artifact = Some(ToolArtifact {
                        path: path.clone(),
                        mime_type: "text/plain".to_string(),
                        size_bytes: metadata.len() as usize,
                    });
                }
            }
            full_content
        };

        // Fallback hashing for any other ToolOutput::Artifact variant
        if let Some(ref art) = artifact {
            if output_hash.is_none() {
                let bytes = std::fs::read(&art.path).map_err(ToolError::ExecutionFailed)?;
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                output_hash = Some(format!("{:x}", hasher.finalize()));
            }
        }

        Ok(ToolExecutionResult {
            content,
            is_error,
            artifact,
            truncated,
            original_bytes,
            output_hash,
            metadata: Value::Null,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    pub content: String,
    pub is_error: bool,
    pub artifact: Option<ToolArtifact>,
    pub truncated: bool,
    pub original_bytes: Option<usize>,
    #[serde(default)]
    pub output_hash: Option<String>,
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
            output_hash: None,
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
            output_hash: None,
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

pub fn artifact_path(artifact_dir: &Path, tool_call_id: &str, suffix: &str) -> PathBuf {
    artifact_dir.join(format!(
        "{}{}",
        sanitize_artifact_stem(tool_call_id),
        suffix
    ))
}

pub fn sanitize_artifact_stem(tool_call_id: &str) -> String {
    let mut stem = tool_call_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    stem = stem.trim_matches('_').to_string();

    let mut hasher = Sha256::new();
    hasher.update(tool_call_id.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let prefix = if stem.is_empty() {
        "tool-call".to_string()
    } else {
        stem.chars().take(24).collect()
    };
    format!("{prefix}-{}", &digest[..12])
}

pub fn is_audited_local_command(command: &str) -> bool {
    let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");

    let mut command_to_check = normalized.as_str();
    if command_to_check.starts_with("bash -lc ") {
        command_to_check = &command_to_check["bash -lc ".len()..];
    } else if command_to_check.starts_with("bash -c ") {
        command_to_check = &command_to_check["bash -c ".len()..];
    } else if command_to_check.starts_with("sh -c ") {
        command_to_check = &command_to_check["sh -c ".len()..];
    }

    let trimmed = command_to_check.trim_matches(|c| c == '\'' || c == '"');
    let normalized_trimmed = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    if contains_shell_metacharacters(&normalized_trimmed)
        || normalized_trimmed.contains("/dev/tcp")
        || normalized_trimmed.contains("/dev/udp")
    {
        return false;
    }

    let local_prefixes = [
        "cargo test",
        "cargo check",
        "cargo build",
        "ls",
        "grep",
        "rg",
        "find",
        "cat",
        "git status",
        "git diff",
        "pwd",
        "echo",
        "printf",
        "git show",
        "git log",
        "sleep",
    ];
    local_prefixes.iter().any(|prefix| {
        normalized_trimmed == *prefix || normalized_trimmed.starts_with(&format!("{prefix} "))
    })
}

fn contains_shell_metacharacters(command: &str) -> bool {
    command.chars().any(|ch| {
        matches!(
            ch,
            '>' | '<' | '|' | '&' | ';' | '`' | '$' | '\\' | '\n' | '\r'
        )
    })
}

fn extract_exec_truncated_flag(content: &str) -> Option<bool> {
    let mut lines = content.lines();
    let _ = lines.next()?;
    let _ = lines.next()?;
    let line = lines.next()?;
    line.strip_prefix("truncated: ")?.parse().ok()
}
