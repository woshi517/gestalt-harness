use std::sync::{Arc, Mutex};
use async_trait::async_trait;
use gestalt_core::{
    context::ContextPacket,
    event::AgentEvent,
    message::Message,
    session::Session,
    tool::ToolExecutionResult,
};
use crate::error::Result;
use crate::context::ContextContributor;

#[derive(Debug, Clone, PartialEq)]
pub enum HookOutcome {
    Continue,
    Block { reason: String },
    AddContext { message: Message },
    Annotate { metadata: serde_json::Value },
}

pub struct BeforeContextBuildCtx {
    pub session_id: String,
    pub history: Vec<Message>,
}

pub struct AfterContextBuildCtx {
    pub session_id: String,
    pub history: Vec<Message>,
    pub packet: ContextPacket,
}

pub struct BeforeToolPolicyCtx {
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
}

pub struct AfterToolResultCtx {
    pub session_id: String,
    pub tool_name: String,
    pub result: ToolExecutionResult,
}

pub struct OnEventCtx {
    pub session_id: String,
    pub event: AgentEvent,
}

#[async_trait]
pub trait CompositionHooks: Send + Sync {
    async fn before_context_build(&self, context: &BeforeContextBuildCtx) -> Result<HookOutcome>;
    async fn after_context_build(&self, context: &AfterContextBuildCtx) -> Result<HookOutcome>;
    async fn before_tool_policy(&self, context: &BeforeToolPolicyCtx) -> Result<HookOutcome>;
    async fn after_tool_result(&self, context: &AfterToolResultCtx) -> Result<HookOutcome>;
    async fn on_event(&self, context: &OnEventCtx) -> Result<()>;
}

pub struct RuntimeContextHookAdapter {
    pub hooks: Arc<dyn CompositionHooks>,
    pub patch_store: Arc<Mutex<Vec<Message>>>,
    pub contributors: Vec<Arc<dyn ContextContributor>>,
    pub workspace_root: std::path::PathBuf,
    pub block_reason: Option<Arc<Mutex<Option<String>>>>,
}

#[async_trait]
impl gestalt_core::hook::ContextHook for RuntimeContextHookAdapter {
    async fn before_context_build(&self, session: &Session) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        let mut events = Vec::new();

        // 2. Run composition hook before_context_build
        let ctx = BeforeContextBuildCtx {
            session_id: session.id.clone(),
            history: session.history.clone(),
        };
        match self.hooks.before_context_build(&ctx).await {
            Ok(HookOutcome::AddContext { message }) => {
                let mut store = self.patch_store.lock().unwrap();
                store.push(message);
            }
            Ok(HookOutcome::Block { reason }) => {
                if let Some(ref br) = self.block_reason {
                    let mut lock = br.lock().unwrap();
                    *lock = Some(reason.clone());
                }
                events.push(AgentEvent::Error {
                    message: format!("before_context_build blocked: {}", reason),
                    recoverable: true,
                });
            }
            _ => {}
        }

        // 3. Run context contributors
        for contributor in &self.contributors {
            match contributor.contribute(&self.workspace_root).await {
                Ok(msg) => {
                    let mut store = self.patch_store.lock().unwrap();
                    store.push(msg);
                }
                Err(err) => {
                    events.push(AgentEvent::Error {
                        message: format!("ContextContributor '{}' failed: {}", contributor.name(), err),
                        recoverable: true,
                    });
                }
            }
        }

        Ok(events)
    }

    async fn after_context_build(
        &self,
        session: &Session,
        packet: &ContextPacket,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        // Clear patch store first so we clear previous context additions and contributors' messages.
        {
            let mut store = self.patch_store.lock().unwrap();
            store.clear();
        }
        let ctx = AfterContextBuildCtx {
            session_id: session.id.clone(),
            history: session.history.clone(),
            packet: packet.clone(),
        };
        let mut events = Vec::new();
        match self.hooks.after_context_build(&ctx).await {
            Ok(HookOutcome::AddContext { message }) => {
                let mut store = self.patch_store.lock().unwrap();
                store.push(message);
            }
            Ok(HookOutcome::Block { reason }) => {
                if let Some(ref br) = self.block_reason {
                    let mut lock = br.lock().unwrap();
                    *lock = Some(reason.clone());
                }
                events.push(AgentEvent::Error {
                    message: format!("after_context_build blocked: {}", reason),
                    recoverable: true,
                });
            }
            _ => {}
        }
        Ok(events)
    }
}

pub struct RuntimeToolHookAdapter {
    pub hooks: Arc<dyn CompositionHooks>,
}

#[async_trait]
impl gestalt_core::hook::ToolHook for RuntimeToolHookAdapter {
    async fn before_tool_execution(
        &self,
        _session: &Session,
        _tool_name: &str,
        _input: &serde_json::Value,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        Ok(Vec::new())
    }

    async fn after_tool_execution(
        &self,
        session: &Session,
        tool_name: &str,
        result: &ToolExecutionResult,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        let ctx = AfterToolResultCtx {
            session_id: session.id.clone(),
            tool_name: tool_name.to_string(),
            result: result.clone(),
        };
        let mut events = Vec::new();
        match self.hooks.after_tool_result(&ctx).await {
            Ok(HookOutcome::Annotate { metadata }) => {
                // Since AgentEvent doesn't have an Annotate variant, we emit a recoverable Error event containing the metadata
                events.push(AgentEvent::Error {
                    message: format!("Metadata annotation for {}: {}", tool_name, metadata),
                    recoverable: true,
                });
            }
            Ok(HookOutcome::Block { reason }) => {
                events.push(AgentEvent::Error {
                    message: format!("after_tool_result blocked: {}", reason),
                    recoverable: true,
                });
            }
            _ => {}
        }
        Ok(events)
    }
}

pub struct RuntimeTraceHookAdapter {
    pub tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
}

impl gestalt_core::hook::TraceHook for RuntimeTraceHookAdapter {
    fn on_trace_write(&self, event: &AgentEvent) -> std::result::Result<(), gestalt_core::error::TraceError> {
        let _ = self.tx.send(event.clone());
        Ok(())
    }
}
