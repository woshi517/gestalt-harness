use gestalt_core::{
    approval::ApprovalProvider, context::ContextPipeline, policy::PolicyEngine, provider::Provider,
    tool::ToolCatalog, trace::TraceSink, HookRegistry,
};
use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::error::{Result, RuntimeError};
use crate::registry::RuntimeRegistryBuilder;
use crate::runtime::AgentRuntime;

use crate::composition_hooks::CompositionHooks;
use crate::event_bus::RuntimeEventBus;
use crate::extension::RuntimeModule;

#[derive(Clone)]
pub struct AgentRuntimeBuilder {
    pub provider: Option<Arc<dyn Provider>>,
    pub tools: Option<Arc<dyn ToolCatalog>>,
    pub middleware: Option<Arc<dyn ContextPipeline>>,
    pub assembler: Option<Arc<dyn gestalt_core::context::ContextAssembler>>,
    pub policy: Option<Arc<dyn PolicyEngine>>,
    pub approval: Option<Arc<dyn ApprovalProvider>>,
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    pub config: RuntimeConfig,
    pub hooks: HookRegistry,
    pub registry: RuntimeRegistryBuilder,
    pub composition_hooks: Option<Arc<dyn CompositionHooks>>,
    pub runtime_modules: Vec<Arc<dyn RuntimeModule>>,
    pub extension_packages: Vec<crate::extension::ResolvedExtensionPackage>,
    pub extension_manager: Option<Arc<crate::extension::ExtensionManager>>,
    pub event_bus: RuntimeEventBus,
    #[cfg(feature = "mcp")]
    pub mcp_registry: Option<Arc<crate::mcp::McpRegistry>>,
    pub workspace_context_snapshot: Option<crate::workspace_context::WorkspaceContextSnapshot>,
}

impl Default for AgentRuntimeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentRuntimeBuilder {
    pub fn new() -> Self {
        Self {
            provider: None,
            tools: None,
            middleware: None,
            assembler: None,
            policy: None,
            approval: None,
            trace_sink: None,
            config: RuntimeConfig::default(),
            hooks: HookRegistry::default(),
            registry: RuntimeRegistryBuilder::new(),
            composition_hooks: None,
            runtime_modules: Vec::new(),
            extension_packages: Vec::new(),
            extension_manager: None,
            event_bus: RuntimeEventBus::new(),
            #[cfg(feature = "mcp")]
            mcp_registry: None,
            workspace_context_snapshot: None,
        }
    }

    pub fn workspace_context_snapshot(
        mut self,
        snapshot: crate::workspace_context::WorkspaceContextSnapshot,
    ) -> Self {
        self.workspace_context_snapshot = Some(snapshot);
        self
    }

    pub fn composition_hooks(mut self, hooks: Arc<dyn CompositionHooks>) -> Self {
        self.composition_hooks = Some(hooks);
        self
    }

    pub fn runtime_module(mut self, module: Arc<dyn RuntimeModule>) -> Self {
        self.runtime_modules.push(module);
        self
    }

    pub fn extension_package(
        mut self,
        package: crate::extension::ResolvedExtensionPackage,
    ) -> Self {
        self.extension_packages.push(package);
        self
    }

    pub fn provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn tools(mut self, tools: Arc<dyn ToolCatalog>) -> Self {
        self.tools = Some(tools);
        self
    }

    #[deprecated(
        since = "0.1.0",
        note = "Use assembler(Arc<dyn ContextAssembler>) instead"
    )]
    pub fn middleware(mut self, middleware: Arc<dyn ContextPipeline>) -> Self {
        self.middleware = Some(middleware);
        self
    }

    pub fn assembler(
        mut self,
        assembler: Arc<dyn gestalt_core::context::ContextAssembler>,
    ) -> Self {
        self.assembler = Some(assembler);
        self
    }

    pub fn policy(mut self, policy: Arc<dyn PolicyEngine>) -> Self {
        self.policy = Some(policy);
        self
    }

    pub fn approval(mut self, approval: Arc<dyn ApprovalProvider>) -> Self {
        self.approval = Some(approval);
        self
    }

    pub fn trace_sink(mut self, trace_sink: Arc<dyn TraceSink>) -> Self {
        self.trace_sink = Some(trace_sink);
        self
    }

    pub fn config(mut self, config: RuntimeConfig) -> Self {
        self.config = config;
        self
    }

    #[cfg(feature = "mcp")]
    pub fn mcp_registry(mut self, registry: Arc<crate::mcp::McpRegistry>) -> Self {
        self.mcp_registry = Some(registry);
        self
    }

    pub fn hooks(mut self, hooks: HookRegistry) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn build(self) -> Result<AgentRuntime> {
        self.build_inner()
    }

    fn build_inner(mut self) -> Result<AgentRuntime> {
        if self.config.max_turns == 0 {
            return Err(RuntimeError::Builder(
                "max_turns must be positive".to_string(),
            ));
        }

        if let Some(policy) = &self.config.context_management_policy {
            policy
                .validate()
                .map_err(|err| RuntimeError::Builder(err.to_string()))?;
        }

        if self.assembler.is_none()
            && self
                .middleware
                .as_ref()
                .is_some_and(|middleware| middleware.as_assembler().is_none())
        {
            return Err(RuntimeError::Builder(
                "runtime requires an assembler-backed context pipeline; use AgentRuntimeBuilder::assembler(...) or a pipeline that implements as_assembler()".to_string(),
            ));
        }

        let resolved_extension_packages = crate::extension::resolve_configured_instances(
            &self.extension_packages,
            &self.config.extension_instances,
        )?;
        let mut resolved_extension_packages = resolved_extension_packages;
        crate::extension::apply_trust_decisions(
            &mut resolved_extension_packages,
            &self.config.trusted_extension_ids,
            &self.config.trusted_extension_pins,
        );

        for module in &self.runtime_modules {
            let name = module.id().to_string();
            if self.registry.extensions.contains(&name) {
                return Err(RuntimeError::Registry(format!(
                    "Duplicate extension name: {}",
                    name
                )));
            }
            module.register(&mut self.registry).map_err(|e| {
                RuntimeError::Extension(format!(
                    "Runtime module '{}' failed to register: {}",
                    name, e
                ))
            })?;
            self.registry.register_extension(name)?;
        }

        // Register skill context contributors if skills are configured
        #[cfg(feature = "skills")]
        let skill_state_handle = if self.config.discovered_skills.is_empty() {
            None
        } else {
            let skill_state = Arc::new(std::sync::Mutex::new(
                crate::skills::contributor::SkillContributorState::new(
                    self.config.discovered_skills.clone(),
                    self.config.active_skills.clone(),
                )
                .with_event_bus(self.event_bus.clone()),
            ));
            let _ = self.registry.register_context_contributor(
                "available_skills".to_string(),
                Arc::new(crate::skills::contributor::AvailableSkillsContributor::new(
                    skill_state.clone(),
                )),
            );
            let _ = self.registry.register_context_contributor(
                "active_skills".to_string(),
                Arc::new(crate::skills::contributor::ActiveSkillsContributor::new(
                    skill_state.clone(),
                )),
            );
            Some(skill_state)
        };
        // Initialize MCP Registry
        #[cfg(feature = "mcp")]
        let mcp_registry = {
            let mcp_registry = self.mcp_registry.clone().unwrap_or_else(|| {
                Arc::new(crate::mcp::McpRegistry::new(
                    self.config.workspace_root.clone(),
                    self.config.mcp_servers.clone(),
                ))
            });

            let mut package_permissions = std::collections::HashMap::new();
            for package in &resolved_extension_packages {
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

            let event_bus = self.event_bus.clone();
            let allow_network = self.config.allow_network;
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

            // Publish configuration events
            for (name, server_cfg) in &self.config.mcp_servers {
                self.event_bus
                    .publish(crate::event_bus::RuntimeEvent::McpServerConfigured {
                        server_name: name.clone(),
                        transport: format!("{:?}", server_cfg.transport),
                    });
            }

            // Wire event callback to propagate MCP Registry events to Runtime Event Bus
            let event_bus = self.event_bus.clone();
            mcp_registry.set_event_callback(Arc::new(move |event| match event {
                crate::mcp::McpRegistryEvent::Connecting { server_name } => {
                    event_bus.publish(crate::event_bus::RuntimeEvent::McpServerConnecting {
                        server_name,
                    });
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
                    event_bus.publish(crate::event_bus::RuntimeEvent::McpToolListChanged {
                        server_name,
                    });
                }
            }));

            // Spawn always_on servers
            for (name, server_cfg) in &self.config.mcp_servers {
                if server_cfg.lifecycle == crate::mcp::McpLifecycleMode::AlwaysOn {
                    let mcp_registry = mcp_registry.clone();
                    let name = name.clone();
                    tokio::spawn(async move {
                        let _ = mcp_registry.get_client(&name).await;
                    });
                }
            }

            mcp_registry
        };

        // Create MCP discovery state
        #[cfg(feature = "mcp")]
        let mcp_discovery_state =
            Arc::new(std::sync::Mutex::new(crate::mcp::McpDiscoveryState::new()));

        // Register MCP discovery tools
        #[cfg(feature = "mcp")]
        {
            self.registry.register_executable_tool(
                "search_tools".to_string(),
                serde_json::json!({
                    "name": "search_tools",
                    "description": "Search for available tools by keyword or description query.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "The query term or keywords to search for in tool names and descriptions."
                            }
                        },
                        "required": ["query"]
                    }
                }),
                Arc::new(crate::mcp::SearchToolsTool::new(mcp_registry.clone())),
                None,
            )?;

            self.registry.register_executable_tool(
                "get_tool_details".to_string(),
                serde_json::json!({
                    "name": "get_tool_details",
                    "description": "Inspect the detailed schema and arguments for a specific tool.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "The name or canonical ID of the tool to inspect."
                            }
                        },
                        "required": ["name"]
                    }
                }),
                Arc::new(crate::mcp::GetToolDetailsTool::new(
                    mcp_registry.clone(),
                    mcp_discovery_state.clone(),
                )),
                None,
            )?;
        }

        let provider = self
            .provider
            .ok_or_else(|| RuntimeError::Builder("Missing provider".to_string()))?;
        let base_tools = self
            .tools
            .ok_or_else(|| RuntimeError::Builder("Missing tools".to_string()))?;

        let mut extension_tools = std::collections::BTreeMap::new();
        for (name, metadata) in &self.registry.tools {
            if let Some(ref tool) = metadata.tool {
                extension_tools.insert(name.clone(), tool.clone());
            }
        }

        let composed_tools =
            crate::tool_catalog::ComposedToolCatalog::new(base_tools, extension_tools)
                .map_err(RuntimeError::Registry)?;
        #[cfg(feature = "mcp")]
        let composed_tools = composed_tools.with_mcp(mcp_registry.clone());
        let mut composed_tools = composed_tools.with_event_bus(self.event_bus.clone());

        let planner = self
            .config
            .tool_profile
            .clone()
            .map(crate::tool_catalog_planner::ToolCatalogPlanner::new);
        #[cfg(feature = "skills")]
        let mut planner = planner;
        #[cfg(feature = "skills")]
        if let Some(ref state) = skill_state_handle {
            planner = Some(match planner {
                Some(p) => p.with_skill_state(state.clone()),
                None => crate::tool_catalog_planner::ToolCatalogPlanner::new(
                    crate::tool_catalog_planner::ToolProfile::All,
                )
                .with_skill_state(state.clone()),
            });
        }

        // Configure MCP in planner
        #[cfg(feature = "mcp")]
        let mut planner = planner;
        #[cfg(feature = "mcp")]
        {
            planner = Some(match planner {
                Some(p) => p.with_mcp(
                    self.config.mcp_discovery_threshold,
                    mcp_discovery_state.clone(),
                    mcp_registry.clone(),
                ),
                None => crate::tool_catalog_planner::ToolCatalogPlanner::new(
                    crate::tool_catalog_planner::ToolProfile::All,
                )
                .with_mcp(
                    self.config.mcp_discovery_threshold,
                    mcp_discovery_state.clone(),
                    mcp_registry.clone(),
                ),
            });
        }

        if let Some(p) = planner {
            composed_tools = composed_tools.with_planner(p);
        }
        let composed_tools = Arc::new(composed_tools);

        let middleware = if let Some(assembler) = self.assembler {
            Arc::new(crate::context::RuntimeContextPipeline::new(assembler))
        } else {
            self.middleware.ok_or_else(|| {
                RuntimeError::Builder(
                    "Missing middleware/context pipeline or assembler".to_string(),
                )
            })?
        };
        let policy = self
            .policy
            .ok_or_else(|| RuntimeError::Builder("Missing policy engine".to_string()))?;
        let approval = self
            .approval
            .ok_or_else(|| RuntimeError::Builder("Missing approval provider".to_string()))?;

        let user_hooks = self.composition_hooks.take();
        let composed_hooks: Arc<dyn crate::composition_hooks::CompositionHooks> =
            Arc::new(crate::composition_hooks::ComposedCompositionHooks { user_hooks });

        let registry_snapshot = self.registry.snapshot();

        let mut runtime = AgentRuntime::new(
            provider,
            composed_tools,
            middleware,
            policy,
            approval,
            self.trace_sink,
            self.config,
            self.hooks,
            self.registry,
            registry_snapshot,
            Some(composed_hooks),
            self.event_bus,
            #[cfg(feature = "mcp")]
            mcp_registry,
            #[cfg(feature = "mcp")]
            mcp_discovery_state,
        );

        if let Some(extension_manager) = self.extension_manager {
            runtime.extension_snapshot = extension_manager.active_snapshot();
            runtime.extension_manager = extension_manager;
        } else {
            let host_context = crate::activation::HostLaunchContext::from_runtime_config(
                &runtime.config,
                runtime.event_bus.clone(),
            );
            let extension_manager = Arc::new(crate::extension::ExtensionManager::new(
                runtime.extension_snapshot.clone(),
                runtime.event_bus.clone(),
                Arc::new(crate::extension::LocalProcessLauncher),
                host_context.clone(),
            ));
            if !resolved_extension_packages.is_empty() {
                let pipeline = crate::activation::ExtensionActivationPipeline {
                    discovery: Arc::new(crate::activation::StaticExtensionSource::new(
                        resolved_extension_packages.clone(),
                    )),
                    launcher: Arc::new(crate::extension::LocalProcessLauncher),
                    base_composition: Arc::new(crate::activation::BaseRuntimeComposition {
                        tool_catalog: runtime.tools.clone(),
                        #[cfg(feature = "mcp")]
                        mcp_registry: runtime.mcp_registry.clone(),
                        base_registry: runtime.registry_snapshot.clone(),
                    }),
                    host_context,
                };
                let request = crate::activation::ActivationRequest {
                    current: Some(runtime.extension_snapshot.clone()),
                    target_instance: None,
                    force: false,
                    mode: crate::activation::ActivationMode::Commit,
                };
                let mut candidate = if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let manager = extension_manager.clone();
                    std::thread::spawn(move || {
                        let _ = handle;
                        let tokio_runtime = tokio::runtime::Runtime::new().map_err(|err| {
                            RuntimeError::Builder(format!(
                                "failed to create tokio runtime for extension activation: {err}"
                            ))
                        })?;
                        tokio_runtime.block_on(pipeline.run(request, &manager))
                    })
                    .join()
                    .map_err(|_| {
                        RuntimeError::Builder("extension activation thread panicked".to_string())
                    })?
                } else {
                    let tokio_runtime = tokio::runtime::Runtime::new().map_err(|err| {
                        RuntimeError::Builder(format!(
                            "failed to create tokio runtime for extension activation: {err}"
                        ))
                    })?;
                    tokio_runtime.block_on(pipeline.run(request, &extension_manager))
                }?;
                extension_manager.publish_snapshot(candidate.snapshot.clone())?;
                runtime.extension_snapshot = candidate.snapshot.clone();
                runtime.tools = candidate.snapshot.tool_catalog();
                runtime.registry_snapshot = candidate.snapshot.registry_snapshot.clone();
                candidate.commit();
            }
            runtime.extension_manager = extension_manager;
        }
        runtime.workspace_context_snapshot = self.workspace_context_snapshot;
        #[cfg(feature = "skills")]
        let runtime = match skill_state_handle {
            Some(state) => runtime.with_skill_state(state),
            None => runtime,
        };
        Ok(runtime)
    }
}
