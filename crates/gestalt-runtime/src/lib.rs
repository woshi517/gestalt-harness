pub mod artifact_store;
pub mod builder;
pub mod composition_hooks;
pub mod config;
pub mod context;
pub mod discovery;
pub mod error;
pub mod event_bus;
pub mod extension;
pub mod extension_trust;
pub mod inspect;
pub mod jsonrpc;
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

pub mod mcp;
pub mod mcp_discovery;

pub use artifact_store::{ArtifactStore, FilesystemArtifactStore, InMemoryArtifactStore};
pub use builder::AgentRuntimeBuilder;
pub use composition_hooks::{
    AfterContextBuildCtx, AfterToolResultCtx, BeforeContextBuildCtx, BeforeToolPolicyCtx,
    CompositionHooks, HookOutcome, OnEventCtx, PrepareNextTurnCtx, RuntimeContextHookAdapter,
    RuntimeNextTurnHookAdapter, RuntimeToolHookAdapter, RuntimeTraceHookAdapter,
};
pub use config::RuntimeConfig;
pub use context::{ContextContributor, ContextPatch, RuntimeContextPipeline};
pub use discovery::{DiscoveredExtension, ExtensionDiscovery};
pub use error::{Result, RuntimeError};
pub use event_bus::{RuntimeEvent, RuntimeEventBus};
pub use extension::GestaltExtension;
pub use extension_trust::build_extension_tool_descriptor;
pub use inspect::{
    compute_hook_contract_hash, compute_policy_fingerprint, RuntimeInspect, ToolInspectInfo,
};
pub use manifest::{Capabilities, Entrypoint, ExtensionManifest, Permissions};
pub use orchestration::{
    AgentRuntimeHandle, DefaultAgentRuntimeHandle, OrchestrationResult, OrchestrationTask,
    Orchestrator,
};
pub use permissions::{check_network_permission, check_path_permission, check_shell_permission};
pub use policy::RuntimePolicyEngine;
pub use process_extension::{
    ProcessBackedContextContributor, ProcessBackedTool, ProcessExtension, ProcessExtensionBroker,
};
pub use registry::{
    compute_schema_hash, compute_tool_schema_hash, ProviderFactory, ProviderMetadata,
    RuntimeRegistry, ToolMetadata,
};
pub use runtime::{AgentRuntime, UserInput};
pub use session_queue::InMemorySteeringQueue;
pub use tool_catalog::ComposedToolCatalog;
pub use tool_catalog_planner::{ToolCatalogPlanner, ToolProfile};
pub use mcp::McpBackedTool;
pub use mcp_discovery::{McpDiscoveryState, SearchToolsTool, GetToolDetailsTool};

