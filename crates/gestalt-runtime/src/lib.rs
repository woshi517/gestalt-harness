pub mod config;
pub mod error;
pub mod builder;
pub mod runtime;
pub mod registry;
pub mod inspect;
pub mod context;
pub mod composition_hooks;
pub mod policy;
pub mod extension;

pub use config::RuntimeConfig;
pub use error::{RuntimeError, Result};
pub use builder::AgentRuntimeBuilder;
pub use runtime::{AgentRuntime, UserInput};
pub use registry::{RuntimeRegistry, ToolMetadata, ProviderMetadata, ProviderFactory, compute_schema_hash, compute_tool_schema_hash};
pub use inspect::{RuntimeInspect, ToolInspectInfo, compute_hook_contract_hash, compute_policy_fingerprint};
pub use context::{ContextContributor, RuntimeContextPipeline};
pub use composition_hooks::{
    CompositionHooks, HookOutcome,
    BeforeContextBuildCtx, AfterContextBuildCtx, BeforeToolPolicyCtx, AfterToolResultCtx, OnEventCtx,
    RuntimeContextHookAdapter, RuntimeToolHookAdapter, RuntimeTraceHookAdapter,
};
pub use policy::RuntimePolicyEngine;
pub use extension::GestaltExtension;
