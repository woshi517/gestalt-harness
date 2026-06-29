use crate::error::Result;
use crate::event_bus::RuntimeEventBus;
use crate::extension::{
    ComponentInstanceId, ExtensionProcessInstance, RuntimeExtensionSnapshot, RuntimeGeneration,
};
use gestalt_core::tool::Tool;
use std::path::PathBuf;
use std::sync::Arc;

pub trait ExtensionSource: Send + Sync {
    fn discover_packages(&self) -> Result<Vec<crate::extension::ResolvedExtensionPackage>>;
}

pub struct StaticExtensionSource {
    packages: Vec<crate::extension::ResolvedExtensionPackage>,
}

impl StaticExtensionSource {
    pub fn new(packages: Vec<crate::extension::ResolvedExtensionPackage>) -> Self {
        Self { packages }
    }
}

impl ExtensionSource for StaticExtensionSource {
    fn discover_packages(&self) -> Result<Vec<crate::extension::ResolvedExtensionPackage>> {
        Ok(self.packages.clone())
    }
}

pub struct BaseRuntimeComposition {
    pub tool_catalog: Arc<dyn gestalt_core::tool::ToolCatalog>,
    #[cfg(feature = "mcp")]
    pub mcp_registry: Arc<crate::mcp::McpRegistry>,
    pub base_registry: crate::registry::RuntimeRegistrySnapshot,
}

#[derive(Clone)]
pub struct HostLaunchContext {
    pub event_bus: RuntimeEventBus,
    pub workspace_root: PathBuf,
    pub allow_network: bool,
    pub effective_permissions: Option<crate::extension::ExtensionGrantConfig>,
    pub trusted_extension_ids: Vec<String>,
    pub timeout_initialize_ms: u64,
    pub timeout_hook_ms: u64,
    pub timeout_context_ms: u64,
    pub timeout_tool_ms: u64,
    pub timeout_shutdown_ms: u64,
    pub max_message_bytes: usize,
    pub max_pending_requests: usize,
    pub environment: std::collections::HashMap<String, String>,
    pub package_source_root: Option<PathBuf>,
    pub extension_instances:
        std::collections::BTreeMap<String, crate::extension::ExtensionInstanceConfig>,
    #[cfg(feature = "mcp")]
    pub mcp_servers: std::collections::HashMap<String, crate::mcp::McpServerConfig>,
}

impl std::fmt::Debug for HostLaunchContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("HostLaunchContext");
        s.field("workspace_root", &self.workspace_root)
            .field("allow_network", &self.allow_network)
            .field("effective_permissions", &self.effective_permissions)
            .field("trusted_extension_ids", &self.trusted_extension_ids)
            .field("timeout_initialize_ms", &self.timeout_initialize_ms)
            .field("timeout_hook_ms", &self.timeout_hook_ms)
            .field("timeout_context_ms", &self.timeout_context_ms)
            .field("timeout_tool_ms", &self.timeout_tool_ms)
            .field("timeout_shutdown_ms", &self.timeout_shutdown_ms)
            .field("max_message_bytes", &self.max_message_bytes)
            .field("max_pending_requests", &self.max_pending_requests)
            .field("environment", &self.environment)
            .field("package_source_root", &self.package_source_root)
            .field("extension_instances", &self.extension_instances);
        #[cfg(feature = "mcp")]
        {
            s.field("mcp_servers", &self.mcp_servers);
        }
        s.finish_non_exhaustive()
    }
}

impl Default for HostLaunchContext {
    fn default() -> Self {
        Self::from_runtime_config(
            &crate::config::RuntimeConfig::default(),
            RuntimeEventBus::new(),
        )
    }
}

impl HostLaunchContext {
    pub fn from_runtime_config(
        config: &crate::config::RuntimeConfig,
        event_bus: RuntimeEventBus,
    ) -> Self {
        Self {
            event_bus,
            workspace_root: config.workspace_root.clone(),
            allow_network: config.allow_network,
            effective_permissions: None,
            trusted_extension_ids: config.trusted_extension_ids.clone(),
            timeout_initialize_ms: config.extension_timeouts.initialize_ms.unwrap_or(10_000),
            timeout_hook_ms: config.extension_timeouts.hook_ms.unwrap_or(5_000),
            timeout_context_ms: config.extension_timeouts.context_ms.unwrap_or(15_000),
            timeout_tool_ms: config.extension_timeouts.tool_ms.unwrap_or(60_000),
            timeout_shutdown_ms: config.extension_timeouts.shutdown_ms.unwrap_or(5_000),
            max_message_bytes: config
                .extension_limits
                .max_message_bytes
                .unwrap_or(8_388_608),
            max_pending_requests: config.extension_limits.max_pending_requests.unwrap_or(16),
            environment: config.environment.clone(),
            package_source_root: None,
            extension_instances: config.extension_instances.clone(),
            #[cfg(feature = "mcp")]
            mcp_servers: config.mcp_servers.clone(),
        }
    }
}

pub struct ExtensionActivationPipeline {
    pub discovery: Arc<dyn ExtensionSource>,
    pub launcher: Arc<dyn crate::extension::ExtensionLauncher>,
    pub base_composition: Arc<BaseRuntimeComposition>,
    pub host_context: HostLaunchContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationMode {
    DryRun,
    Commit,
}

pub struct ActivationRequest {
    pub current: Option<Arc<RuntimeExtensionSnapshot>>,
    pub target_instance: Option<String>,
    pub force: bool,
    pub mode: ActivationMode,
}

#[derive(Debug, Clone)]
pub struct ManagedMcpServer {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct ObserverWorker {
    pub component_id: String,
}

#[derive(Clone)]
pub enum ManagedExtensionResource {
    Process {
        reuse_key: crate::extension::ReuseKey,
        process: Arc<ExtensionProcessInstance>,
    },
    Mcp {
        reuse_key: crate::extension::ReuseKey,
        server: Arc<ManagedMcpServer>,
    },
    Observer {
        reuse_key: crate::extension::ReuseKey,
        worker: Arc<ObserverWorker>,
    },
}

impl std::fmt::Debug for ManagedExtensionResource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Process { process, .. } => f.debug_tuple("Process").field(process).finish(),
            Self::Mcp { server, .. } => f.debug_tuple("Mcp").field(server).finish(),
            Self::Observer { worker, .. } => f.debug_tuple("Observer").field(worker).finish(),
        }
    }
}

impl ManagedExtensionResource {
    pub fn reuse_key(&self) -> &crate::extension::ReuseKey {
        match self {
            Self::Process { reuse_key, .. } => reuse_key,
            Self::Mcp { reuse_key, .. } => reuse_key,
            Self::Observer { reuse_key, .. } => reuse_key,
        }
    }

    pub fn id(&self) -> String {
        match self {
            Self::Process { process, .. } => process.component_id.clone(),
            Self::Mcp { server, .. } => server.name.clone(),
            Self::Observer { worker, .. } => worker.component_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct ActivationDiagnostic {
    pub component_id: ComponentInstanceId,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct ExtensionGenerationDiff {
    pub added: Vec<ComponentInstanceId>,
    pub removed: Vec<ComponentInstanceId>,
    pub replaced: Vec<ComponentInstanceId>,
    pub reused: Vec<ComponentInstanceId>,
}

pub struct ActivationCandidate {
    pub snapshot: Arc<RuntimeExtensionSnapshot>,
    pub diff: ExtensionGenerationDiff,
    pub newly_started: Vec<ManagedExtensionResource>,
    pub reused: Vec<ManagedExtensionResource>,
    pub diagnostics: Vec<ActivationDiagnostic>,
    pub committed: bool,
}

impl ActivationCandidate {
    pub fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ActivationCandidate {
    fn drop(&mut self) {
        if !self.committed {
            for res in &self.newly_started {
                if let ManagedExtensionResource::Process { process: p, .. } = res {
                    let p = p.clone();
                    tokio::spawn(async move {
                        p.transition_to(crate::extension::ExtensionProcessState::Stopping);
                        p.shutdown().await;
                    });
                }
            }
        }
    }
}

pub struct GenerationRetirement {
    pub generation: RuntimeGeneration,
}

pub struct RuntimeSnapshotLease {
    pub snapshot: Arc<RuntimeExtensionSnapshot>,
    pub retirement: Arc<GenerationRetirement>,
    pub manager: std::sync::Weak<crate::extension::ExtensionManager>,
}

impl Drop for RuntimeSnapshotLease {
    fn drop(&mut self) {
        if let Some(manager) = self.manager.upgrade() {
            manager.release_lease(self.snapshot.generation);
        }
    }
}

pub struct HostApprovalBroker {
    pending_approvals: std::sync::Mutex<
        std::collections::HashMap<
            String,
            tokio::sync::oneshot::Sender<gestalt_core::approval::ApprovalDecision>,
        >,
    >,
}

impl HostApprovalBroker {
    pub fn new() -> Self {
        Self {
            pending_approvals: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn register(
        &self,
        id: String,
        tx: tokio::sync::oneshot::Sender<gestalt_core::approval::ApprovalDecision>,
    ) {
        self.pending_approvals.lock().unwrap().insert(id, tx);
    }

    pub fn respond(
        &self,
        id: &str,
        decision: gestalt_core::approval::ApprovalDecision,
    ) -> Result<()> {
        let mut pending = self.pending_approvals.lock().unwrap();
        if let Some(tx) = pending.remove(id) {
            let _ = tx.send(decision);
            Ok(())
        } else {
            Err(crate::error::RuntimeError::Orchestration(format!(
                "No pending approval found with ID: {}",
                id
            )))
        }
    }
}

impl Default for HostApprovalBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl gestalt_core::approval::ApprovalProvider for HostApprovalBroker {
    async fn approve(
        &self,
        request: gestalt_core::approval::ApprovalRequest,
    ) -> std::result::Result<
        gestalt_core::approval::ApprovalDecision,
        gestalt_core::error::HarnessError,
    > {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let id = request.tool_call_id.clone();
        self.register(id.clone(), tx);
        rx.await
            .map_err(|_| gestalt_core::error::HarnessError::Cancelled)
    }
}

impl ExtensionActivationPipeline {
    pub async fn run(
        &self,
        request: ActivationRequest,
        manager: &Arc<crate::extension::ExtensionManager>,
    ) -> Result<ActivationCandidate> {
        let active = manager.active_snapshot();
        let candidate_generation = RuntimeGeneration(active.generation.0 + 1);

        // 1. Discover packages
        let discovered = self.discovery.discover_packages()?;
        let mut discovered = discovered;
        crate::extension::apply_trust_decisions(
            &mut discovered,
            &self.host_context.trusted_extension_ids,
        );

        // 2. Resolve configured instances or targeted instance
        let mut final_resolved_packages = Vec::new();
        if let Some(target) = &request.target_instance {
            if !self.host_context.extension_instances.contains_key(target) {
                return Err(crate::error::RuntimeError::Extension(format!(
                    "Unknown instance ID: {}",
                    target
                )));
            }

            let target_config = self.host_context.extension_instances.get(target).unwrap();
            let discovered_target_package = discovered
                .iter()
                .find(|p| p.descriptor.id == target_config.package)
                .cloned();

            let resolved_target = if let Some(template) = discovered_target_package {
                let mut package = template.with_instance(
                    target.clone(),
                    target_config.config.clone(),
                    target_config.grants.clone(),
                );
                package.components.retain(|component| {
                    target_config
                        .components
                        .get(&component.id.component_id)
                        .copied()
                        .unwrap_or(true)
                });
                if package.components.is_empty() {
                    return Err(crate::error::RuntimeError::Extension(format!(
                        "Configured extension instance '{}' disabled all components",
                        target
                    )));
                }
                Some(package)
            } else {
                return Err(crate::error::RuntimeError::Extension(format!(
                    "Discovered package not found for configured instance '{}' (package: '{}')",
                    target, target_config.package
                )));
            };

            // Retain all other instances from active composition
            let mut found_target_in_active = false;
            for active_package in active.resolved_packages.iter() {
                if &active_package.instance_id == target {
                    if let Some(ref target_package) = resolved_target {
                        final_resolved_packages.push(target_package.clone());
                    }
                    found_target_in_active = true;
                } else {
                    final_resolved_packages.push(active_package.clone());
                }
            }
            if !found_target_in_active {
                if let Some(target_package) = resolved_target {
                    final_resolved_packages.push(target_package);
                }
            }
        } else {
            // no target: rediscover and rebuild full composition
            final_resolved_packages = crate::extension::resolve_configured_instances(
                &discovered,
                &self.host_context.extension_instances,
            )?;
        }

        // 3. Namespace validation
        let mut tool_runtime_names = std::collections::HashSet::new();
        let mut canonical_tool_ids = std::collections::HashSet::new();
        let mut mcp_server_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut context_source_ids = std::collections::HashSet::new();
        let mut lifecycle_capability_ids = std::collections::HashSet::new();
        let mut skill_ids = std::collections::HashSet::new();
        let mut client_descriptor_ids = std::collections::HashSet::new();

        for tool_schema in self.base_composition.tool_catalog.schemas() {
            if let Some(name) = tool_schema.get("name").and_then(|n| n.as_str()) {
                tool_runtime_names.insert(name.to_string());
            }
        }

        for package in &final_resolved_packages {
            for component in &package.components {
                match component.kind {
                    crate::extension::ComponentKind::CommandTool => {
                        let runtime_name = format!(
                            "{}__{}",
                            component.id.instance_id, component.id.component_id
                        );
                        if !tool_runtime_names.insert(runtime_name.clone()) {
                            return Err(crate::error::RuntimeError::Extension(format!(
                                "Duplicate tool runtime name collision: {}",
                                runtime_name
                            )));
                        }
                        let canonical_id = format!(
                            "extension:{}@{}/{}",
                            component.id.package_id,
                            component.id.instance_id,
                            component.id.component_id
                        );
                        if !canonical_tool_ids.insert(canonical_id.clone()) {
                            return Err(crate::error::RuntimeError::Extension(format!(
                                "Duplicate canonical tool ID collision: {}",
                                canonical_id
                            )));
                        }
                    }
                    crate::extension::ComponentKind::LegacyProcess => {
                        for tool in &component.tools {
                            if !tool_runtime_names.insert(tool.name.clone()) {
                                return Err(crate::error::RuntimeError::Extension(format!(
                                    "Duplicate tool runtime name collision: {}",
                                    tool.name
                                )));
                            }
                        }
                        for injector in &component.context_injectors {
                            if !context_source_ids.insert(injector.name.clone()) {
                                return Err(crate::error::RuntimeError::Extension(format!(
                                    "Duplicate context contributor name collision: {}",
                                    injector.name
                                )));
                            }
                        }
                    }
                    #[cfg(feature = "mcp")]
                    crate::extension::ComponentKind::McpServer => {
                        let mcp_name = crate::extension::package_mcp_server_name(
                            &component.id.package_id,
                            &component.id.instance_id,
                            &component.id.component_id,
                        );
                        if !mcp_server_names.insert(mcp_name.clone()) {
                            return Err(crate::error::RuntimeError::Extension(format!(
                                "Duplicate MCP server name collision: {}",
                                mcp_name
                            )));
                        }
                    }
                    #[cfg(not(feature = "mcp"))]
                    crate::extension::ComponentKind::McpServer => {}
                    crate::extension::ComponentKind::GestaltLifecycle => {
                        let canonical_comp_id = component.id.canonical_id();
                        if !lifecycle_capability_ids.insert(canonical_comp_id.clone()) {
                            return Err(crate::error::RuntimeError::Extension(format!(
                                "Duplicate lifecycle component ID collision: {}",
                                canonical_comp_id
                            )));
                        }
                    }
                    crate::extension::ComponentKind::Skill => {
                        let skill_id = component.id.canonical_id();
                        if !skill_ids.insert(skill_id.clone()) {
                            return Err(crate::error::RuntimeError::Extension(format!(
                                "Duplicate skill ID collision: {}",
                                skill_id
                            )));
                        }
                    }
                    crate::extension::ComponentKind::ClientProduct => {
                        let client_id = component.id.canonical_id();
                        if !client_descriptor_ids.insert(client_id.clone()) {
                            return Err(crate::error::RuntimeError::Extension(format!(
                                "Duplicate client descriptor ID collision: {}",
                                client_id
                            )));
                        }
                    }
                }
            }
        }

        // 4. Resource activation (reuse vs newly_started)
        let mut newly_started = Vec::new();
        let mut reused = Vec::new();
        let mut diagnostics = Vec::new();
        let mut lifecycle_clients: std::collections::HashMap<
            String,
            Arc<dyn crate::lifecycle::LifecycleClient>,
        > = std::collections::HashMap::new();

        let mut context_registrations = Vec::new();
        let mut policy_registrations = Vec::new();
        let mut routing_registrations = Vec::new();
        let mut verification_registrations = Vec::new();
        let mut observer_registrations = Vec::new();

        let mut registry_builder = crate::registry::RuntimeRegistryBuilder::new();
        // Seed registry builder with base registry
        registry_builder.tools = self
            .base_composition
            .base_registry
            .tools
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    crate::registry::ToolMetadata {
                        name: v.name.clone(),
                        schema: v.schema.clone(),
                        schema_hash: v.schema_hash.clone(),
                        tool: v.tool.clone(),
                        extension_id: v.extension_id.clone(),
                    },
                )
            })
            .collect();
        registry_builder.context_contributors = self
            .base_composition
            .base_registry
            .context_contributors
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    crate::registry::ContextContributorMetadata {
                        name: v.name.clone(),
                        contributor: v.contributor.clone(),
                        stability: v.stability,
                        extension_id: v.extension_id.clone(),
                    },
                )
            })
            .collect();
        registry_builder.extensions = self.base_composition.base_registry.extensions.clone();

        for package in &final_resolved_packages {
            let extension_identity = format!("{}@{}", package.descriptor.id, package.instance_id);
            if !registry_builder.extensions.contains(&extension_identity) {
                registry_builder.extensions.push(extension_identity.clone());
            }

            for component in &package.components {
                let runtime_component = package
                    .to_runtime_component(&component.id.component_id)
                    .ok_or_else(|| {
                        crate::error::RuntimeError::Extension(format!(
                            "Failed to map component '{}' to ExtensionRuntimeComponent",
                            component.id.canonical_id()
                        ))
                    })?;
                let reuse_key = runtime_component.reuse_key();

                let mut reused_resource = None;
                if !request.force {
                    if let Some(active_package) = active.resolved_packages.iter().find(|p| {
                        p.instance_id == package.instance_id
                            && p.descriptor.id == package.descriptor.id
                    }) {
                        if let Some(active_comp) =
                            active_package.to_runtime_component(&component.id.component_id)
                        {
                            if active_comp.reuse_key() == reuse_key {
                                if let Some(res) = active
                                    .managed_resources
                                    .iter()
                                    .find(|r| r.id() == component.id.canonical_id())
                                {
                                    reused_resource = Some(res.clone());
                                }
                            }
                        }
                    }
                }

                match component.kind {
                    crate::extension::ComponentKind::CommandTool => {
                        let tool = Arc::new(crate::extension::CommandTool::from_component(
                            component,
                            self.host_context.workspace_root.clone(),
                            self.host_context.event_bus.clone(),
                        )?);
                        registry_builder.register_executable_tool(
                            tool.name().to_string(),
                            tool.schema(),
                            tool,
                            Some(extension_identity.clone()),
                        )?;
                    }
                    crate::extension::ComponentKind::LegacyProcess => {
                        let process = if let Some(ManagedExtensionResource::Process {
                            process: p,
                            ..
                        }) = reused_resource
                        {
                            reused.push(ManagedExtensionResource::Process {
                                reuse_key: reuse_key.clone(),
                                process: p.clone(),
                            });
                            p
                        } else {
                            let launch_result = manager
                                .launch_process(&runtime_component, &self.host_context)
                                .await;
                            match launch_result {
                                Ok(p) => {
                                    newly_started.push(ManagedExtensionResource::Process {
                                        reuse_key: reuse_key.clone(),
                                        process: p.clone(),
                                    });
                                    p
                                }
                                Err(e) => {
                                    if component.optional {
                                        diagnostics.push(ActivationDiagnostic {
                                            component_id: component.id.clone(),
                                            severity: DiagnosticSeverity::Warning,
                                            message: format!("Optional legacy process component failed to launch: {}", e),
                                        });
                                        continue;
                                    } else {
                                        for res in &newly_started {
                                            if let ManagedExtensionResource::Process {
                                                process: p,
                                                ..
                                            } = res
                                            {
                                                p.transition_to(crate::extension::ExtensionProcessState::Stopping);
                                                p.shutdown().await;
                                            }
                                        }
                                        return Err(e);
                                    }
                                }
                            }
                        };

                        if let Some(broker) = process.broker() {
                            let process_ext =
                                Arc::new(crate::process_extension::ProcessExtension::new(
                                    crate::manifest::ExtensionManifest {
                                        id: component.id.package_id.clone(),
                                        name: component.id.canonical_id(),
                                        version: package.descriptor.version.clone(),
                                        manifest_version: Some("1".to_string()),
                                        protocol_version: component.protocol_version.clone(),
                                        runtime: "stdio".to_string(),
                                        entrypoint: component.entrypoint.clone(),
                                        capabilities: crate::manifest::Capabilities {
                                            tools: !component.tools.is_empty(),
                                            hooks: !component.hooks.is_empty(),
                                            context: !component.context_injectors.is_empty(),
                                            supports_cancellation: component.supports_cancellation,
                                        },
                                        permissions: component.permissions.clone(),
                                        tools: component.tools.clone(),
                                        hooks: component.hooks.clone(),
                                        context_injectors: component.context_injectors.clone(),
                                    },
                                    broker,
                                ));
                            use crate::extension::GestaltExtension;
                            process_ext.register(&mut registry_builder)?;
                        }
                    }
                    crate::extension::ComponentKind::GestaltLifecycle => {
                        let client: Arc<dyn crate::lifecycle::LifecycleClient> =
                            Arc::new(crate::lifecycle::ProcessLifecycleClient::new(
                                manager.clone(),
                                runtime_component.clone(),
                                self.host_context.clone(),
                            ));

                        if let Some(ManagedExtensionResource::Process { process: p, .. }) =
                            reused_resource
                        {
                            reused.push(ManagedExtensionResource::Process {
                                reuse_key: reuse_key.clone(),
                                process: p.clone(),
                            });
                        } else {
                            let launch_result = manager
                                .launch_process(&runtime_component, &self.host_context)
                                .await;
                            match launch_result {
                                Ok(p) => {
                                    newly_started.push(ManagedExtensionResource::Process {
                                        reuse_key: reuse_key.clone(),
                                        process: p.clone(),
                                    });
                                }
                                Err(e) => {
                                    if component.optional {
                                        diagnostics.push(ActivationDiagnostic {
                                            component_id: component.id.clone(),
                                            severity: DiagnosticSeverity::Warning,
                                            message: format!(
                                                "Optional lifecycle component failed to launch: {}",
                                                e
                                            ),
                                        });
                                        continue;
                                    } else {
                                        for res in &newly_started {
                                            if let ManagedExtensionResource::Process {
                                                process: p,
                                                ..
                                            } = res
                                            {
                                                p.transition_to(crate::extension::ExtensionProcessState::Stopping);
                                                p.shutdown().await;
                                            }
                                        }
                                        return Err(e);
                                    }
                                }
                            }
                        }

                        lifecycle_clients.insert(component.id.canonical_id(), client.clone());

                        if !request.force
                            && active.resolved_packages.iter().any(|p| {
                                p.instance_id == package.instance_id
                                    && p.descriptor.id == package.descriptor.id
                                    && p.to_runtime_component(&component.id.component_id)
                                        .is_some_and(|ac| ac.reuse_key() == reuse_key)
                            })
                        {
                            let canonical_id = component.id.canonical_id();
                            for reg in active.context_plan.registrations.iter() {
                                if reg.descriptor.component_id == canonical_id {
                                    context_registrations.push(reg.clone());
                                }
                            }
                            for reg in active.policy_plan.registrations.iter() {
                                if reg.descriptor.component_id == canonical_id {
                                    policy_registrations.push(reg.clone());
                                }
                            }
                            for reg in active.routing_plan.registrations.iter() {
                                if reg.descriptor.component_id == canonical_id {
                                    routing_registrations.push(reg.clone());
                                }
                            }
                            for reg in active.verification_plan.registrations.iter() {
                                if reg.descriptor.component_id == canonical_id {
                                    verification_registrations.push(reg.clone());
                                }
                            }
                            for reg in active.observer_plan.registrations.iter() {
                                if reg.descriptor.component_id == canonical_id {
                                    observer_registrations.push(reg.clone());
                                }
                            }
                        } else {
                            let init_req = crate::lifecycle::InitializeRequestV2 {
                                supported_versions: vec!["2.0".to_string()],
                            };
                            match client.initialize(init_req).await {
                                Ok(_) => match client.describe_capabilities().await {
                                    Ok(capabilities) => {
                                        for cap in capabilities {
                                            let failure_mode = match cap.failure_mode.as_str() {
                                                    "fail_closed" | "FailClosed" => crate::lifecycle::CapabilityFailureMode::FailClosed,
                                                    "fail_open" | "FailOpen" => crate::lifecycle::CapabilityFailureMode::FailOpen,
                                                    "ignore" | "Ignore" => crate::lifecycle::CapabilityFailureMode::Ignore,
                                                    _ => match cap.capability {
                                                        crate::lifecycle::LifecycleCapabilityKind::ContextProvider | crate::lifecycle::LifecycleCapabilityKind::EventObserver => {
                                                            crate::lifecycle::CapabilityFailureMode::FailOpen
                                                        }
                                                        _ => crate::lifecycle::CapabilityFailureMode::FailClosed,
                                                    }
                                                };
                                            let data_scope = match cap.data_scope.as_str() {
                                                    "none" | "None" => crate::lifecycle::CapabilityDataScope::None,
                                                    "tool_request" | "ToolRequest" => crate::lifecycle::CapabilityDataScope::ToolRequest,
                                                    "current_turn" | "CurrentTurn" => crate::lifecycle::CapabilityDataScope::CurrentTurn,
                                                    "projected_context" | "ProjectedContext" => crate::lifecycle::CapabilityDataScope::ProjectedContext,
                                                    "runtime_event" | "RuntimeEvent" => crate::lifecycle::CapabilityDataScope::RuntimeEvent,
                                                    _ => crate::lifecycle::CapabilityDataScope::None,
                                                };
                                            let descriptor =
                                                crate::lifecycle::TypedCapabilityDescriptor {
                                                    component_id: cap.component_id.clone(),
                                                    priority: cap.priority,
                                                    timeout: std::time::Duration::from_millis(
                                                        cap.timeout_ms,
                                                    ),
                                                    failure_mode,
                                                    data_scope,
                                                };
                                            match cap.capability {
                                                    crate::lifecycle::LifecycleCapabilityKind::ContextProvider => {
                                                        context_registrations.push(crate::lifecycle::ContextProviderRegistration {
                                                            descriptor,
                                                            stability: gestalt_core::ContextStability::TurnDynamic,
                                                            source: component.id.canonical_id(),
                                                        });
                                                    }
                                                    crate::lifecycle::LifecycleCapabilityKind::PolicyGuard => {
                                                        policy_registrations.push(crate::lifecycle::PolicyGuardRegistration {
                                                            descriptor,
                                                            source: component.id.canonical_id(),
                                                        });
                                                    }
                                                    crate::lifecycle::LifecycleCapabilityKind::TurnRouter => {
                                                        routing_registrations.push(crate::lifecycle::TurnRouterRegistration {
                                                            descriptor,
                                                            source: component.id.canonical_id(),
                                                        });
                                                    }
                                                    crate::lifecycle::LifecycleCapabilityKind::Verifier => {
                                                        verification_registrations.push(crate::lifecycle::ExternalVerifierRegistration {
                                                            descriptor,
                                                            source: component.id.canonical_id(),
                                                        });
                                                    }
                                                    crate::lifecycle::LifecycleCapabilityKind::EventObserver => {
                                                        observer_registrations.push(crate::lifecycle::EventObserverRegistration {
                                                            descriptor,
                                                            source: component.id.canonical_id(),
                                                        });
                                                    }
                                                }
                                        }
                                    }
                                    Err(e) => {
                                        if component.optional {
                                            diagnostics.push(ActivationDiagnostic {
                                                    component_id: component.id.clone(),
                                                    severity: DiagnosticSeverity::Warning,
                                                    message: format!("Optional lifecycle component failed to describe capabilities: {}", e),
                                                });
                                        } else {
                                            for res in &newly_started {
                                                if let ManagedExtensionResource::Process {
                                                    process: p,
                                                    ..
                                                } = res
                                                {
                                                    p.transition_to(crate::extension::ExtensionProcessState::Stopping);
                                                    p.shutdown().await;
                                                }
                                            }
                                            return Err(e);
                                        }
                                    }
                                },
                                Err(e) => {
                                    if component.optional {
                                        diagnostics.push(ActivationDiagnostic {
                                            component_id: component.id.clone(),
                                            severity: DiagnosticSeverity::Warning,
                                            message: format!("Optional lifecycle component failed to initialize: {}", e),
                                        });
                                    } else {
                                        for res in &newly_started {
                                            if let ManagedExtensionResource::Process {
                                                process: p,
                                                ..
                                            } = res
                                            {
                                                p.transition_to(crate::extension::ExtensionProcessState::Stopping);
                                                p.shutdown().await;
                                            }
                                        }
                                        return Err(e);
                                    }
                                }
                            }
                        }
                    }
                    #[cfg(feature = "mcp")]
                    crate::extension::ComponentKind::McpServer => {
                        let mcp_name = crate::extension::package_mcp_server_name(
                            &component.id.package_id,
                            &component.id.instance_id,
                            &component.id.component_id,
                        );
                        let mcp_res = Arc::new(ManagedMcpServer {
                            name: mcp_name.clone(),
                        });
                        if reused_resource.is_some() {
                            reused.push(ManagedExtensionResource::Mcp {
                                reuse_key: reuse_key.clone(),
                                server: mcp_res,
                            });
                        } else {
                            newly_started.push(ManagedExtensionResource::Mcp {
                                reuse_key: reuse_key.clone(),
                                server: mcp_res,
                            });
                        }
                    }
                    #[cfg(not(feature = "mcp"))]
                    crate::extension::ComponentKind::McpServer => {
                        return Err(crate::error::RuntimeError::Extension(
                            "MCP feature is disabled".to_string()
                        ));
                    }
                    _ => {}
                }
            }
        }

        // 5. Rebuild MCP registry config
        // 5. Rebuild MCP registry config
        #[cfg(feature = "mcp")]
        let direct_mcp_fingerprint = {
            let sorted_direct_mcp: std::collections::BTreeMap<
                String,
                crate::mcp::McpServerConfig,
            > = self
                .host_context
                .mcp_servers
                .iter()
                .map(|(name, config)| (name.clone(), config.clone()))
                .collect();
            fingerprint_json(&sorted_direct_mcp)
        };
        #[cfg(not(feature = "mcp"))]
        let direct_mcp_fingerprint = String::new();

        #[cfg(feature = "mcp")]
        let mcp_registry = {
            let mut mcp_servers_config = self.host_context.mcp_servers.clone();
            for package in &final_resolved_packages {
                for (name, config) in crate::extension::package_mcp_servers(package)? {
                    mcp_servers_config.insert(name, config);
                }
            }

            let mcp_registry = Arc::new(crate::mcp::McpRegistry::new(
                self.host_context.workspace_root.clone(),
                mcp_servers_config,
            ));

            let mut package_permissions = std::collections::HashMap::new();
            for package in &final_resolved_packages {
                for component in &package.components {
                    if component.kind == crate::extension::ComponentKind::McpServer {
                        let server_name = crate::extension::package_mcp_server_name(
                            &component.id.package_id,
                            &component.id.instance_id,
                            &component.id.component_id,
                        );
                        package_permissions.insert(
                            server_name,
                            (component.permissions.clone(), component.grants.clone()),
                        );
                    }
                }
            }

            let event_bus = self.host_context.event_bus.clone();
            let allow_network = self.host_context.allow_network;
            mcp_registry.set_permission_validator(move |name, config| {
                match &config.transport {
                    crate::mcp::McpTransportConfig::Stdio { .. } => {
                        if let Some((permissions, grants)) = package_permissions.get(name) {
                            crate::permissions::check_shell_permission_effective(
                                permissions,
                                Some(grants),
                                &event_bus,
                                name,
                            )
                            .map_err(|e| e.clone())?;
                        }
                    }
                    crate::mcp::McpTransportConfig::Http { url, .. } => {
                        let host = if let Ok(parsed_url) = url::Url::parse(url) {
                            parsed_url.host_str().unwrap_or("").to_string()
                        } else {
                            url.clone()
                        };
                        if let Some((permissions, grants)) = package_permissions.get(name) {
                            crate::permissions::check_network_permission_effective(
                                permissions,
                                Some(grants),
                                allow_network,
                                &host,
                                &event_bus,
                                name,
                            )
                            .map_err(|e| e.clone())?;
                        } else if !allow_network {
                            return Err(format!(
                                "Network access to host '{host}' is not allowed by host policy"
                            ));
                        }
                    }
                }
                Ok(())
            });

            let event_bus = self.host_context.event_bus.clone();
            mcp_registry.set_event_callback(Arc::new(move |event| match event {
                crate::mcp::McpRegistryEvent::Connecting { server_name } => {
                    event_bus
                        .publish(crate::event_bus::RuntimeEvent::McpServerConnecting { server_name });
                }
                crate::mcp::McpRegistryEvent::Connected {
                    server_name,
                    protocol_version,
                    tool_count,
                } => {
                    event_bus.publish(crate::event_bus::RuntimeEvent::McpServerConnected {
                        server_name,
                        protocol_version,
                        tool_count,
                    });
                }
                crate::mcp::McpRegistryEvent::ConnectionFailed {
                    server_name,
                    reason,
                } => {
                    event_bus.publish(crate::event_bus::RuntimeEvent::McpServerConnectionFailed {
                        server_name,
                        reason,
                    });
                }
                crate::mcp::McpRegistryEvent::ToolCatalogRefreshed {
                    server_name,
                    tool_count,
                    schema_hash,
                } => {
                    event_bus.publish(crate::event_bus::RuntimeEvent::McpToolCatalogRefreshed {
                        server_name,
                        tool_count,
                        schema_hash,
                    });
                }
                crate::mcp::McpRegistryEvent::ToolListChanged { server_name } => {
                    event_bus
                        .publish(crate::event_bus::RuntimeEvent::McpToolListChanged { server_name });
                }
            }));
            mcp_registry
        };

        // 6. Build composed tool catalog
        let mut extension_tools = std::collections::BTreeMap::new();
        for (name, metadata) in &registry_builder.tools {
            if let Some(ref tool) = metadata.tool {
                extension_tools.insert(name.clone(), tool.clone());
            }
        }

        let mut composed_tools = crate::tool_catalog::ComposedToolCatalog::new(
            self.base_composition.tool_catalog.clone(),
            extension_tools,
        )
        .map_err(crate::error::RuntimeError::Registry)?;
        #[cfg(feature = "mcp")]
        {
            composed_tools = composed_tools.with_mcp(mcp_registry.clone());
        }
        let composed_tools = composed_tools.with_event_bus(self.host_context.event_bus.clone());

        // 7. Construct candidate snapshot
        let mut complete_fp = crate::extension::compute_complete_fingerprint(
            &registry_builder.snapshot().fingerprint.0,
            &final_resolved_packages,
        );
        complete_fp = fingerprint_join(&complete_fp, &direct_mcp_fingerprint);

        let mut all_managed_resources = Vec::new();
        all_managed_resources.extend(newly_started.clone());
        all_managed_resources.extend(reused.clone());

        let mut process_instances = Vec::new();
        for res in &all_managed_resources {
            if let ManagedExtensionResource::Process { process: p, .. } = res {
                process_instances.push(p.clone());
            }
        }

        let mut negotiated_protocols = std::collections::HashMap::new();
        for res in &all_managed_resources {
            if let ManagedExtensionResource::Process { process: p, .. } = res {
                if let Some(broker) = p.broker() {
                    negotiated_protocols
                        .insert(p.component_id.clone(), broker.negotiated_version());
                }
            }
        }

        // Calculate Diff
        let mut diff = ExtensionGenerationDiff::default();
        {
            let mut current_components = std::collections::HashMap::new();
            for package in active.resolved_packages.iter() {
                for component in &package.components {
                    if let Some(runtime_comp) =
                        package.to_runtime_component(&component.id.component_id)
                    {
                        current_components.insert(component.id.clone(), runtime_comp.reuse_key().1);
                    }
                }
            }

            let mut candidate_components = std::collections::HashMap::new();
            for package in &final_resolved_packages {
                for component in &package.components {
                    if let Some(runtime_comp) =
                        package.to_runtime_component(&component.id.component_id)
                    {
                        candidate_components
                            .insert(component.id.clone(), runtime_comp.reuse_key().1);
                    }
                }
            }

            for (id, fp) in &candidate_components {
                if let Some(old_fp) = current_components.get(id) {
                    if old_fp == fp {
                        diff.reused.push(id.clone());
                    } else {
                        diff.replaced.push(id.clone());
                    }
                } else {
                    diff.added.push(id.clone());
                }
            }

            for id in current_components.keys() {
                if !candidate_components.contains_key(id) {
                    diff.removed.push(id.clone());
                }
            }
        }

        let snapshot = Arc::new(RuntimeExtensionSnapshot {
            generation: candidate_generation,
            fingerprint: crate::registry::RuntimeFingerprint(complete_fp),
            registry_snapshot: registry_builder.snapshot(),
            tool_catalog: Arc::new(composed_tools),
            context_plan: Arc::new(crate::lifecycle::ContextProviderPlan::new(
                context_registrations,
            )),
            policy_plan: Arc::new(crate::lifecycle::PolicyGuardPlan::new(policy_registrations)),
            routing_plan: Arc::new(crate::lifecycle::TurnRouterPlan::new(routing_registrations)),
            verification_plan: Arc::new(crate::lifecycle::ExternalVerifierPlan::new(
                verification_registrations,
            )),
            observer_plan: Arc::new(crate::lifecycle::EventObserverPlan::new(
                observer_registrations,
            )),
            #[cfg(feature = "mcp")]
            mcp_registry,
            process_instances: Arc::from(process_instances),
            package_health: Arc::from([]),
            diagnostics: Arc::from(diagnostics.clone()),
            managed_resources: Arc::from(all_managed_resources),
            negotiated_protocol: Arc::new(negotiated_protocols),
            lifecycle_clients: Arc::new(lifecycle_clients),
            resolved_packages: Arc::from(final_resolved_packages),
        });

        Ok(ActivationCandidate {
            snapshot,
            diff,
            newly_started,
            reused,
            diagnostics,
            committed: false,
        })
    }
}

fn fingerprint_json<T: serde::Serialize>(value: &T) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    match serde_json::to_vec(value) {
        Ok(serialized) => hasher.update(serialized),
        Err(err) => hasher.update(err.to_string().as_bytes()),
    }
    format!("{:x}", hasher.finalize())
}

fn fingerprint_join(left: &str, right: &str) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(left.as_bytes());
    hasher.update(b"|");
    hasher.update(right.as_bytes());
    format!("{:x}", hasher.finalize())
}
