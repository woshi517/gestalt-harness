use gestalt_core::{
    approval::SessionGrant,
    context::{
        ContextOmission, ContextProjectionState, ContextSourceRef, SessionMessage, TokenBudget,
    },
    event::{ApprovalOutcome, PolicyStatus, StopReason, VerificationFinding, VerificationStatus},
    model::ResolvedModelSnapshot,
    session::ExecutionMode,
    tool::RiskLevel,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceEvent {
    RunStarted {
        resolved_model: ResolvedModelSnapshot,
    },
    UserMessage {
        content: String,
    },
    ContextBuilt {
        packet_id: String,
        token_estimate: usize,
        #[serde(default)]
        packet_hash: Option<String>,
        #[serde(default)]
        sources: Option<Vec<ContextSourceRef>>,
        #[serde(default)]
        omissions: Option<Vec<ContextOmission>>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<gestalt_core::tool_failure::ToolErrorReport>,
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
    Checkpoint {
        history: Vec<SessionMessage>,
        #[serde(default)]
        context_state: Box<ContextProjectionState>,
        token_budget: TokenBudget,
        #[serde(default)]
        latest_projection_id: Option<String>,
        packet_hash: Option<String>,
        prompt_source: Option<String>,
    },
    AssistantMessageCommitted {
        message: gestalt_core::message::Message,
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
        tools: Vec<gestalt_core::tool_name_mapping::ToolNameMapping>,
    },
    ToolCallValidationFailed {
        tool_call_id: String,
        tool_name: String,
        error: gestalt_core::tool_failure::ToolErrorReport,
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
        message: gestalt_core::session_queue::QueuedSessionMessage,
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
        range: gestalt_core::HistoryRange,
        canonical_range: gestalt_core::HistoryRange,
    },
    ContextCompacted {
        checkpoint_id: String,
        range: gestalt_core::HistoryRange,
        canonical_range: gestalt_core::HistoryRange,
    },
    ContextManagementFailed {
        error: String,
    },
    ContextExhaustion {
        details: String,
    },
}

impl TraceEvent {
    #[must_use]
    pub fn from_agent_event(event: gestalt_core::event::AgentEvent) -> Self {
        serde_json::from_value(serde_json::to_value(event).expect("serializable agent event"))
            .expect("trace event mirrors agent event")
    }

    #[must_use]
    pub fn into_agent_event(self) -> gestalt_core::event::AgentEvent {
        serde_json::from_value(serde_json::to_value(self).expect("serializable trace event"))
            .expect("agent event mirrors trace event")
    }
}

impl From<gestalt_core::event::AgentEvent> for TraceEvent {
    fn from(event: gestalt_core::event::AgentEvent) -> Self {
        Self::from_agent_event(event)
    }
}

impl From<TraceEvent> for gestalt_core::event::AgentEvent {
    fn from(event: TraceEvent) -> Self {
        event.into_agent_event()
    }
}

#[must_use]
pub fn is_known_kind(kind: &str) -> bool {
    matches!(
        kind,
        "run_started"
            | "user_message"
            | "context_built"
            | "prompt_snapshot_created"
            | "prompt_snapshot_loaded"
            | "prompt_snapshot_reused"
            | "prompt_cache_plan_generated"
            | "ephemeral_context_injected"
            | "model_request"
            | "text"
            | "thinking"
            | "tool_call_streamed"
            | "tool_call_proposed"
            | "policy_decision"
            | "approval_decision"
            | "tool_result"
            | "artifact_created"
            | "policy_violation"
            | "memory_proposal"
            | "verification_result"
            | "usage"
            | "stop"
            | "error"
            | "workspace_snapshot_captured"
            | "checkpoint"
            | "assistant_message_committed"
            | "interrupted"
            | "context_build_started"
            | "context_build_failed"
            | "model_response_started"
            | "model_response_stream_completed"
            | "model_response_stream_failed"
            | "model_response_stream_interrupted"
            | "policy_evaluation_started"
            | "policy_evaluation_failed"
            | "policy_evaluation_cancelled"
            | "approval_requested"
            | "approval_cancelled"
            | "tool_execution_started"
            | "hook_started"
            | "hook_completed"
            | "hook_failed"
            | "tool_catalog_selected"
            | "tool_call_validation_failed"
            | "tool_retry_attempt"
            | "next_turn_override_requested"
            | "next_turn_blocked"
            | "session_message_injected"
            | "session_message_queue_drained"
            | "context_contributor_resolved"
            | "workspace_context_loaded"
            | "workspace_context_skipped"
            | "workspace_context_rejected"
            | "workspace_context_load_failed"
            | "memory_context_load_failed"
            | "memory_context_loaded"
            | "memory_context_skipped"
            | "memory_context_rejected"
            | "memory_entries_selected"
            | "context_snapshot_created"
            | "memory_proposal_created"
            | "memory_proposal_decision_recorded"
            | "memory_write_succeeded"
            | "memory_write_conflict"
            | "memory_write_failed"
            | "context_pressure"
            | "context_clearing"
            | "context_compaction_started"
            | "context_compacted"
            | "context_management_failed"
            | "context_exhaustion"
    )
}
