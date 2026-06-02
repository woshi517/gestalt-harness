use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::{
    context::ContextPacket,
    error::{Result, TraceError},
    event::AgentEvent,
    provider::ProviderRequest,
    session::Session,
    tool::ToolExecutionResult,
};

#[async_trait]
pub trait SessionHook: Send + Sync {
    async fn on_session_start(&self, session: &Session) -> Result<Vec<AgentEvent>>;
    async fn on_session_end(&self, session: &Session) -> Result<Vec<AgentEvent>>;
}

#[async_trait]
pub trait ContextHook: Send + Sync {
    async fn before_context_build(&self, session: &Session) -> Result<Vec<AgentEvent>>;
    async fn after_context_build(
        &self,
        session: &Session,
        packet: &ContextPacket,
    ) -> Result<Vec<AgentEvent>>;
}

#[async_trait]
pub trait ModelHook: Send + Sync {
    async fn before_model_request(
        &self,
        session: &Session,
        request: &ProviderRequest,
    ) -> Result<Vec<AgentEvent>>;
    async fn after_model_response(
        &self,
        session: &Session,
        event: &AgentEvent,
    ) -> Result<Vec<AgentEvent>>;
}

#[async_trait]
pub trait ToolHook: Send + Sync {
    async fn before_tool_execution(
        &self,
        session: &Session,
        tool_name: &str,
        input: &Value,
    ) -> Result<Vec<AgentEvent>>;
    async fn after_tool_execution(
        &self,
        session: &Session,
        tool_name: &str,
        result: &ToolExecutionResult,
    ) -> Result<Vec<AgentEvent>>;
}

#[async_trait]
pub trait VerificationHook: Send + Sync {
    async fn after_verification(
        &self,
        session: &Session,
        event: &AgentEvent,
    ) -> Result<Vec<AgentEvent>>;
}

pub trait TraceHook: Send + Sync {
    fn on_trace_write(&self, event: &AgentEvent) -> std::result::Result<(), TraceError>;
}

#[derive(Default, Clone)]
pub struct HookRegistry {
    pub session_hooks: Vec<Arc<dyn SessionHook>>,
    pub context_hooks: Vec<Arc<dyn ContextHook>>,
    pub model_hooks: Vec<Arc<dyn ModelHook>>,
    pub tool_hooks: Vec<Arc<dyn ToolHook>>,
    pub verification_hooks: Vec<Arc<dyn VerificationHook>>,
    pub trace_hooks: Vec<Arc<dyn TraceHook>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_session_hook(&mut self, hook: Arc<dyn SessionHook>) {
        self.session_hooks.push(hook);
    }

    pub fn register_context_hook(&mut self, hook: Arc<dyn ContextHook>) {
        self.context_hooks.push(hook);
    }

    pub fn register_model_hook(&mut self, hook: Arc<dyn ModelHook>) {
        self.model_hooks.push(hook);
    }

    pub fn register_tool_hook(&mut self, hook: Arc<dyn ToolHook>) {
        self.tool_hooks.push(hook);
    }

    pub fn register_verification_hook(&mut self, hook: Arc<dyn VerificationHook>) {
        self.verification_hooks.push(hook);
    }

    pub fn register_trace_hook(&mut self, hook: Arc<dyn TraceHook>) {
        self.trace_hooks.push(hook);
    }
}
