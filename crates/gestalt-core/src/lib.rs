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
pub mod context;
pub mod error;
pub mod event;
pub mod message;
pub mod policy;
pub mod provider;
pub mod session;
pub mod tool;
pub mod turn;

pub use agent::AgentLoop;
pub use approval::{
    ApprovalDecision, ApprovalProvider, ApprovalRequest, AutoApprovalProvider, DenyApprovalProvider,
};
pub use context::{ContextPipeline, TokenBudget};
pub use error::{
    ApprovalError, ConfigError, ContextError, HarnessError, PolicyError, ProviderError, Result,
    ToolError, TraceError,
};
pub use event::{AgentEvent, PolicyStatus, StopReason, VerificationStatus};
pub use message::{ContentBlock, ContentTrust, DocumentSource, ImageSource, Message};
pub use policy::{PolicyDecision, PolicyEngine, PolicyRequest};
pub use provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest};
pub use session::{ExecutionMode, RunResult, Session, SessionConfig};
pub use tool::{
    RiskLevel, Tool, ToolArtifact, ToolCatalog, ToolContext, ToolExecutionResult, ToolOutput,
    ToolSchema,
};
pub use turn::{AssistantTurn, ProposedToolCall, TurnAccumulator};
