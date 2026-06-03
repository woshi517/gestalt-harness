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
pub mod snapshot;
pub mod tool;
pub mod trace;
pub mod turn;

pub use agent::AgentLoop;
pub use approval::{
    hash_input, hash_input_short, ApprovalDecision, ApprovalProvider, ApprovalRequest,
    AutoApprovalProvider, DenyApprovalProvider, SessionGrant,
};
pub use cancel::CancelToken;
pub use context::{ContextPipeline, TokenBudget};
pub use error::{
    ApprovalError, ConfigError, ContextError, HarnessError, PolicyError, ProviderError, Result,
    ToolError, TraceError,
};
pub use event::{
    AgentEvent, ApprovalOutcome, FindingSeverity, PolicyStatus, StopReason, VerificationFinding,
    VerificationStatus,
};
pub use hook::{
    ContextHook, HookRegistry, ModelHook, SessionHook, ToolHook, TraceHook, VerificationHook,
};
pub use message::{ContentBlock, ContentTrust, DocumentSource, ImageSource, Message};
pub use model::{ModelInfo, ModelInfoSource};
pub use policy::{PolicyDecision, PolicyEngine, PolicyRequest};
pub use provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest};
pub use session::{ExecutionMode, RunResult, Session, SessionConfig};
pub use snapshot::{GitWorkspaceSnapshotter, WorkspaceSnapshot, WorkspaceSnapshotter};
pub use tool::{
    artifact_path, is_audited_local_command, sanitize_artifact_stem, RiskLevel, Tool, ToolArtifact,
    ToolCatalog, ToolContext, ToolExecutionResult, ToolOutput, ToolSchema,
};
pub use turn::{AssistantTurn, ProposedToolCall, TurnAccumulator};
