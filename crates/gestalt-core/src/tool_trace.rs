use serde::{Deserialize, Serialize};
use crate::tool_descriptor::{ToolNamespace, AnnotationSource};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallTraceMetadata {
    pub namespace: ToolNamespace,
    pub annotation_source: AnnotationSource,
    pub policy_source: Option<String>,
    pub duration_ms: Option<u64>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRetryTraceMetadata {
    pub attempt: usize,
    pub error: String,
    pub next_retry_delay_ms: u64,
}
