//! # gestalt-core
//!
//! Core traits, event types, agent loop, and session state for gestalt-harness.
//!
//! ## Ownership Boundary
//!
//! This crate defines the interfaces that all other gestalt crates depend on.
//! It contains **zero** file I/O, **zero** HTTP calls, and **zero** concrete
//! tool or provider implementations.
//!
//! ## Key Exports (Phase 1)
//!
//! - `AgentLoop` — the sacred loop (~200 lines)
//! - `AgentEvent` — the unified event enum
//! - `Message`, `ContentBlock` — transcript types
//! - `Provider`, `Tool`, `PolicyEngine`, `ContextPipeline` — core traits
//! - `Session`, `SessionConfig`, `RunResult` — session state
//! - `HarnessError` — error taxonomy

pub mod agent;
pub mod approval;
pub mod cancel;
pub mod context;
pub mod error;
pub mod event;
pub mod hook;
pub mod message;
pub mod model;
pub mod policy;
pub mod provider;
pub mod session;
pub mod session_queue;
pub mod snapshot;
pub mod tool;
pub mod tool_descriptor;
pub mod tool_failure;
pub mod tool_name_mapping;
pub mod tool_trace;
pub mod tool_validation;
pub mod trace;
pub mod turn;

pub use agent::AgentLoop;
pub use approval::{
    hash_input, hash_input_short, ApprovalDecision, ApprovalProvider, ApprovalRequest,
    AutoApprovalProvider, DenyApprovalProvider, SessionGrant,
};
pub use cancel::CancelToken;
pub use context::{
    ArtifactRef, CheckpointRef, ClearAction, ClearedToolResultRef, CompactionCheckpointRef,
    ContextAssembler, ContextCaptureMode, ContextEpoch, ContextManagementPolicy, ContextOmission,
    ContextPacket, ContextPipeline, ContextPlan, ContextPreparationRequest, ContextProjectionState,
    ContextSourceRef, ContextStability, ContextStateDelta, DurabilityMode, HistoryRange, MessageId,
    MessageNamespace, PreparedContext, ProjectedHistory, ProjectedHistoryItem, ProjectionManifest,
    ProjectionMessageMetadata, PromptAssemblyStrategy, PromptCachePlan, PromptSegment,
    PromptSegmentKind, PromptSnapshot, PromptSnapshotRef, SessionId, SessionMessage, StateUpdate,
    TokenBudget, ToolRetention, ToolRetentionRegistrySnapshot, ToolUseId,
};
pub use error::{
    ApprovalError, ConfigError, ContextError, HarnessError, PolicyError, ProviderError, Result,
    ToolError, TraceError,
};
pub use event::{
    AgentEvent, ApprovalOutcome, FindingSeverity, PolicyStatus, StopReason, VerificationFinding,
    VerificationStatus,
};
pub use hook::{
    ContextHook, HookDispatcher, HookRegistry, ModelHook, SessionHook, ToolHook, TraceHook,
    VerificationHook,
};
pub use message::{ContentBlock, ContentTrust, DocumentSource, ImageSource, Message};
pub use model::{
    ModelCapabilities, ModelInfo, ModelInfoSource, ModelSelection, ResolvedModelSnapshot,
};
pub use policy::{PolicyDecision, PolicyEngine, PolicyRequest};
pub use provider::{
    ApiFormat, EventStream, PromptCacheMode, Provider, ProviderCapabilities, ProviderRequest,
    ProviderToolSchema,
};
pub use session::{ExecutionMode, RunResult, Session, SessionConfig};
pub use session_queue::{
    MessageSource, QueueAck, QueueLifecycle, QueuedSessionMessage, SteeringQueue,
};
pub use snapshot::{WorkspaceSnapshot, WorkspaceSnapshotter};
pub use tool::{
    artifact_path, is_audited_local_command, sanitize_artifact_stem, RiskLevel, Tool, ToolArtifact,
    ToolCatalog, ToolContext, ToolExecutionResult, ToolOutput, ToolOutputMaterializer, ToolSchema,
};
pub use tool_descriptor::{
    AnnotationSource, CanonicalToolId, ProviderToolFormat, ResponseShapeRules, ToolAnnotation,
    ToolAnnotations, ToolDescriptor, ToolNamespace, ToolResponseContract, ToolRetryPolicy,
};
pub use tool_failure::{ToolErrorReport, ToolFailureKind};
pub use tool_name_mapping::ToolNameMapping;
pub use tool_trace::{ToolCallTraceMetadata, ToolRetryTraceMetadata};
pub use tool_validation::ToolCallValidator;
pub use turn::{AssistantTurn, ProposedToolCall, TurnAccumulator};
