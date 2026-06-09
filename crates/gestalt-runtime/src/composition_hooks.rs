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
    SwitchModel {
        model: String,
        provider: Option<String>,
    },
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

pub struct PrepareNextTurnCtx {
    pub session_id: String,
    pub history: Vec<Message>,
    pub turn_index: usize,
    pub current_model: String,
    pub current_provider: String,
}

#[async_trait]
pub trait CompositionHooks: Send + Sync {
    async fn before_context_build(&self, context: &BeforeContextBuildCtx) -> Result<HookOutcome>;
    async fn after_context_build(&self, context: &AfterContextBuildCtx) -> Result<HookOutcome>;
    async fn before_tool_policy(&self, context: &BeforeToolPolicyCtx) -> Result<HookOutcome>;
    async fn after_tool_result(&self, context: &AfterToolResultCtx) -> Result<HookOutcome>;
    async fn prepare_next_turn(&self, context: &PrepareNextTurnCtx) -> Result<HookOutcome>;
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
    pub skill_state: Option<Arc<Mutex<crate::skill_contributor::SkillContributorState>>>,
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

        // Resolve dynamic skill activation for this turn. Trigger matching uses
        // the most recent user message as the task hint. The resulting diff is
        // published on the event bus so consumers (inspect, traces, debug)
        // can see what changed.
        if let Some(state) = &self.skill_state {
            let task_hint = last_user_text(&session.history);
            let mut guard = state.lock().unwrap();
            let (_resolved, diff) = guard.resolve_active(task_hint.as_deref());
            guard.publish_diff(&diff);
        }

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

pub struct RuntimeModelHookAdapter {
    pub hooks: Arc<dyn CompositionHooks>,
    pub event_bus: RuntimeEventBus,
}

pub struct RuntimeNextTurnHookAdapter {
    pub hooks: Arc<dyn CompositionHooks>,
    pub event_bus: RuntimeEventBus,
}

#[async_trait]
impl gestalt_core::hook::NextTurnHook for RuntimeNextTurnHookAdapter {
    async fn prepare_next_turn(
        &self,
        session: &Session,
        current_turn: usize,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        self.event_bus.publish(RuntimeEvent::HookStarted {
            hook_name: "prepare_next_turn".to_string(),
            lifecycle_point: "prepare_next_turn".to_string(),
        });

        let effective_model = session
            .next_turn_override
            .as_ref()
            .map(|o| o.model.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| session.config.model.clone());
        let effective_provider = session
            .next_turn_override
            .as_ref()
            .and_then(|o| o.provider.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| session.config.provider.clone());

        let ctx = PrepareNextTurnCtx {
            session_id: session.id.clone(),
            history: session.history.clone(),
            turn_index: current_turn,
            current_model: effective_model,
            current_provider: effective_provider,
        };

        match self.hooks.prepare_next_turn(&ctx).await {
            Ok(outcome) => {
                self.event_bus.publish(RuntimeEvent::HookCompleted {
                    hook_name: "prepare_next_turn".to_string(),
                    lifecycle_point: "prepare_next_turn".to_string(),
                    outcome: format!("{:?}", outcome),
                });

                match outcome {
                    HookOutcome::SwitchModel { model, provider } => {
                        let override_model = if model.is_empty() {
                            session.config.model.clone()
                        } else {
                            model
                        };
                        let pending = gestalt_core::session::NextTurnOverride {
                            model: override_model,
                            provider,
                        };
                        Ok(vec![AgentEvent::NextTurnOverrideRequested {
                            model: pending.model.clone(),
                            provider: pending.provider.clone(),
                        }])
                    }
                    HookOutcome::Block { reason } => Ok(vec![AgentEvent::NextTurnBlocked {
                        reason,
                    }]),
                    _ => Ok(Vec::new()),
                }
            }
            Err(err) => {
                self.event_bus.publish(RuntimeEvent::HookFailed {
                    hook_name: "prepare_next_turn".to_string(),
                    lifecycle_point: "prepare_next_turn".to_string(),
                    error: err.to_string(),
                });
                Ok(Vec::new())
            }
        }
    }
}

#[async_trait]
impl gestalt_core::hook::ModelHook for RuntimeModelHookAdapter {
    async fn before_model_request(
        &self,
        _session: &Session,
        _request: &gestalt_core::provider::ProviderRequest,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        Ok(Vec::new())
    }

    async fn after_model_response(
        &self,
        _session: &Session,
        _event: &AgentEvent,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        Ok(Vec::new())
    }
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
                "switch_model" => {
                    let model = obj
                        .get("model")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let provider = obj
                        .get("provider")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    return HookOutcome::SwitchModel { model, provider };
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

    async fn prepare_next_turn(&self, context: &PrepareNextTurnCtx) -> Result<HookOutcome> {
        if let Some(ref user) = self.user_hooks {
            let res = user.prepare_next_turn(context).await?;
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
                    .find(|h| h.lifecycle_point == "prepare_next_turn")
                {
                    let params = serde_json::json!({
                        "name": hook_decl.name.clone(),
                        "lifecycle_point": "prepare_next_turn",
                        "context": {
                            "session_id": context.session_id.clone(),
                            "history": context.history.clone(),
                            "turn_index": context.turn_index,
                            "current_model": context.current_model.clone(),
                            "current_provider": context.current_provider.clone(),
                        }
                    });
                    if let Ok(res_val) = pe.broker.call("hooks/call", Some(params)).await {
                        let outcome = parse_hook_outcome(res_val);
                        match outcome {
                            HookOutcome::Block { .. } => return Ok(outcome),
                            HookOutcome::SwitchModel { .. } | HookOutcome::Annotate { .. } => {
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

/// Extract the most recent user-supplied text from session history. Used as
/// the task hint passed to the deterministic skill activation engine. Returns
/// `None` if no user text can be found.
fn last_user_text(history: &[gestalt_core::message::Message]) -> Option<String> {
    for msg in history.iter().rev() {
        if let gestalt_core::message::Message::User { content } = msg {
            let mut combined = String::new();
            for block in content {
                if let gestalt_core::message::ContentBlock::Text { text } = block {
                    if !combined.is_empty() {
                        combined.push('\n');
                    }
                    combined.push_str(text);
                }
            }
            if !combined.is_empty() {
                return Some(combined);
            }
        }
    }
    None
}
