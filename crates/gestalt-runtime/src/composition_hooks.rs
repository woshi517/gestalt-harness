use crate::context::ContextContributor;
use crate::error::Result;
use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use async_trait::async_trait;
use gestalt_core::{
    context::{ContextPacket, PromptSnapshot},
    event::AgentEvent,
    message::Message,
    session::Session,
    tool::ToolExecutionResult,
    ContextStability,
};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;

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
    pub artifact_dir: Option<PathBuf>,
}

pub struct AfterContextBuildCtx {
    pub session_id: String,
    pub history: Vec<Message>,
    pub packet: ContextPacket,
    pub artifact_dir: Option<PathBuf>,
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
    pub patch_store: Arc<Mutex<Vec<crate::context::ContextPatch>>>,
    pub contributors: Vec<Arc<dyn ContextContributor>>,
    pub workspace_root: std::path::PathBuf,
    pub block_reason: Option<Arc<Mutex<Option<String>>>>,
    pub event_bus: RuntimeEventBus,
    pub prompt_snapshot_state: Arc<Mutex<Option<String>>>,
}

#[async_trait]
impl gestalt_core::hook::ContextHook for RuntimeContextHookAdapter {
    async fn before_context_build(
        &self,
        session: &Session,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        let mut events = Vec::new();

        self.event_bus.publish(RuntimeEvent::HookStarted {
            hook_name: "before_context_build".to_string(),
            lifecycle_point: "before_context_build".to_string(),
        });

        // Run composition hook before_context_build
        let ctx = BeforeContextBuildCtx {
            session_id: session.id.clone(),
            history: session.history.clone(),
            artifact_dir: session.tool_ctx.artifact_dir.clone(),
        };
        match self.hooks.before_context_build(&ctx).await {
            Ok(outcome) => {
                self.event_bus.publish(RuntimeEvent::HookCompleted {
                    hook_name: "before_context_build".to_string(),
                    lifecycle_point: "before_context_build".to_string(),
                    outcome: format!("{:?}", outcome),
                });
                match outcome {
                    HookOutcome::AddContext { message } => {
                        let mut store = self.patch_store.lock().unwrap();
                        store.push(crate::context::ContextPatch::new(
                            message,
                            ContextStability::TurnDynamic,
                        ));
                    }
                    HookOutcome::Block { reason } => {
                        if let Some(ref br) = self.block_reason {
                            let mut lock = br.lock().unwrap();
                            *lock = Some(reason.clone());
                        }
                    }
                    _ => {}
                }
            }
            Err(err) => {
                self.event_bus.publish(RuntimeEvent::HookFailed {
                    hook_name: "before_context_build".to_string(),
                    lifecycle_point: "before_context_build".to_string(),
                    error: err.to_string(),
                });
            }
        }

        // Run context contributors
        for contributor in &self.contributors {
            match contributor.contribute(&self.workspace_root).await {
                Ok(msg) => {
                    let mut store = self.patch_store.lock().unwrap();
                    store.push(crate::context::ContextPatch::new(msg, contributor.stability()));
                }
                Err(err) => {
                    events.push(AgentEvent::Error {
                        message: format!(
                            "ContextContributor '{}' failed: {}",
                            contributor.name(),
                            err
                        ),
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
        self.event_bus.publish(RuntimeEvent::HookStarted {
            hook_name: "after_context_build".to_string(),
            lifecycle_point: "after_context_build".to_string(),
        });
        let ctx = AfterContextBuildCtx {
            session_id: session.id.clone(),
            history: session.history.clone(),
            packet: packet.clone(),
            artifact_dir: session.tool_ctx.artifact_dir.clone(),
        };
        let events = Vec::new();
        match self.hooks.after_context_build(&ctx).await {
            Ok(outcome) => {
                self.event_bus.publish(RuntimeEvent::HookCompleted {
                    hook_name: "after_context_build".to_string(),
                    lifecycle_point: "after_context_build".to_string(),
                    outcome: format!("{:?}", outcome),
                });
                match outcome {
                    HookOutcome::AddContext { message } => {
                        let mut store = self.patch_store.lock().unwrap();
                        store.push(crate::context::ContextPatch::new(
                            message,
                            ContextStability::TurnDynamic,
                        ));
                    }
                    HookOutcome::Block { reason } => {
                        if let Some(ref br) = self.block_reason {
                            let mut lock = br.lock().unwrap();
                            *lock = Some(reason.clone());
                        }
                    }
                    _ => {}
                }
            }
            Err(err) => {
                self.event_bus.publish(RuntimeEvent::HookFailed {
                    hook_name: "after_context_build".to_string(),
                    lifecycle_point: "after_context_build".to_string(),
                    error: err.to_string(),
                });
            }
        }

        if let Some(cache_plan) = packet.cache_plan.as_ref() {
            let snapshot_path = ctx
                .artifact_dir
                .as_ref()
                .map(|dir| dir.join("prompt-snapshot.json"));
            let snapshot_messages = packet
                .messages
                .iter()
                .take(cache_plan.prefix_message_count)
                .cloned()
                .collect::<Vec<_>>();
            let snapshot = PromptSnapshot::new(snapshot_messages, 0);

            if let Some(path) = snapshot_path.as_ref() {
                let _ = gestalt_trace::write_prompt_snapshot(path, &snapshot);
            }

            let mut state = self.prompt_snapshot_state.lock().unwrap();
            let event = if state.as_ref() == Some(&snapshot.snapshot_hash) {
                AgentEvent::PromptSnapshotReused {
                    snapshot_hash: snapshot.snapshot_hash.clone(),
                    prefix_hash: snapshot.prefix_hash.clone(),
                }
            } else {
                *state = Some(snapshot.snapshot_hash.clone());
                AgentEvent::PromptSnapshotCreated {
                    snapshot_hash: snapshot.snapshot_hash.clone(),
                    prefix_hash: snapshot.prefix_hash.clone(),
                    created_turn: session.history.len(),
                }
            };

            let cache_event = AgentEvent::PromptCachePlanGenerated {
                snapshot_hash: snapshot.snapshot_hash,
                prefix_hash: snapshot.prefix_hash,
                prefix_message_count: cache_plan.prefix_message_count,
            };

            return Ok(vec![event, cache_event]);
        }
        Ok(events)
    }
}

pub struct RuntimeToolHookAdapter {
    pub hooks: Arc<dyn CompositionHooks>,
    pub event_bus: RuntimeEventBus,
}

#[async_trait]
impl gestalt_core::hook::ToolHook for RuntimeToolHookAdapter {
    async fn before_tool_execution(
        &self,
        session: &Session,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        self.event_bus.publish(RuntimeEvent::HookStarted {
            hook_name: "before_tool_policy".to_string(),
            lifecycle_point: "before_tool_policy".to_string(),
        });
        let ctx = BeforeToolPolicyCtx {
            session_id: session.id.clone(),
            tool_name: tool_name.to_string(),
            tool_input: input.clone(),
        };
        let mut events = Vec::new();
        match self.hooks.before_tool_policy(&ctx).await {
            Ok(outcome) => {
                self.event_bus.publish(RuntimeEvent::HookCompleted {
                    hook_name: "before_tool_policy".to_string(),
                    lifecycle_point: "before_tool_policy".to_string(),
                    outcome: format!("{:?}", outcome),
                });
                if let HookOutcome::Block { reason } = outcome {
                    events.push(AgentEvent::Error {
                        message: format!("before_tool_policy blocked: {}", reason),
                        recoverable: true,
                    });
                }
            }
            Err(err) => {
                self.event_bus.publish(RuntimeEvent::HookFailed {
                    hook_name: "before_tool_policy".to_string(),
                    lifecycle_point: "before_tool_policy".to_string(),
                    error: err.to_string(),
                });
            }
        }
        Ok(events)
    }

    async fn after_tool_execution(
        &self,
        session: &Session,
        tool_name: &str,
        result: &ToolExecutionResult,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        self.event_bus.publish(RuntimeEvent::HookStarted {
            hook_name: "after_tool_result".to_string(),
            lifecycle_point: "after_tool_result".to_string(),
        });
        let ctx = AfterToolResultCtx {
            session_id: session.id.clone(),
            tool_name: tool_name.to_string(),
            result: result.clone(),
        };
        let mut events = Vec::new();
        match self.hooks.after_tool_result(&ctx).await {
            Ok(outcome) => {
                self.event_bus.publish(RuntimeEvent::HookCompleted {
                    hook_name: "after_tool_result".to_string(),
                    lifecycle_point: "after_tool_result".to_string(),
                    outcome: format!("{:?}", outcome),
                });
                if let HookOutcome::Block { reason } = outcome {
                    events.push(AgentEvent::Error {
                        message: format!("after_tool_result blocked: {}", reason),
                        recoverable: true,
                    });
                }
            }
            Err(err) => {
                self.event_bus.publish(RuntimeEvent::HookFailed {
                    hook_name: "after_tool_result".to_string(),
                    lifecycle_point: "after_tool_result".to_string(),
                    error: err.to_string(),
                });
            }
        }
        Ok(events)
    }
}

pub struct RuntimeTraceHookAdapter {
    pub tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>,
}

impl gestalt_core::hook::TraceHook for RuntimeTraceHookAdapter {
    fn on_trace_write(
        &self,
        event: &AgentEvent,
    ) -> std::result::Result<(), gestalt_core::error::TraceError> {
        let _ = self.tx.send(event.clone());
        Ok(())
    }
}

fn parse_hook_outcome(val: serde_json::Value) -> HookOutcome {
    if let Some(s) = val.as_str() {
        if s == "continue" {
            return HookOutcome::Continue;
        }
    }
    if let Some(obj) = val.as_object() {
        if let Some(t) = obj.get("type").and_then(|v| v.as_str()) {
            match t {
                "block" => {
                    let reason = obj
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Blocked by hook")
                        .to_string();
                    return HookOutcome::Block { reason };
                }
                "add_context" => {
                    if let Some(msg_val) = obj.get("message") {
                        if let Ok(message) = serde_json::from_value(msg_val.clone()) {
                            return HookOutcome::AddContext { message };
                        }
                    }
                }
                "annotate" => {
                    let metadata = obj
                        .get("metadata")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    return HookOutcome::Annotate { metadata };
                }
                _ => {}
            }
        }
    }
    HookOutcome::Continue
}

pub struct ComposedCompositionHooks {
    pub user_hooks: Option<Arc<dyn CompositionHooks>>,
    pub extensions: Vec<Arc<dyn crate::extension::GestaltExtension>>,
}

#[async_trait]
impl CompositionHooks for ComposedCompositionHooks {
    async fn before_context_build(&self, context: &BeforeContextBuildCtx) -> Result<HookOutcome> {
        if let Some(ref user) = self.user_hooks {
            let res = user.before_context_build(context).await?;
            if !matches!(res, HookOutcome::Continue) {
                return Ok(res);
            }
        }

        let mut final_outcome = HookOutcome::Continue;
        for ext in &self.extensions {
            if let Some(pe) = ext.as_process_extension() {
                if let Some(hook_decl) = pe
                    .manifest
                    .hooks
                    .iter()
                    .find(|h| h.lifecycle_point == "before_context_build")
                {
                    let params = serde_json::json!({
                        "name": hook_decl.name.clone(),
                        "lifecycle_point": "before_context_build",
                        "context": {
                            "session_id": context.session_id.clone(),
                            "history": context.history.clone(),
                        }
                    });
                    if let Ok(res_val) = pe.broker.call("hooks/call", Some(params)).await {
                        let outcome = parse_hook_outcome(res_val);
                        match outcome {
                            HookOutcome::Block { .. } => return Ok(outcome),
                            HookOutcome::AddContext { .. } | HookOutcome::Annotate { .. } => {
                                final_outcome = outcome;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(final_outcome)
    }

    async fn after_context_build(&self, context: &AfterContextBuildCtx) -> Result<HookOutcome> {
        if let Some(ref user) = self.user_hooks {
            let res = user.after_context_build(context).await?;
            if !matches!(res, HookOutcome::Continue) {
                return Ok(res);
            }
        }

        let mut final_outcome = HookOutcome::Continue;
        for ext in &self.extensions {
            if let Some(pe) = ext.as_process_extension() {
                if let Some(hook_decl) = pe
                    .manifest
                    .hooks
                    .iter()
                    .find(|h| h.lifecycle_point == "after_context_build")
                {
                    let params = serde_json::json!({
                        "name": hook_decl.name.clone(),
                        "lifecycle_point": "after_context_build",
                        "context": {
                            "session_id": context.session_id.clone(),
                            "history": context.history.clone(),
                            "packet": context.packet.clone(),
                        }
                    });
                    if let Ok(res_val) = pe.broker.call("hooks/call", Some(params)).await {
                        let outcome = parse_hook_outcome(res_val);
                        match outcome {
                            HookOutcome::Block { .. } => return Ok(outcome),
                            HookOutcome::AddContext { .. } | HookOutcome::Annotate { .. } => {
                                final_outcome = outcome;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(final_outcome)
    }

    async fn before_tool_policy(&self, context: &BeforeToolPolicyCtx) -> Result<HookOutcome> {
        if let Some(ref user) = self.user_hooks {
            let res = user.before_tool_policy(context).await?;
            if !matches!(res, HookOutcome::Continue) {
                return Ok(res);
            }
        }

        let mut final_outcome = HookOutcome::Continue;
        for ext in &self.extensions {
            if let Some(pe) = ext.as_process_extension() {
                if let Some(hook_decl) = pe
                    .manifest
                    .hooks
                    .iter()
                    .find(|h| h.lifecycle_point == "before_tool_policy")
                {
                    let params = serde_json::json!({
                        "name": hook_decl.name.clone(),
                        "lifecycle_point": "before_tool_policy",
                        "context": {
                            "session_id": context.session_id.clone(),
                            "tool_name": context.tool_name.clone(),
                            "tool_input": context.tool_input.clone(),
                        }
                    });
                    if let Ok(res_val) = pe.broker.call("hooks/call", Some(params)).await {
                        let outcome = parse_hook_outcome(res_val);
                        match outcome {
                            HookOutcome::Block { .. } => return Ok(outcome),
                            HookOutcome::AddContext { .. } | HookOutcome::Annotate { .. } => {
                                final_outcome = outcome;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(final_outcome)
    }

    async fn after_tool_result(&self, context: &AfterToolResultCtx) -> Result<HookOutcome> {
        if let Some(ref user) = self.user_hooks {
            let res = user.after_tool_result(context).await?;
            if !matches!(res, HookOutcome::Continue) {
                return Ok(res);
            }
        }

        let mut final_outcome = HookOutcome::Continue;
        for ext in &self.extensions {
            if let Some(pe) = ext.as_process_extension() {
                if let Some(hook_decl) = pe
                    .manifest
                    .hooks
                    .iter()
                    .find(|h| h.lifecycle_point == "after_tool_result")
                {
                    let params = serde_json::json!({
                        "name": hook_decl.name.clone(),
                        "lifecycle_point": "after_tool_result",
                        "context": {
                            "session_id": context.session_id.clone(),
                            "tool_name": context.tool_name.clone(),
                            "result": context.result.clone(),
                        }
                    });
                    if let Ok(res_val) = pe.broker.call("hooks/call", Some(params)).await {
                        let outcome = parse_hook_outcome(res_val);
                        match outcome {
                            HookOutcome::Block { .. } => return Ok(outcome),
                            HookOutcome::AddContext { .. } | HookOutcome::Annotate { .. } => {
                                final_outcome = outcome;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(final_outcome)
    }

    async fn on_event(&self, context: &OnEventCtx) -> Result<()> {
        if let Some(ref user) = self.user_hooks {
            user.on_event(context).await?;
        }

        for ext in &self.extensions {
            if let Some(pe) = ext.as_process_extension() {
                if let Some(hook_decl) = pe
                    .manifest
                    .hooks
                    .iter()
                    .find(|h| h.lifecycle_point == "on_event")
                {
                    let params = serde_json::json!({
                        "name": hook_decl.name.clone(),
                        "lifecycle_point": "on_event",
                        "context": {
                            "session_id": context.session_id.clone(),
                            "event": context.event.clone(),
                        }
                    });
                    let _ = pe.broker.call("hooks/call", Some(params)).await;
                }
            }
        }
        Ok(())
    }
}
