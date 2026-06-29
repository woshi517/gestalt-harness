//! H1A Stable Runtime-Control DTOs and Traits.
//!
//! This module defines the product-neutral stable control contract.
//! It contains stable, serializable DTOs and narrow capability traits.
//!
//! All traits and types follow the architectural decisions in H0B.

use serde::{Deserialize, Serialize};
use std::fmt;

// =========================================================================
// 1. Non-interchangeable Versioned Newtypes (H1A-F02)
// =========================================================================

/// Unique identifier for a logical session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionIdV1(pub String);

impl fmt::Display for SessionIdV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a physical execution run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunIdV1(pub String);

impl fmt::Display for RunIdV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a turn within a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnIdV1(pub String);

impl fmt::Display for TurnIdV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a submitted message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageIdV1(pub String);

impl fmt::Display for MessageIdV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for an approval request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ApprovalIdV1(pub String);

impl fmt::Display for ApprovalIdV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallIdV1(pub String);

impl fmt::Display for ToolCallIdV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtifactIdV1(pub String);

impl fmt::Display for ArtifactIdV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Correlation identifier for request-response tracking.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CorrelationIdV1(pub String);

impl fmt::Display for CorrelationIdV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Key used to ensure idempotency of operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IdempotencyKeyV1(pub String);

impl fmt::Display for IdempotencyKeyV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Opaque cursor used for paging or resuming event streams.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CursorV1(pub String);

impl fmt::Display for CursorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// =========================================================================
// 2. Control Error Classification (H1A-F04)
// =========================================================================

/// Stable error codes for external control interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ControlErrorCodeV1 {
    /// Request validation failed (e.g. missing required field, bad ID format).
    Validation,
    /// Optimistic concurrency or lineage conflict (e.g. duplicate session start, stale run ID).
    Conflict,
    /// Backpressure: steering or message queue is full.
    QueueFull,
    /// Stream cursor has lagged behind the retention window.
    LaggedCursor,
    /// Stream cursor has expired.
    ExpiredCursor,
    /// Requested entity (session, run, artifact, approval) not found.
    NotFound,
    /// Action blocked by active policy rules.
    UnauthorizedPolicy,
    /// Operation was explicitly cancelled.
    Cancelled,
    /// Service is temporarily unavailable.
    Unavailable,
    /// Internal implementation failure (redacted details).
    InternalFailure,
}

/// Stable external control error payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlErrorV1 {
    /// Classification code.
    pub code: ControlErrorCodeV1,
    /// User-friendly error message.
    pub message: String,
    /// Whether repeating the request after a delay is safe/expected.
    pub retryable: bool,
    /// Optional structured/redacted details context.
    pub details: Option<serde_json::Value>,
    /// Optional correlation ID linking to host logs.
    pub correlation_id: Option<CorrelationIdV1>,
}

impl std::error::Error for ControlErrorV1 {}

impl fmt::Display for ControlErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ControlErrorV1 [code: {:?}, message: {}, retryable: {}]",
            self.code, self.message, self.retryable
        )
    }
}

// =========================================================================
// 3. Request/Response DTOs (H1A-F03)
// =========================================================================

/// Request to start a new logical session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSessionRequestV1 {
    /// Optional caller-assigned session ID.
    pub session_id: Option<SessionIdV1>,
    /// Optional key for idempotency.
    pub idempotency_key: Option<IdempotencyKeyV1>,
    /// Optional configuration overrides.
    pub config_override: Option<serde_json::Value>,
}

/// Response from starting a new logical session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartSessionResponseV1 {
    /// Active session ID.
    pub session_id: SessionIdV1,
    /// Physical run ID for the initial execution run.
    pub run_id: RunIdV1,
    /// Correlation token for tracking the session start.
    pub correlation_id: Option<CorrelationIdV1>,
}

/// Request to submit new input and continue the session's active run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinueSessionRequestV1 {
    /// Target session ID.
    pub session_id: SessionIdV1,
    /// Physical run ID the caller expects to continue.
    pub run_id: RunIdV1,
    /// Input prompt or instruction content.
    pub message: String,
    /// Optional key for idempotency.
    pub idempotency_key: Option<IdempotencyKeyV1>,
}

/// Response from continuing a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinueSessionResponseV1 {
    pub session_id: SessionIdV1,
    pub run_id: RunIdV1,
    /// Whether the message was successfully accepted into the steering queue.
    pub acknowledged: bool,
    pub correlation_id: Option<CorrelationIdV1>,
}

/// Request to resume an existing session that is currently suspended/idle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeSessionRequestV1 {
    pub session_id: SessionIdV1,
    /// Target run ID to resume.
    pub run_id: RunIdV1,
    /// Optional key for idempotency.
    pub idempotency_key: Option<IdempotencyKeyV1>,
}

/// Response from resuming a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeSessionResponseV1 {
    pub session_id: SessionIdV1,
    pub run_id: RunIdV1,
    pub correlation_id: Option<CorrelationIdV1>,
}

/// Request to branch a session starting from a specific parent checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchSessionRequestV1 {
    /// Session being branched from.
    pub parent_session_id: SessionIdV1,
    /// Run ID representing the branch-point state.
    pub parent_run_id: RunIdV1,
    /// Optional new session ID for the branch.
    pub new_session_id: Option<SessionIdV1>,
    /// Optional key for idempotency.
    pub idempotency_key: Option<IdempotencyKeyV1>,
}

/// Response from branching a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchSessionResponseV1 {
    /// The newly created session ID.
    pub new_session_id: SessionIdV1,
    /// The new physical run ID.
    pub new_run_id: RunIdV1,
    pub correlation_id: Option<CorrelationIdV1>,
}

/// Request to submit an asynchronous message to the session steering queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitMessageRequestV1 {
    pub session_id: SessionIdV1,
    /// Message payload text.
    pub message: String,
    /// Optional key for idempotency.
    pub idempotency_key: Option<IdempotencyKeyV1>,
}

/// Response from submitting a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitMessageResponseV1 {
    pub session_id: SessionIdV1,
    /// Unique identifier assigned to the queued message.
    pub message_id: MessageIdV1,
    /// True if the message was enqueued.
    pub acknowledged: bool,
    pub correlation_id: Option<CorrelationIdV1>,
}

/// Request to cancel a running execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelRunRequestV1 {
    pub session_id: SessionIdV1,
    pub run_id: RunIdV1,
    pub correlation_id: Option<CorrelationIdV1>,
}

/// Response from cancelling a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelRunResponseV1 {
    pub session_id: SessionIdV1,
    pub run_id: RunIdV1,
    /// True if the cancellation signal was delivered.
    pub cancelled: bool,
}

/// Request to list all active sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSessionsRequestV1 {
    pub cursor: Option<CursorV1>,
    pub limit: Option<usize>,
}

/// Response containing active sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSessionsResponseV1 {
    pub sessions: Vec<SessionIdV1>,
    pub next_cursor: Option<CursorV1>,
}

/// Request to list runs for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRunsRequestV1 {
    pub session_id: SessionIdV1,
    pub cursor: Option<CursorV1>,
    pub limit: Option<usize>,
}

/// Response containing runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRunsResponseV1 {
    pub runs: Vec<RunIdV1>,
    pub next_cursor: Option<CursorV1>,
}

// =========================================================================
// 4. Policy and Approval DTOs (H1A-F05, H1A-F06)
// =========================================================================

/// Projection of a policy check result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyProjectionV1 {
    pub tool_call_id: ToolCallIdV1,
    pub canonical_tool_id: String,
    pub input_hash: String,
    pub risk_level: String,
    pub execution_mode: String,
    pub decision: String,
    pub reason: Option<String>,
    pub matched_rule: Option<String>,
    pub source: Option<String>,
}

/// Terms governing a bounded session grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionGrantTermsV1 {
    pub tool_name: String,
    pub risk_ceiling: String,
    pub matched_rule: String,
    pub policy_source: String,
    pub expires_in_turns: usize,
}

/// Projection representing an outstanding/pending approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalProjectionV1 {
    pub approval_id: ApprovalIdV1,
    pub tool_call_id: ToolCallIdV1,
    pub correlation_id: Option<CorrelationIdV1>,
    pub summary: String,
    /// Schema rules or constraints on what inputs are editable.
    pub editable_input_rules: Option<serde_json::Value>,
    pub original_hash: String,
    pub edited_hash: Option<String>,
    /// RFC 3339 timestamp of expiration.
    pub expires_at: Option<String>,
    pub is_cancelled: bool,
    pub session_grant_terms: Option<SessionGrantTermsV1>,
}

/// Response decision for an approval challenge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum ApprovalDecisionV1 {
    Approve,
    Deny,
    Edit(serde_json::Value),
    AlwaysAllowForSession,
}

/// Request pending approvals for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPendingApprovalsRequestV1 {
    pub session_id: SessionIdV1,
}

/// Response listing pending approvals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListPendingApprovalsResponseV1 {
    pub approvals: Vec<ApprovalProjectionV1>,
}

/// Request to submit an approval response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RespondToApprovalRequestV1 {
    pub approval_id: ApprovalIdV1,
    pub decision: ApprovalDecisionV1,
}

/// Response from submitting an approval response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RespondToApprovalResponseV1 {
    pub success: bool,
}

// =========================================================================
// 5. Event Projection DTOs (H0B-F04, H1A-F03)
// =========================================================================

/// Lost-less wrapper around event payloads crossing the control boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelopeV1 {
    pub sequence_number: u64,
    pub run_id: RunIdV1,
    pub session_id: SessionIdV1,
    /// RFC 3339 formatted timestamp.
    pub timestamp: String,
    pub payload: EventPayloadV1,
}

/// Projected event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPayloadV1 {
    pub schema_version: String,
    pub kind: String,
    pub data: serde_json::Value,
}

/// Request to poll events for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollEventsRequestV1 {
    pub session_id: SessionIdV1,
    pub cursor: Option<CursorV1>,
    pub limit: Option<usize>,
}

/// Response containing polled events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollEventsResponseV1 {
    pub events: Vec<EventEnvelopeV1>,
    pub next_cursor: Option<CursorV1>,
}

// =========================================================================
// 6. Artifact DTOs (H1A-F07)
// =========================================================================

/// Metadata describing a logical session artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactMetadataV1 {
    pub logical_id: ArtifactIdV1,
    pub display_path: String,
    pub size: u64,
    pub media_type: String,
    pub integrity: String,
}

/// Request to list session artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListArtifactsRequestV1 {
    pub session_id: SessionIdV1,
    pub cursor: Option<CursorV1>,
    pub limit: Option<usize>,
}

/// Response containing artifact metadata list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListArtifactsResponseV1 {
    pub artifacts: Vec<ArtifactMetadataV1>,
    pub next_cursor: Option<CursorV1>,
}

/// Request to describe an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescribeArtifactRequestV1 {
    pub session_id: SessionIdV1,
    pub artifact_id: ArtifactIdV1,
}

/// Response containing artifact metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescribeArtifactResponseV1 {
    pub metadata: ArtifactMetadataV1,
}

/// Request a bounded range of artifact content bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadArtifactRangeRequestV1 {
    pub session_id: SessionIdV1,
    pub artifact_id: ArtifactIdV1,
    pub offset: u64,
    pub length: u64,
}

/// Response containing bounded artifact chunk bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadArtifactRangeResponseV1 {
    pub metadata: ArtifactMetadataV1,
    pub offset: u64,
    pub length: u64,
    pub data: Vec<u8>,
}

/// Request to create a new artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateArtifactRequestV1 {
    pub session_id: SessionIdV1,
    pub display_path: String,
    pub data: Vec<u8>,
}

/// Response containing created artifact metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateArtifactResponseV1 {
    pub metadata: ArtifactMetadataV1,
}

// =========================================================================
// 7. Runtime Inspection DTOs
// =========================================================================

/// Health status projection of an extension instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionHealthV1 {
    pub instance_id: String,
    pub status: String,
    pub details: Option<String>,
}

/// Request to inspect overall runtime state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectRuntimeRequestV1 {}

/// Response containing runtime state inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectRuntimeResponseV1 {
    pub generation: String,
    pub extension_health: Vec<ExtensionHealthV1>,
    pub active_sessions_count: usize,
}

// =========================================================================
// 8. Narrow Capability Interfaces (H1A-F01)
// =========================================================================

/// Capability to control and steer logical session lifecycles.
#[async_trait::async_trait]
pub trait SessionControlV1: Send + Sync {
    /// Start a new logical session.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Checks that config override (if present) is well-formed JSON.
    /// - **Policy**: Ensures session creation is authorized under current policy rules.
    /// - **Approval**: Not applicable.
    /// - **Provider**: Resolves default models or capabilities.
    /// - **Tool**: Not applicable.
    /// - **Trace**: Emits a `SessionStarted` trace event.
    /// - **Context**: Sets up the initial context window.
    /// - **Cancellation**: Aborting during session initialization cancels the run.
    /// - **Concurrency**: Restricts session IDs to single-active-writer. Multiple starts on same ID yield `CONFLICT`.
    /// - **Retry**: Safe to retry on `UNAVAILABLE` or `CONFLICT` (if resolving with a new ID).
    /// - **Panic**: Standard Rust panic conditions on memory exhaustion only.
    async fn start_session(
        &self,
        req: StartSessionRequestV1,
    ) -> Result<StartSessionResponseV1, ControlErrorV1>;

    /// Submit inline message to continue the session's current turn.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Validates that target session and run IDs exist and are active.
    /// - **Policy**: Evaluates policy rules against the incoming message payload.
    /// - **Approval**: May trigger approval challenge if a matched policy dictates user confirmation.
    /// - **Provider**: Standard LLM generation triggers.
    /// - **Tool**: Not applicable.
    /// - **Trace**: Appends prompt to event history.
    /// - **Context**: Checks context budget; may trigger truncation.
    /// - **Cancellation**: Aborts generation immediately if cancel token is triggered.
    /// - **Concurrency**: Serializes message submission per session; rejects concurrent writes with `QUEUE_FULL` or `CONFLICT`.
    /// - **Retry**: Retryable on `QUEUE_FULL` or connection loss.
    /// - **Panic**: Not applicable.
    async fn continue_session(
        &self,
        req: ContinueSessionRequestV1,
    ) -> Result<ContinueSessionResponseV1, ControlErrorV1>;

    /// Resume a session from a previous run/turn checkpoint.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Verifies target session exists and has a valid saved checkpoint.
    /// - **Policy**: Checks execution context permissions.
    /// - **Approval**: Not applicable.
    /// - **Provider**: Not applicable.
    /// - **Tool**: Not applicable.
    /// - **Trace**: Projects a `SessionResumed` event.
    /// - **Context**: Restores the context state to matching turn.
    /// - **Cancellation**: Aborting during startup cancels resumption.
    /// - **Concurrency**: Fails with `CONFLICT` if session is already running.
    /// - **Retry**: Retryable if conflict resolves.
    /// - **Panic**: Not applicable.
    async fn resume_session(
        &self,
        req: ResumeSessionRequestV1,
    ) -> Result<ResumeSessionResponseV1, ControlErrorV1>;

    /// Branch a session from a past checkpoint into a new logical path.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Validates parent session and run ID exist.
    /// - **Policy**: Verifies authorization to clone parent context.
    /// - **Approval**: Not applicable.
    /// - **Provider**: Not applicable.
    /// - **Tool**: Not applicable.
    /// - **Trace**: Generates a branched session record.
    /// - **Context**: Copies history up to target run checkpoint.
    /// - **Cancellation**: Not applicable.
    /// - **Concurrency**: Thread-safe; new session has a distinct single active writer.
    /// - **Retry**: Retry on collision.
    /// - **Panic**: Not applicable.
    async fn branch_session(
        &self,
        req: BranchSessionRequestV1,
    ) -> Result<BranchSessionResponseV1, ControlErrorV1>;

    /// Submit a message to the steering queue. Returns immediately.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Checks that session ID exists.
    /// - **Policy**: Enforces limits on queue depth.
    /// - **Approval**: Not applicable at queue time.
    /// - **Provider**: Not applicable.
    /// - **Tool**: Not applicable.
    /// - **Trace**: Writes message payload to input buffer.
    /// - **Context**: Not applicable.
    /// - **Cancellation**: Not applicable.
    /// - **Concurrency**: Single-active-writer per session queue; concurrent pushes reject with `QUEUE_FULL`.
    /// - **Retry**: Retryable on `QUEUE_FULL` backpressure.
    /// - **Panic**: Not applicable.
    async fn submit_message(
        &self,
        req: SubmitMessageRequestV1,
    ) -> Result<SubmitMessageResponseV1, ControlErrorV1>;

    /// Cancel a running execution.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Validates that target session and run IDs match an active execution.
    /// - **Policy**: Checks authorization to issue cancellation signals.
    /// - **Approval**: Not applicable.
    /// - **Provider**: Triggers model API cancellation if supported.
    /// - **Tool**: Aborts active subprocesses immediately.
    /// - **Trace**: Records a Cancelled trace event.
    /// - **Context**: Retains history up to cancellation point (does not rewrite).
    /// - **Cancellation**: Resolves cancellation-race conditions deterministically.
    /// - **Concurrency**: Safe to invoke concurrently with other control calls.
    /// - **Retry**: Not applicable.
    /// - **Panic**: Not applicable.
    async fn cancel_run(
        &self,
        req: CancelRunRequestV1,
    ) -> Result<CancelRunResponseV1, ControlErrorV1>;
}

/// Capability to query runs and active sessions.
#[async_trait::async_trait]
pub trait RunQueryV1: Send + Sync {
    /// List active logical sessions.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Validates cursor and page limit.
    /// - **Policy**: Filters results based on user access policies.
    /// - **Approval/Provider/Tool/Context/Cancellation**: Not applicable.
    /// - **Trace**: Not applicable.
    /// - **Concurrency**: Safe for concurrent reads.
    /// - **Retry**: Safe to retry on `LAGGED_CURSOR` or `EXPIRED_CURSOR` with new cursor.
    /// - **Panic**: Not applicable.
    async fn list_sessions(
        &self,
        req: ListSessionsRequestV1,
    ) -> Result<ListSessionsResponseV1, ControlErrorV1>;

    /// List physical runs for a logical session.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Validates session ID exists.
    /// - **Policy**: Filters results based on session visibility rules.
    /// - **Approval/Provider/Tool/Context/Cancellation**: Not applicable.
    /// - **Trace**: Not applicable.
    /// - **Concurrency**: Safe for concurrent reads.
    /// - **Retry**: Retry on cursor errors.
    /// - **Panic**: Not applicable.
    async fn list_runs(
        &self,
        req: ListRunsRequestV1,
    ) -> Result<ListRunsResponseV1, ControlErrorV1>;
}

/// Capability to manage policy approvals and decisions.
#[async_trait::async_trait]
pub trait ApprovalControlV1: Send + Sync {
    /// List pending approvals for a session.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Validates session ID.
    /// - **Policy**: Checks session read permissions.
    /// - **Approval/Provider/Tool/Context/Cancellation**: Not applicable.
    /// - **Trace**: Not applicable.
    /// - **Concurrency**: Thread-safe query.
    /// - **Retry**: Safe to retry.
    /// - **Panic**: Not applicable.
    async fn list_pending_approvals(
        &self,
        req: ListPendingApprovalsRequestV1,
    ) -> Result<ListPendingApprovalsResponseV1, ControlErrorV1>;

    /// Respond to an outstanding approval challenge.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Validates approval ID and matches hashes against original request.
    /// - **Policy**: Re-evaluates edited inputs under active policies (H1A-B06).
    /// - **Approval**: Validates expiration, duplicate, and late states.
    /// - **Provider/Tool/Trace/Context**: Not applicable.
    /// - **Cancellation**: Rejected if approval or run has been cancelled.
    /// - **Concurrency**: Synchronized response verification; duplicate responses yield `CONFLICT`.
    /// - **Retry**: Not retryable if rejected due to validation/expiration.
    /// - **Panic**: Not applicable.
    async fn respond_to_approval(
        &self,
        req: RespondToApprovalRequestV1,
    ) -> Result<RespondToApprovalResponseV1, ControlErrorV1>;

    /// Get detailed policy projection for a tool call.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Checks tool call ID exists.
    /// - **Policy/Approval/Provider/Tool/Trace/Context/Cancellation/Concurrency/Retry/Panic**: Not applicable.
    async fn get_policy_projection(
        &self,
        tool_call_id: ToolCallIdV1,
    ) -> Result<PolicyProjectionV1, ControlErrorV1>;
}

/// Capability to consume trace events from session streams.
#[async_trait::async_trait]
pub trait EventSourceV1: Send + Sync {
    /// Poll lossless events starting from an opaque cursor.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Checks cursor and session ID.
    /// - **Policy**: Redacts event details (secrets, raw chains) before returning.
    /// - **Approval/Provider/Tool/Context/Cancellation**: Not applicable.
    /// - **Trace**: Reads from local/persisted run logs (e.g. `.gestalt/runs/`).
    /// - **Concurrency**: Thread-safe polling.
    /// - **Retry**: Lagged/expired cursors return `LAGGED_CURSOR`/`EXPIRED_CURSOR` with new resumption info.
    /// - **Panic**: Not applicable.
    async fn poll_events(
        &self,
        req: PollEventsRequestV1,
    ) -> Result<PollEventsResponseV1, ControlErrorV1>;
}

/// Capability to access session artifacts.
#[async_trait::async_trait]
pub trait ArtifactAccessV1: Send + Sync {
    /// List artifacts generated within a session.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Checks session ID exists.
    /// - **Policy**: Restricts cross-session traversal or unauthorized access.
    /// - **Approval/Provider/Tool/Context/Cancellation**: Not applicable.
    /// - **Trace**: Not applicable.
    /// - **Concurrency**: Safe for concurrent reads.
    /// - **Retry**: Safe to retry.
    /// - **Panic**: Not applicable.
    async fn list_artifacts(
        &self,
        req: ListArtifactsRequestV1,
    ) -> Result<ListArtifactsResponseV1, ControlErrorV1>;

    /// Describe artifact metadata (size, integrity, display path).
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Checks session ID and artifact ID.
    /// - **Policy**: Ensures artifact exists and belongs to the requested session.
    /// - **Approval/Provider/Tool/Context/Cancellation**: Not applicable.
    /// - **Trace**: Not applicable.
    /// - **Concurrency**: Safe for concurrent reads.
    /// - **Retry**: Safe to retry.
    /// - **Panic**: Not applicable.
    async fn describe_artifact(
        &self,
        req: DescribeArtifactRequestV1,
    ) -> Result<DescribeArtifactResponseV1, ControlErrorV1>;

    /// Read a bounded range of bytes from an artifact.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Validates target range bounds and offset. Rejects negative or overflow offsets.
    /// - **Policy**: Enforces maximum chunk limits and prevents path traversal (H1A-B07).
    /// - **Approval/Provider/Tool/Context/Cancellation**: Not applicable.
    /// - **Trace**: Not applicable.
    /// - **Concurrency**: Safe for concurrent reads.
    /// - **Retry**: Safe to retry.
    /// - **Panic**: Not applicable.
    async fn read_artifact_range(
        &self,
        req: ReadArtifactRangeRequestV1,
    ) -> Result<ReadArtifactRangeResponseV1, ControlErrorV1>;

    /// Create a new artifact in the store.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Validates input size, name format, and path traversal.
    /// - **Policy**: Validates creation permissions.
    /// - **Approval/Provider/Tool/Context/Cancellation**: Not applicable.
    /// - **Trace**: Records an `ArtifactCreated` event.
    /// - **Concurrency**: Synchronized write; collision on path returns `CONFLICT`.
    /// - **Retry**: Safe to retry.
    /// - **Panic**: Not applicable.
    async fn create_artifact(
        &self,
        req: CreateArtifactRequestV1,
    ) -> Result<CreateArtifactResponseV1, ControlErrorV1>;
}

/// Capability to inspect global runtime status.
#[async_trait::async_trait]
pub trait RuntimeInspectionV1: Send + Sync {
    /// Inspect overall runtime generation status and extension health.
    ///
    /// # Behavior & Error Semantics (H0B-B03)
    /// - **Validation**: Not applicable.
    /// - **Policy**: Verifies admin/inspection authorization.
    /// - **Approval/Provider/Tool/Trace/Context/Cancellation**: Not applicable.
    /// - **Concurrency**: Safe for concurrent reads.
    /// - **Retry**: Safe to retry.
    /// - **Panic**: Not applicable.
    async fn inspect_runtime(
        &self,
        req: InspectRuntimeRequestV1,
    ) -> Result<InspectRuntimeResponseV1, ControlErrorV1>;
}

// =========================================================================
// 9. Approved Aggregate Façade (H1A-F01)
// =========================================================================

/// Aggregate control façade exporting all stable v1 control capabilities.
pub trait RuntimeControlV1:
    SessionControlV1
    + RunQueryV1
    + ApprovalControlV1
    + EventSourceV1
    + ArtifactAccessV1
    + RuntimeInspectionV1
{}
