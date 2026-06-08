use gestalt_core::{
    approval::ApprovalProvider, context::ContextPipeline, policy::PolicyEngine, provider::Provider,
    tool::ToolCatalog, trace::TraceSink, HookRegistry,
};
use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::error::{Result, RuntimeError};
use crate::registry::RuntimeRegistry;
use crate::runtime::AgentRuntime;

use crate::composition_hooks::CompositionHooks;
use crate::event_bus::RuntimeEventBus;
use crate::extension::GestaltExtension;

#[derive(Clone)]
pub struct AgentRuntimeBuilder {
    pub provider: Option<Arc<dyn Provider>>,
    pub tools: Option<Arc<dyn ToolCatalog>>,
    pub middleware: Option<Arc<dyn ContextPipeline>>,
    pub policy: Option<Arc<dyn PolicyEngine>>,
    pub approval: Option<Arc<dyn ApprovalProvider>>,
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    pub config: RuntimeConfig,
    pub hooks: HookRegistry,
    pub registry: RuntimeRegistry,
    pub composition_hooks: Option<Arc<dyn CompositionHooks>>,
    pub extensions: Vec<Arc<dyn GestaltExtension>>,
    pub event_bus: RuntimeEventBus,
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
            policy: None,
            approval: None,
            trace_sink: None,
            config: RuntimeConfig::default(),
            hooks: HookRegistry::default(),
            registry: RuntimeRegistry::new(),
            composition_hooks: None,
            extensions: Vec::new(),
            event_bus: RuntimeEventBus::new(),
        }
    }

    pub fn composition_hooks(mut self, hooks: Arc<dyn CompositionHooks>) -> Self {
        self.composition_hooks = Some(hooks);
        self
    }

    pub fn extension(mut self, extension: Arc<dyn GestaltExtension>) -> Self {
        self.extensions.push(extension);
        self
    }

    pub fn extensions(mut self, extensions: Vec<Arc<dyn GestaltExtension>>) -> Self {
        self.extensions.extend(extensions);
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

    pub fn middleware(mut self, middleware: Arc<dyn ContextPipeline>) -> Self {
        self.middleware = Some(middleware);
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

    pub fn hooks(mut self, hooks: HookRegistry) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn build(mut self) -> Result<AgentRuntime> {
        if self.config.max_turns == 0 {
            return Err(RuntimeError::Builder(
                "max_turns must be positive".to_string(),
            ));
        }

        // Wire the trusted-extension allow-list before any extension
        // descriptor is built, so `build_extension_tool_descriptor`
        // can promote annotations to `BuiltInTrusted`.
        crate::extension_trust::set_trusted_extension_ids(
            self.config.trusted_extension_ids.iter().cloned(),
        );

        // Apply extensions before constructing AgentRuntime
        for ext in &self.extensions {
            let name = ext.name().to_string();
            if self.registry.extensions.contains(&name) {
                return Err(RuntimeError::Registry(format!(
                    "Duplicate extension name: {}",
                    name
                )));
            }
            ext.register(&mut self.registry).map_err(|e| {
                RuntimeError::Extension(format!("Extension '{}' failed to register: {}", name, e))
            })?;
            self.registry.register_extension(name)?;
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

        let mut composed_tools =
            crate::tool_catalog::ComposedToolCatalog::new(base_tools, extension_tools)
                .map_err(RuntimeError::Registry)?;
        if let Some(ref profile) = self.config.tool_profile {
            composed_tools = composed_tools.with_planner(
                crate::tool_catalog_planner::ToolCatalogPlanner::new(profile.clone()),
            );
        }
        let composed_tools = Arc::new(composed_tools);

        let middleware = self.middleware.ok_or_else(|| {
            RuntimeError::Builder("Missing middleware/context pipeline".to_string())
        })?;
        let policy = self
            .policy
            .ok_or_else(|| RuntimeError::Builder("Missing policy engine".to_string()))?;
        let approval = self
            .approval
            .ok_or_else(|| RuntimeError::Builder("Missing approval provider".to_string()))?;

        let user_hooks = self.composition_hooks.take();
        let composed_hooks: Arc<dyn crate::composition_hooks::CompositionHooks> =
            Arc::new(crate::composition_hooks::ComposedCompositionHooks {
                user_hooks,
                extensions: self.extensions.clone(),
            });

        Ok(AgentRuntime::new(
            provider,
            composed_tools,
            middleware,
            policy,
            approval,
            self.trace_sink,
            self.config,
            self.hooks,
            self.registry,
            Some(composed_hooks),
            self.event_bus,
        ))
    }
}
