mod command_tool;
mod component;
pub mod config;
mod instance;
mod inventory;
mod launcher;
mod manager;
mod mcp_component;
mod package;
mod process_instance;
mod runtime_module;
mod runtime_snapshot;

pub use crate::lifecycle::{
    ContextProviderPlan, EventObserverPlan, ExternalVerifierPlan, PolicyGuardPlan, TurnRouterPlan,
};
pub use command_tool::CommandTool;
pub use component::{ComponentKind, ExtensionComponentDescriptor, ResolvedExtensionComponent};
pub use config::{ExtensionGrantConfig, ExtensionInstanceConfig, ExtensionsConfig};
pub use instance::{ComponentInstanceId, ExtensionInstanceSpec};
pub use inventory::ExtensionInventory;
pub use launcher::{ExtensionLauncher, LocalProcessLauncher, NoopExtensionLauncher};
pub use manager::{ComponentFingerprint, ExtensionManager, ExtensionRuntimeComponent, ReuseKey};
pub use mcp_component::{merge_mcp_server_configs, package_mcp_server_name, package_mcp_servers};
pub use package::{
    compute_complete_fingerprint, resolve_configured_instances, ExtensionCompatibility,
    ExtensionManifestV2, ExtensionPackageDescriptor, ResolvedExtensionPackage,
};
pub use process_instance::{ExtensionProcessInstance, ExtensionProcessState, InFlightCallGuard};
pub use runtime_module::{GestaltExtension, RuntimeModule};
pub use runtime_snapshot::{
    ExtensionInstanceHealth, ExtensionInstanceHealthStatus, RuntimeExtensionSnapshot,
    RuntimeGeneration,
};
