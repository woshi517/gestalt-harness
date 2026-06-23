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
    PromptSnapshotCreated {
        snapshot_hash: String,
        prefix_hash: String,
        created_turn: usize,
    },
    PromptSnapshotLoaded {
        snapshot_hash: String,
        source: String,
    },
    PromptSnapshotReused {
        snapshot_hash: String,
        prefix_hash: String,
    },
    PromptCachePlanGenerated {
        snapshot_hash: String,
        prefix_hash: String,
        prefix_message_count: usize,
    },
    EphemeralContextInjected {
        source: String,
        token_estimate: usize,
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
        /// Optional structured failure report. Mirrors
        /// `Message::ToolResult.failure` and the
        /// `ToolExecutionResult.failure` field; included here so trace
        /// consumers do not have to re-parse the rendered `output`
        /// string to recover the failure class.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<crate::tool_failure::ToolErrorReport>,
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
    // --- Session Lineage & Resumability boundaries ---
    //
    // `context_state` is boxed so this variant does not bloat the entire
    // `AgentEvent` enum (clippy::large_enum_variant). It is only emitted at
    // session boundaries, so the extra allocation is negligible and avoids
    // forcing ~472 bytes onto every event value flowing through the runtime.
    Checkpoint {
        history: Vec<crate::context::SessionMessage>,
        #[serde(default)]
        context_state: Box<crate::context::ContextProjectionState>,
        token_budget: crate::context::TokenBudget,
        #[serde(default)]
        latest_projection_id: Option<String>,
        packet_hash: Option<String>,
        prompt_source: Option<String>,
    },
    AssistantMessageCommitted {
        message: crate::message::Message,
    },
    Interrupted {
        reason: String,
    },
    ContextBuildStarted,
    ContextBuildFailed {
        reason: String,
    },
    ModelResponseStarted {
        provider_request_hash: String,
    },
    ModelResponseStreamCompleted {
        provider_request_hash: String,
    },
    ModelResponseStreamFailed {
        provider_request_hash: String,
        error: String,
    },
    ModelResponseStreamInterrupted {
        provider_request_hash: String,
    },
    PolicyEvaluationStarted {
        tool_call_id: String,
    },
    PolicyEvaluationFailed {
        tool_call_id: String,
        error: String,
    },
    PolicyEvaluationCancelled {
        tool_call_id: String,
    },
    ApprovalRequested {
        tool_call_id: String,
        tool_name: String,
        input: Value,
        risk: RiskLevel,
    },
    ApprovalCancelled {
        tool_call_id: String,
    },
    ToolExecutionStarted {
        id: String,
        tool_name: String,
        input_hash: String,
        policy_source: String,
        working_dir: String,
        parallel_group_id: Option<String>,
        parallel_safe: bool,
    },
    HookStarted {
        hook_type: String,
        name: String,
    },
    HookCompleted {
        hook_type: String,
        name: String,
    },
    HookFailed {
        hook_type: String,
        name: String,
        error: String,
    },
    ToolCatalogSelected {
        tools: Vec<crate::tool_name_mapping::ToolNameMapping>,
    },
    ToolCallValidationFailed {
        tool_call_id: String,
        tool_name: String,
        error: crate::tool_failure::ToolErrorReport,
    },
    ToolRetryAttempt {
        tool_call_id: String,
        attempt: usize,
        error: String,
        delay_ms: u64,
    },
    NextTurnOverrideRequested {
        model: String,
        #[serde(default)]
        provider: Option<String>,
        #[serde(default)]
        variant: Option<String>,
    },
    NextTurnBlocked {
        reason: String,
    },
    SessionMessageInjected {
        message: crate::session_queue::QueuedSessionMessage,
    },
    SessionMessageQueueDrained {
        count: usize,
    },
    ContextContributorResolved {
        name: String,
        stability: String,
    },
    WorkspaceContextLoaded {
        path: String,
        bytes: usize,
        tokens: usize,
    },
    WorkspaceContextSkipped {
        reason: String,
    },
    WorkspaceContextRejected {
        reason: String,
    },
    WorkspaceContextLoadFailed {
        error: String,
    },
    MemoryContextLoadFailed {
        error: String,
    },
    MemoryContextLoaded {
        path: String,
        bytes: usize,
        tokens: usize,
        strategy: String,
    },
    MemoryContextSkipped {
        reason: String,
    },
    MemoryContextRejected {
        reason: String,
    },
    MemoryEntriesSelected {
        total_entries: usize,
        selected_entries: usize,
        pinned_entries: usize,
    },
    ContextSnapshotCreated {
        hash: String,
    },
    MemoryProposalCreated {
        session_id: String,
        proposal_id: String,
        operation_count: usize,
    },
    MemoryProposalDecisionRecorded {
        proposal_id: String,
        decision: String,
        accepted_operations: Vec<String>,
    },
    MemoryWriteSucceeded {
        path: String,
        bytes: usize,
    },
    MemoryWriteConflict {
        path: String,
        expected_hash: String,
        actual_hash: String,
    },
    MemoryWriteFailed {
        path: String,
        error: String,
    },
    ContextPressure {
        usable_limit: usize,
        current_estimate: usize,
    },
    ContextClearing {
        cleared_count: usize,
        cleared_tokens: usize,
    },
    ContextCompactionStarted {
        range: crate::context::HistoryRange,
        canonical_range: crate::context::HistoryRange,
    },
    ContextCompacted {
        checkpoint_id: String,
        range: crate::context::HistoryRange,
        canonical_range: crate::context::HistoryRange,
    },
    ContextManagementFailed {
        error: String,
    },
    ContextExhaustion {
        details: String,
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
    HookBlocked,
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
