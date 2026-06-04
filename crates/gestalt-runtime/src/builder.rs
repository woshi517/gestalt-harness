use std::sync::Arc;
use gestalt_core::{
    approval::ApprovalProvider,
    context::ContextPipeline,
    policy::PolicyEngine,
    provider::Provider,
    tool::ToolCatalog,
    trace::TraceSink,
    HookRegistry,
};

use crate::config::RuntimeConfig;
use crate::error::{RuntimeError, Result};
use crate::runtime::AgentRuntime;
use crate::registry::RuntimeRegistry;

use crate::extension::GestaltExtension;
use crate::composition_hooks::CompositionHooks;

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
            return Err(RuntimeError::Builder("max_turns must be positive".to_string()));
        }

        // Apply extensions before constructing AgentRuntime
        for ext in &self.extensions {
            let name = ext.name().to_string();
            if self.registry.extensions.contains(&name) {
                return Err(RuntimeError::Registry(format!("Duplicate extension name: {}", name)));
            }
            ext.register(&mut self.registry).map_err(|e| {
                RuntimeError::Extension(format!("Extension '{}' failed to register: {}", name, e))
            })?;
            self.registry.register_extension(name)?;
        }

        let provider = self.provider.ok_or_else(|| RuntimeError::Builder("Missing provider".to_string()))?;
        let tools = self.tools.ok_or_else(|| RuntimeError::Builder("Missing tools".to_string()))?;
        let middleware = self.middleware.ok_or_else(|| RuntimeError::Builder("Missing middleware/context pipeline".to_string()))?;
        let policy = self.policy.ok_or_else(|| RuntimeError::Builder("Missing policy engine".to_string()))?;
        let approval = self.approval.ok_or_else(|| RuntimeError::Builder("Missing approval provider".to_string()))?;

        Ok(AgentRuntime::new(
            provider,
            tools,
            middleware,
            policy,
            approval,
            self.trace_sink,
            self.config,
            self.hooks,
            self.registry,
            self.composition_hooks,
        ))
    }
}
