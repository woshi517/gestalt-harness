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
    },
    ModelRequest {
        provider: String,
        model: String,
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
        execution_mode: Option<ExecutionMode>,
        matched_rule_id: Option<String>,
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
    },
    MemoryProposal {
        diff: String,
    },
    VerificationResult {
        status: VerificationStatus,
        checks: usize,
        failed: usize,
        report: Option<String>,
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
