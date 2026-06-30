#![allow(deprecated)]

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
pub mod exec;
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
pub mod tool_catalog;
pub mod tool_catalog_planner;
pub mod tool_output;
pub mod workspace_context;
pub mod workspace_snapshot;

#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "providers")]
pub mod providers;
#[cfg(feature = "skills")]
pub mod skills;
#[cfg(feature = "tools")]
pub mod tools;
#[cfg(feature = "trace")]
pub mod trace;
#[cfg(feature = "verify")]
pub mod verify;

pub use context::assembler::{
    estimate_message_tokens, estimate_text_tokens, ContextMessageAssembler,
};
pub use exec::{ExecRequest, ExecResult, ExecutionSandbox, NoSandbox, SandboxMount};
#[cfg(feature = "mcp")]
pub use mcp::*;
pub use policy::engine::{
    classify_bash, BashPolicy, MinimalPolicyEngine, NetworkPolicy, PathPolicy, PolicyAction,
    PolicyConfig,
};
#[cfg(feature = "providers")]
pub use providers::registry::registered;
#[cfg(feature = "providers")]
pub use providers::*;
#[cfg(feature = "skills")]
pub use skills::*;
#[cfg(feature = "tools")]
pub use tools::*;
#[cfg(feature = "trace")]
pub use trace::*;
#[cfg(feature = "verify")]
pub use verify::*;

pub use context::{
    accounting, checkpoint_validation, compaction as context_compaction, default_prompt,
    tool_clearing, tool_exchanges,
};
#[cfg(feature = "mcp")]
pub use mcp::transport;
#[cfg(feature = "mcp")]
pub use mcp::{client, error as mcp_error, model as mcp_model, registry as mcp_registry};
#[cfg(feature = "providers")]
pub use providers::sse;
#[cfg(feature = "providers")]
pub use providers::{auth, catalog, openai, registry as model_registry, strict_schema};
#[cfg(feature = "skills")]
pub use skills::{
    activation as skill_activation, discovery as skill_discovery, events as skill_events,
    index as skill_index, manifest as skill_manifest, policy as skill_policy,
    resources as skill_resources,
};
#[cfg(feature = "tools")]
pub use tools::{backends, path, registry as tool_registry_module};
#[cfg(feature = "trace")]
pub use trace::{
    context_artifacts, evaluator, fixture, golden, resume, run_manifest, tool_metrics,
};
#[cfg(feature = "verify")]
pub use verify::verifiers;

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
pub use context::projection::{CompactionCheckpoint, MessageMetadataRef, ProjectionManifest};
pub use context::report::{
    load_context_build_report, persist_context_build_report, CapturedContributionV1,
    ContextBuildReportInputV1, ContextBuildReportV1, ContextOmissionReportV1,
    ContextPersistenceDiagnosticV1, ContextPressureV1, ContextSourceReportV1,
    CONTEXT_BUILD_REPORT_SCHEMA_VERSION, MAX_CAPTURED_CONTRIBUTIONS_BYTES,
    MAX_CAPTURED_CONTRIBUTION_BYTES,
};
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
#[cfg(feature = "mcp")]
pub use mcp::McpBackedTool;
#[cfg(feature = "mcp")]
pub use mcp::{GetToolDetailsTool, McpDiscoveryState, SearchToolsTool};
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
pub use tool_output::RuntimeToolOutputMaterializer;
pub use workspace_context::{
    load_and_snapshot_workspace_context, ContextSnapshotMode, MemoryContextConfig,
    MemorySelectionStrategy, MemoryWriteMode, WorkspaceContextConfig, WorkspaceContextError,
    WorkspaceContextLoader, WorkspaceContextSnapshot,
};
pub use workspace_snapshot::GitWorkspaceSnapshotter;
