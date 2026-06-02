use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::approval::SessionGrant;
use crate::session::ExecutionMode;
use crate::tool::RiskLevel;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    UserMessage {
        content: String,
    },
    ContextBuilt {
        packet_id: String,
        token_estimate: usize,
        #[serde(default)]
        packet_hash: Option<String>,
        #[serde(default)]
        sources: Option<Vec<crate::context::ContextSourceRef>>,
        #[serde(default)]
        omissions: Option<Vec<crate::context::ContextOmission>>,
        #[serde(default)]
        prompt_source: Option<String>,
    },
    ModelRequest {
        provider: String,
        model: String,
        #[serde(default)]
        packet_hash: Option<String>,
        #[serde(default)]
        temperature: Option<f32>,
        #[serde(default)]
        max_tokens: Option<usize>,
        #[serde(default)]
        provider_request_hash: Option<String>,
    },
    Text {
        delta: String,
    },
    Thinking {
        delta: String,
    },
    ToolCallStreamed {
        id: String,
        name: String,
        input_delta: String,
    },
    ToolCallProposed {
        id: String,
        name: String,
        input: Value,
    },
    PolicyDecision {
        tool_call_id: String,
        tool_name: Option<String>,
        input_hash: Option<String>,
        risk: Option<RiskLevel>,
        mode: Option<ExecutionMode>,
        matched_rule: Option<String>,
        decision: PolicyStatus,
        reason: Option<String>,
        policy_source: String,
    },
    ApprovalDecision {
        tool_call_id: String,
        decision: ApprovalOutcome,
        original_input_hash: String,
        edited_input_hash: Option<String>,
        grant_terms: Option<SessionGrant>,
    },
    ToolResult {
        id: String,
        output: String,
        is_error: bool,
        truncated: bool,
        #[serde(default)]
        tool_name: Option<String>,
        #[serde(default)]
        working_dir: Option<String>,
        #[serde(default)]
        duration_ms: Option<u64>,
        #[serde(default)]
        output_hash: Option<String>,
        #[serde(default)]
        artifact_refs: Option<Vec<String>>,
        #[serde(default)]
        policy_source: Option<String>,
    },
    ArtifactCreated {
        path: String,
        size_bytes: usize,
        mime_type: String,
        hash: String,
    },
    PolicyViolation {
        tool_call_id: String,
        tool_name: String,
        reason: String,
    },
    MemoryProposal {
        diff: String,
    },
    VerificationResult {
        status: VerificationStatus,
        checks: usize,
        failed: usize,
        report: Option<String>,
        #[serde(default)]
        findings: Option<Vec<VerificationFinding>>,
    },
    Usage {
        input_tokens: usize,
        output_tokens: usize,
    },
    Stop {
        reason: StopReason,
    },
    Error {
        message: String,
        recoverable: bool,
    },
    WorkspaceSnapshotCaptured {
        snapshot_id: String,
        dirty: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyStatus {
    Allowed,
    Confirm,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxOutput,
    ContentFiltered,
    MaxTurns,
    BudgetExhausted,
    PolicyViolation,
    ProviderError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    Warning,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalOutcome {
    Approve,
    Deny,
    Edit,
    AlwaysAllow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationFinding {
    pub severity: FindingSeverity,
    pub message: String,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Error,
    Warning,
    Info,
}
