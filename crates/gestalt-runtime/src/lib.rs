#![allow(deprecated)]

#[path = "legacy/context/lib.rs"]
mod legacy_context;
#[path = "legacy/exec/lib.rs"]
mod legacy_exec;
#[path = "legacy/mcp/lib.rs"]
mod legacy_mcp;
#[path = "legacy/models/lib.rs"]
mod legacy_models;
#[path = "legacy/policy/lib.rs"]
mod legacy_policy;
#[path = "legacy/skills/lib.rs"]
mod legacy_skills;
#[path = "legacy/tools/lib.rs"]
mod legacy_tools;
#[path = "legacy/trace/lib.rs"]
mod legacy_trace;
#[path = "legacy/verify/lib.rs"]
mod legacy_verify;

pub mod activation;
pub mod artifact_store;
pub mod builder;
pub mod compaction;
pub mod composition_hooks;
pub mod config;
pub mod context;
pub mod control;
pub mod discovery;
pub mod error;
pub mod event_bus;
pub mod extension;
pub mod extension_trust;
pub mod inspect;
pub mod jsonrpc;
pub mod lifecycle;
pub mod manifest;
pub mod orchestration;
pub mod permissions;
pub mod policy;
pub mod process_extension;
pub mod registry;
pub mod runtime;
pub mod session_queue;
pub mod skill_contributor;
pub mod tool_catalog;
pub mod tool_catalog_planner;
pub mod workspace_context;
pub mod workspace_snapshot;

pub use legacy_context::*;
pub use legacy_exec::{ExecRequest, ExecResult, ExecutionSandbox, NoSandbox, SandboxMount};
pub use legacy_mcp::*;
pub use legacy_models::registry::registered;
pub use legacy_models::*;
pub use legacy_policy::*;
pub use legacy_skills::*;
pub use legacy_tools::*;
pub use legacy_trace::*;
pub use legacy_verify::*;

pub use legacy_context::{
    accounting, checkpoint_validation, compaction as context_compaction, default_prompt,
    tool_clearing, tool_exchanges,
};
pub use legacy_mcp::transport;
pub use legacy_mcp::{client, error as mcp_error, model as mcp_model, registry as mcp_registry};
pub use legacy_models::sse;
pub use legacy_models::{auth, catalog, openai, registry as model_registry, strict_schema};
pub use legacy_skills::{
    activation as skill_activation, discovery as skill_discovery, events as skill_events,
    index as skill_index, manifest as skill_manifest, policy as skill_policy,
    resources as skill_resources,
};
pub use legacy_tools::{backends, path, registry as tool_registry_module, tools};
pub use legacy_trace::{
    context_artifacts, evaluator, fixture, golden, resume, run_manifest, tool_metrics,
};
pub use legacy_verify::verifiers;

pub mod mcp;
pub mod mcp_discovery;

pub use activation::{
    ActivationCandidate, ActivationDiagnostic, ActivationMode, ActivationRequest,
    BaseRuntimeComposition, DiagnosticSeverity, ExtensionActivationPipeline,
    ExtensionGenerationDiff, ExtensionSource, HostApprovalBroker, HostLaunchContext,
    ManagedExtensionResource, RuntimeSnapshotLease, StaticExtensionSource,
};
pub use artifact_store::{ArtifactStore, FilesystemArtifactStore, InMemoryArtifactStore};
pub use builder::AgentRuntimeBuilder;
pub use composition_hooks::{
    AfterContextBuildCtx, AfterToolResultCtx, BeforeContextBuildCtx, BeforeToolPolicyCtx,
    CompositionHooks, HookOutcome, OnEventCtx, PrepareNextTurnCtx, RuntimeContextHookAdapter,
    RuntimeNextTurnHookAdapter, RuntimeToolHookAdapter, RuntimeTraceHookAdapter,
};
pub use config::RuntimeConfig;
pub use context::{ContextContributor, ContextPatch, RuntimeContextPipeline};
pub use control::{HostControl, ReloadExtensionsReport, ReloadExtensionsRequest, RuntimeControl};
pub use discovery::{DiscoveredExtension, DiscoveredExtensionPackage, ExtensionDiscovery};
pub use error::{Result, RuntimeError};
pub use event_bus::{RuntimeEvent, RuntimeEventBus};
pub use extension::GestaltExtension;
pub use extension_trust::{build_extension_tool_descriptor, ExtensionTrust};
pub use inspect::{
    compute_hook_contract_hash, compute_policy_fingerprint, RuntimeInspect, ToolInspectInfo,
};
pub use manifest::{Capabilities, Entrypoint, ExtensionManifest, Permissions};
pub use mcp::McpBackedTool;
pub use mcp_discovery::{GetToolDetailsTool, McpDiscoveryState, SearchToolsTool};
pub use orchestration::{
    AgentRuntimeHandle, DefaultAgentRuntimeHandle, OrchestrationResult, OrchestrationTask,
    Orchestrator, RuntimeHost,
};
pub use permissions::{
    check_network_permission, check_network_permission_effective, check_path_permission,
    check_path_permission_effective, check_shell_permission, check_shell_permission_effective,
};
pub use policy::RuntimePolicyEngine;
pub use process_extension::{
    ProcessBackedContextContributor, ProcessBackedTool, ProcessExtension, ProcessExtensionBroker,
};
pub use registry::{
    compute_schema_hash, compute_tool_schema_hash, ProviderFactory, ProviderMetadata,
    RuntimeFingerprint, RuntimeRegistry, RuntimeRegistryBuilder, RuntimeRegistrySnapshot,
    ToolMetadata, ToolRegistrationSnapshot,
};
pub use runtime::{AgentRuntime, UserInput};
pub use session_queue::InMemorySteeringQueue;
pub use tool_catalog::ComposedToolCatalog;
pub use tool_catalog_planner::{ToolCatalogPlanner, ToolProfile};
pub use workspace_context::{
    load_and_snapshot_workspace_context, ContextSnapshotMode, MemoryContextConfig,
    MemorySelectionStrategy, MemoryWriteMode, WorkspaceContextConfig, WorkspaceContextError,
    WorkspaceContextLoader, WorkspaceContextSnapshot,
};
pub use workspace_snapshot::GitWorkspaceSnapshotter;
