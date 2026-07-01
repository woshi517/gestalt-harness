//! Explicitly unstable runtime implementation surface.
//!
//! These exports support first-party crates, integration tests, and extension
//! development. They may change without v0.1 compatibility guarantees. Product
//! integrations should prefer [`crate::api::v1`].

#![allow(ambiguous_glob_reexports)]

macro_rules! unstable_module {
    ($name:ident) => {
        pub mod $name {
            pub use crate::$name::*;
        }
    };
}

unstable_module!(activation);
unstable_module!(artifact_store);
unstable_module!(builder);
unstable_module!(compaction);
unstable_module!(composition_hooks);
unstable_module!(config);
unstable_module!(context);
unstable_module!(control);
unstable_module!(discovery);
unstable_module!(error);
unstable_module!(event_bus);
unstable_module!(exec);
unstable_module!(extension);
unstable_module!(extension_trust);
unstable_module!(inspect);
unstable_module!(jsonrpc);
unstable_module!(lifecycle);
unstable_module!(manifest);
unstable_module!(orchestration);
unstable_module!(permissions);
unstable_module!(policy);
unstable_module!(registry);
unstable_module!(runtime);
unstable_module!(session_queue);
unstable_module!(tool_catalog);
unstable_module!(tool_catalog_planner);
unstable_module!(tool_output);
unstable_module!(workspace_context);
unstable_module!(workspace_snapshot);

#[cfg(feature = "mcp")]
unstable_module!(mcp);
#[cfg(feature = "providers")]
unstable_module!(providers);
#[cfg(feature = "skills")]
unstable_module!(skills);
#[cfg(feature = "tools")]
unstable_module!(tools);
#[cfg(feature = "trace")]
unstable_module!(trace);
#[cfg(feature = "verify")]
unstable_module!(verify);

pub use crate::activation::{
    ActivationCandidate, ActivationDiagnostic, ActivationMode, ActivationRequest,
    BaseRuntimeComposition, DiagnosticSeverity, ExtensionActivationPipeline,
    ExtensionGenerationDiff, ExtensionSource, HostApprovalBroker, HostLaunchContext,
    ManagedExtensionResource, RuntimeSnapshotLease, StaticExtensionSource,
};
pub use crate::artifact_store::{ArtifactStore, FilesystemArtifactStore, InMemoryArtifactStore};
pub use crate::builder::AgentRuntimeBuilder;
pub use crate::composition_hooks::{
    AfterContextBuildCtx, AfterToolResultCtx, BeforeContextBuildCtx, BeforeToolPolicyCtx,
    CompositionHooks, HookOutcome, OnEventCtx, PrepareNextTurnCtx, RuntimeContextHookAdapter,
    RuntimeNextTurnHookAdapter, RuntimeToolHookAdapter, RuntimeTraceHookAdapter,
};
pub use crate::config::RuntimeConfig;
pub use crate::context::assembler::{
    estimate_message_tokens, estimate_text_tokens, ContextMessageAssembler,
};
pub use crate::context::projection::{
    CompactionCheckpoint, MessageMetadataRef, ProjectionManifest,
};
pub use crate::context::report::{
    load_context_build_report, persist_context_build_report, CapturedContributionV1,
    ContextBuildReportInputV1, ContextBuildReportV1, ContextOmissionReportV1,
    ContextPersistenceDiagnosticV1, ContextPressureV1, ContextSourceReportV1,
    CONTEXT_BUILD_REPORT_SCHEMA_VERSION, MAX_CAPTURED_CONTRIBUTIONS_BYTES,
    MAX_CAPTURED_CONTRIBUTION_BYTES,
};
pub use crate::context::{
    accounting, checkpoint_validation, compaction as context_compaction, default_prompt,
    tool_clearing, tool_exchanges, ContextContributor, ContextPatch, RuntimeContextPipeline,
};
pub use crate::control::{ReloadExtensionsReport, ReloadExtensionsRequest};
pub use crate::discovery::{DiscoveredExtensionPackage, ExtensionDiscovery};
pub use crate::error::{Result, RuntimeError};
pub use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
pub use crate::exec::{ExecRequest, ExecResult, ExecutionSandbox, NoSandbox, SandboxMount};
pub use crate::extension::RuntimeModule;
pub use crate::extension_trust::{ExtensionTrust, TrustedExtensionPin};
pub use crate::inspect::{
    compute_hook_contract_hash, compute_policy_fingerprint, RuntimeInspect, ToolInspectInfo,
};
pub use crate::manifest::{Entrypoint, Permissions};
pub use crate::orchestration::{
    AgentRuntimeHandle, DefaultAgentRuntimeHandle, OrchestrationResult, OrchestrationTask,
    Orchestrator, RuntimeHost,
};
pub use crate::permissions::{
    check_network_permission, check_network_permission_effective, check_path_permission,
    check_path_permission_effective, check_shell_permission, check_shell_permission_effective,
};
pub use crate::policy::engine::{
    classify_bash, BashPolicy, MinimalPolicyEngine, NetworkPolicy, PathPolicy, PolicyAction,
    PolicyConfig,
};
pub use crate::policy::RuntimePolicyEngine;
pub use crate::registry::{
    compute_schema_hash, compute_tool_schema_hash, ProviderFactory, ProviderMetadata,
    RuntimeFingerprint, RuntimeRegistryBuilder, RuntimeRegistrySnapshot, ToolMetadata,
    ToolRegistrationSnapshot,
};
pub use crate::runtime::{AgentRuntime, UserInput};
pub use crate::session_queue::InMemorySteeringQueue;
pub use crate::tool_catalog::ComposedToolCatalog;
pub use crate::tool_catalog_planner::{ToolCatalogPlanner, ToolProfile};
pub use crate::tool_output::RuntimeToolOutputMaterializer;
pub use crate::workspace_context::{
    load_and_snapshot_workspace_context, ContextSnapshotMode, MemoryContextConfig,
    MemorySelectionStrategy, MemoryWriteMode, WorkspaceContextConfig, WorkspaceContextError,
    WorkspaceContextLoader, WorkspaceContextSnapshot,
};
pub use crate::workspace_snapshot::GitWorkspaceSnapshotter;

#[cfg(feature = "mcp")]
pub use crate::mcp::*;
#[cfg(feature = "providers")]
pub use crate::providers::registry::registered;
#[cfg(feature = "providers")]
pub use crate::providers::*;
#[cfg(feature = "skills")]
pub use crate::skills::*;
#[cfg(feature = "tools")]
pub use crate::tools::*;
#[cfg(feature = "trace")]
pub use crate::trace::*;
#[cfg(feature = "verify")]
pub use crate::verify::*;

#[cfg(feature = "mcp")]
pub use crate::mcp::{
    client, error as mcp_error, model as mcp_model, registry as mcp_registry, transport,
};
#[cfg(feature = "providers")]
pub use crate::providers::{auth, catalog, openai, registry as model_registry, sse, strict_schema};
#[cfg(feature = "skills")]
pub use crate::skills::{
    activation as skill_activation, discovery as skill_discovery, events as skill_events,
    index as skill_index, manifest as skill_manifest, policy as skill_policy,
    resources as skill_resources,
};
#[cfg(feature = "tools")]
pub use crate::tools::{backends, path, registry as tool_registry_module};
#[cfg(feature = "trace")]
pub use crate::trace::{
    context_artifacts, evaluator, fixture, golden, resume, run_manifest, tool_metrics,
};
#[cfg(feature = "verify")]
pub use crate::verify::verifiers;

/// First-party accessors for builder implementation state.
///
/// This trait is intentionally outside `api::v1`; downstream embedders must not
/// rely on it as a compatibility contract.
pub trait AgentRuntimeBuilderExt {
    fn runtime_config(&self) -> &RuntimeConfig;
    fn runtime_event_bus(&self) -> &RuntimeEventBus;
    fn runtime_registry(&self) -> &RuntimeRegistryBuilder;
    fn runtime_registry_mut(&mut self) -> &mut RuntimeRegistryBuilder;
    fn configured_tools(&self) -> Option<&std::sync::Arc<dyn gestalt_core::tool::ToolCatalog>>;
}

impl AgentRuntimeBuilderExt for AgentRuntimeBuilder {
    fn runtime_config(&self) -> &RuntimeConfig {
        &self.config
    }

    fn runtime_event_bus(&self) -> &RuntimeEventBus {
        &self.event_bus
    }

    fn runtime_registry(&self) -> &RuntimeRegistryBuilder {
        &self.registry
    }

    fn runtime_registry_mut(&mut self) -> &mut RuntimeRegistryBuilder {
        &mut self.registry
    }

    fn configured_tools(&self) -> Option<&std::sync::Arc<dyn gestalt_core::tool::ToolCatalog>> {
        self.tools.as_ref()
    }
}
