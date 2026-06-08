use std::{collections::HashSet, sync::Arc};

use futures::StreamExt;
use sha2::{Digest as _, Sha256};

use crate::{
    approval::{ApprovalProvider, SessionGrant},
    context::ContextPipeline,
    error::{HarnessError, ProviderError, Result},
    event::{AgentEvent, StopReason},
    message::Message,
    policy::PolicyEngine,
    provider::{Provider, ProviderRequest},
    session::{RunResult, Session},
    tool::ToolCatalog,
};

pub mod executor;
use executor::ToolExecutor;

pub struct AgentLoop {
    provider: Arc<dyn Provider>,
    middleware: Arc<dyn ContextPipeline>,
    max_turns: usize,
    executor: ToolExecutor,
    pub hooks: crate::hook::HookRegistry,
}

pub trait EmitOutcome {
    fn into_result(self) -> Result<()>;
}

impl EmitOutcome for () {
    fn into_result(self) -> Result<()> {
        Ok(())
    }
}

impl EmitOutcome for Result<()> {
    fn into_result(self) -> Result<()> {
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnOutcome {
    ToolExecuted,
    Stop(StopReason),
}

impl AgentLoop {
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: Arc<dyn ToolCatalog>,
        middleware: Arc<dyn ContextPipeline>,
        policy: Arc<dyn PolicyEngine>,
        approval: Arc<dyn ApprovalProvider>,
        max_turns: usize,
    ) -> Self {
        Self {
            provider,
            middleware,
            max_turns,
            executor: ToolExecutor::new(tools, policy, approval),
            hooks: crate::hook::HookRegistry::default(),
        }
    }

    pub fn with_hooks(mut self, hooks: crate::hook::HookRegistry) -> Self {
        self.hooks = hooks;
        self
    }

    pub async fn run<F, R>(
        &self,
        session: &mut Session,
        cancel_token: &crate::cancel::CancelToken,
        sink: Option<&dyn crate::trace::TraceSink>,
        mut emit_fn: F,
    ) -> Result<RunResult>
    where
        F: FnMut(AgentEvent) -> R + Send,
        R: EmitOutcome,
    {
        let mut emit = |event: AgentEvent| -> Result<()> {
            for hook in &self.hooks.trace_hooks {
                if let Err(err) = hook.on_trace_write(&event) {
                    emit_fn(AgentEvent::Error {
                        message: format!("TraceHook.on_trace_write failed: {err}"),
                        recoverable: true,
                    })
                    .into_result()?;
                }
            }
            emit_fn(event).into_result()
        };

        for hook in &self.hooks.session_hooks {
            emit(AgentEvent::HookStarted {
                hook_type: "session".to_string(),
                name: "on_session_start".to_string(),
            })?;
            let hook_res = tokio::select! {
                res = hook.on_session_start(session) => res,
                _ = cancel_token.cancelled() => return Err(HarnessError::Cancelled),
            };
            match hook_res {
                Ok(events) => {
                    emit(AgentEvent::HookCompleted {
                        hook_type: "session".to_string(),
                        name: "on_session_start".to_string(),
                    })?;
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(err) => {
                    emit(AgentEvent::HookFailed {
                        hook_type: "session".to_string(),
                        name: "on_session_start".to_string(),
                        error: err.to_string(),
                    })?;
                    emit(AgentEvent::Error {
                        message: format!("SessionHook.on_session_start failed: {err}"),
                        recoverable: true,
                    })?;
                }
            }
        }

        let mut last_packet_hash: Option<String> = None;
        let mut last_prompt_source: Option<String> = None;

        let emit_checkpoint = |emit: &mut dyn FnMut(AgentEvent) -> Result<()>,
                               session: &Session,
                               hash: Option<String>,
                               ps: Option<String>|
         -> Result<()> {
            emit(AgentEvent::Checkpoint {
                history: session.history.clone(),
                token_budget: session.token_budget.clone(),
                packet_hash: hash,
                prompt_source: ps,
            })
        };

        emit_checkpoint(
            &mut emit,
            session,
            last_packet_hash.clone(),
            last_prompt_source.clone(),
        )?;

        let mut turns = 0_usize;
        let mut total_input_tokens = 0_usize;
        let mut total_output_tokens = 0_usize;
        let mut artifacts = Vec::new();
        let mut session_grants: Vec<SessionGrant> = Vec::new();
        let final_stop = loop {
            if cancel_token.is_cancelled() {
                return Err(HarnessError::Cancelled);
            }
            if turns >= self.max_turns {
                let reason = StopReason::MaxTurns;
                emit(AgentEvent::Stop { reason })?;
                break reason;
            }
            if session.token_budget.exhausted() {
                let reason = StopReason::BudgetExhausted;
                emit(AgentEvent::Stop { reason })?;
                break reason;
            }

            let request = self
                .build_request(
                    session,
                    &mut emit,
                    cancel_token,
                    &mut last_packet_hash,
                    &mut last_prompt_source,
                )
                .await?;
            let outcome = self
                .run_turn(
                    session,
                    request,
                    &mut emit,
                    &mut total_input_tokens,
                    &mut total_output_tokens,
                    &mut artifacts,
                    &mut session_grants,
                    turns,
                    cancel_token,
                    sink,
                )
                .await?;

            turns = turns.saturating_add(1);

            let stop_opt = self.stop_reason(session, turns, outcome);

            if matches!(outcome, TurnOutcome::ToolExecuted | TurnOutcome::Stop(_)) {
                emit_checkpoint(
                    &mut emit,
                    session,
                    last_packet_hash.clone(),
                    last_prompt_source.clone(),
                )?;
            }

            if let Some(reason) = stop_opt {
                if !matches!(reason, StopReason::EndTurn | StopReason::ToolUse) {
                    emit(AgentEvent::Stop { reason })?;
                }

                break reason;
            }
        };

        for hook in &self.hooks.session_hooks {
            emit(AgentEvent::HookStarted {
                hook_type: "session".to_string(),
                name: "on_session_end".to_string(),
            })?;
            let hook_res = tokio::select! {
                res = hook.on_session_end(session) => res,
                _ = cancel_token.cancelled() => return Err(HarnessError::Cancelled),
            };
            match hook_res {
                Ok(events) => {
                    emit(AgentEvent::HookCompleted {
                        hook_type: "session".to_string(),
                        name: "on_session_end".to_string(),
                    })?;
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(err) => {
                    emit(AgentEvent::HookFailed {
                        hook_type: "session".to_string(),
                        name: "on_session_end".to_string(),
                        error: err.to_string(),
                    })?;
                    emit(AgentEvent::Error {
                        message: format!("SessionHook.on_session_end failed: {err}"),
                        recoverable: true,
                    })?;
                }
            }
        }

        let snapshot_id: String = session.snapshot.content_hash.chars().take(12).collect();
        Ok(RunResult {
            session_id: session.id.clone(),
            turns,
            stop_reason: final_stop,
            total_input_tokens,
            total_output_tokens,
            artifacts,
            workspace_snapshot_id: Some(snapshot_id),
        })
    }

    async fn build_request<F>(
        &self,
        session: &Session,
        emit: &mut F,
        cancel_token: &crate::cancel::CancelToken,
        last_packet_hash: &mut Option<String>,
        last_prompt_source: &mut Option<String>,
    ) -> Result<ProviderRequest>
    where
        F: FnMut(AgentEvent) -> Result<()> + Send,
    {
        emit(AgentEvent::ContextBuildStarted)?;

        for hook in &self.hooks.context_hooks {
            emit(AgentEvent::HookStarted {
                hook_type: "context".to_string(),
                name: "before_context_build".to_string(),
            })?;
            let hook_res = tokio::select! {
                res = hook.before_context_build(session) => res,
                _ = cancel_token.cancelled() => {
                    emit(AgentEvent::ContextBuildFailed { reason: "cancelled".to_string() })?;
                    return Err(HarnessError::Cancelled);
                }
            };
            match hook_res {
                Ok(events) => {
                    emit(AgentEvent::HookCompleted {
                        hook_type: "context".to_string(),
                        name: "before_context_build".to_string(),
                    })?;
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(err) => {
                    emit(AgentEvent::HookFailed {
                        hook_type: "context".to_string(),
                        name: "before_context_build".to_string(),
                        error: err.to_string(),
                    })?;
                    emit(AgentEvent::Error {
                        message: format!("ContextHook.before_context_build failed: {err}"),
                        recoverable: true,
                    })?;
                }
            }
        }

        let packet = self
            .middleware
            .build_packet(&session.history, &session.token_budget);

        for hook in &self.hooks.context_hooks {
            emit(AgentEvent::HookStarted {
                hook_type: "context".to_string(),
                name: "after_context_build".to_string(),
            })?;
            let hook_res = tokio::select! {
                res = hook.after_context_build(session, &packet) => res,
                _ = cancel_token.cancelled() => {
                    emit(AgentEvent::ContextBuildFailed { reason: "cancelled".to_string() })?;
                    return Err(HarnessError::Cancelled);
                }
            };
            match hook_res {
                Ok(events) => {
                    emit(AgentEvent::HookCompleted {
                        hook_type: "context".to_string(),
                        name: "after_context_build".to_string(),
                    })?;
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(err) => {
                    emit(AgentEvent::HookFailed {
                        hook_type: "context".to_string(),
                        name: "after_context_build".to_string(),
                        error: err.to_string(),
                    })?;
                    emit(AgentEvent::Error {
                        message: format!("ContextHook.after_context_build failed: {err}"),
                        recoverable: true,
                    })?;
                }
            }
        }

        let model = if session.config.model.is_empty() {
            self.provider.default_model().to_string()
        } else {
            session.config.model.clone()
        };

        let token_estimate = self.provider.count_tokens(&model, &packet.messages)?;

        *last_packet_hash = Some(packet.packet_hash.clone());
        *last_prompt_source = packet.prompt_source.clone();

        emit(AgentEvent::ContextBuilt {
            packet_id: session.id.clone(),
            token_estimate,
            packet_hash: Some(packet.packet_hash.clone()),
            sources: Some(packet.sources),
            omissions: Some(packet.omissions),
            prompt_source: packet.prompt_source.clone(),
        })?;

        let descriptors = self.executor.tools().descriptors();
        let (tools, tool_name_map) = self.provider.adapt_tools(&descriptors);

        emit(AgentEvent::ToolCatalogSelected {
            tools: tool_name_map.clone(),
        })?;

        let request = ProviderRequest {
            model,
            messages: packet.messages,
            tools,
            tool_name_map,
            max_tokens: session.config.max_tokens,
            temperature: session.config.temperature,
            top_p: None,
            stop_sequences: Vec::new(),
            cache_plan: packet.cache_plan.clone(),
            metadata: serde_json::Value::Null,
        };

        let serialized_request = serde_json::to_string(&request).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(serialized_request.as_bytes());
        let provider_request_hash = format!("{:x}", hasher.finalize());

        emit(AgentEvent::ModelRequest {
            provider: self.provider.id().to_string(),
            model: request.model.clone(),
            packet_hash: Some(packet.packet_hash),
            temperature: request.temperature,
            max_tokens: Some(request.max_tokens as usize),
            provider_request_hash: Some(provider_request_hash),
        })?;

        Ok(request)
    }

    async fn run_turn<F>(
        &self,
        session: &mut Session,
        request: ProviderRequest,
        emit: &mut F,
        total_input_tokens: &mut usize,
        total_output_tokens: &mut usize,
        artifacts: &mut Vec<String>,
        session_grants: &mut Vec<SessionGrant>,
        current_turn: usize,
        cancel_token: &crate::cancel::CancelToken,
        sink: Option<&dyn crate::trace::TraceSink>,
    ) -> Result<TurnOutcome>
    where
        F: FnMut(AgentEvent) -> Result<()> + Send,
    {
        let tool_name_map = request.tool_name_map.clone();
        for hook in &self.hooks.model_hooks {
            emit(AgentEvent::HookStarted {
                hook_type: "model".to_string(),
                name: "before_model_request".to_string(),
            })?;
            let hook_res = tokio::select! {
                res = hook.before_model_request(session, &request) => res,
                _ = cancel_token.cancelled() => return Err(HarnessError::Cancelled),
            };
            match hook_res {
                Ok(events) => {
                    emit(AgentEvent::HookCompleted {
                        hook_type: "model".to_string(),
                        name: "before_model_request".to_string(),
                    })?;
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(err) => {
                    emit(AgentEvent::HookFailed {
                        hook_type: "model".to_string(),
                        name: "before_model_request".to_string(),
                        error: err.to_string(),
                    })?;
                    emit(AgentEvent::Error {
                        message: format!("ModelHook.before_model_request failed: {err}"),
                        recoverable: true,
                    })?;
                }
            }
        }

        let serialized_request = serde_json::to_string(&request).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(serialized_request.as_bytes());
        let provider_request_hash = format!("{:x}", hasher.finalize());

        emit(AgentEvent::ModelResponseStarted {
            provider_request_hash: provider_request_hash.clone(),
        })?;

        let stream_res = tokio::select! {
            res = self.provider.stream(request) => res,
            _ = cancel_token.cancelled() => {
                emit(AgentEvent::ModelResponseStreamInterrupted {
                    provider_request_hash: provider_request_hash.clone(),
                })?;
                return Err(HarnessError::Cancelled);
            }
        };
        let mut stream = match stream_res {
            Ok(s) => s,
            Err(e) => {
                emit(AgentEvent::ModelResponseStreamFailed {
                    provider_request_hash: provider_request_hash.clone(),
                    error: e.to_string(),
                })?;
                return Err(e);
            }
        };

        let mut accumulator = crate::turn::TurnAccumulator::default();
        let mut stop_reason = StopReason::EndTurn;
        let mut emitted_proposals = HashSet::new();

        loop {
            let next_item = tokio::select! {
                item = stream.next() => item,
                _ = cancel_token.cancelled() => {
                    emit(AgentEvent::ModelResponseStreamInterrupted {
                        provider_request_hash: provider_request_hash.clone(),
                    })?;
                    return Err(HarnessError::Cancelled);
                }
            };
            let event = match next_item {
                Some(ev) => ev,
                None => break,
            };

            match event {
                Ok(event) => {
                    let accumulated = accumulator.push(event)?;
                    for acc_ev in accumulated {
                        if let AgentEvent::ToolCallProposed { id, .. } = &acc_ev {
                            emitted_proposals.insert(id.clone());
                        }
                        emit(acc_ev.clone())?;
                        match &acc_ev {
                            AgentEvent::Usage {
                                input_tokens,
                                output_tokens,
                            } => {
                                *total_input_tokens =
                                    total_input_tokens.saturating_add(*input_tokens);
                                *total_output_tokens =
                                    total_output_tokens.saturating_add(*output_tokens);
                                session
                                    .token_budget
                                    .record_usage(*input_tokens, *output_tokens);
                            }
                            AgentEvent::Stop { reason } => {
                                stop_reason = *reason;
                            }
                            AgentEvent::Error {
                                recoverable: false,
                                message,
                            } => {
                                return Err(HarnessError::Provider(
                                    ProviderError::UnexpectedResponse {
                                        details: message.clone(),
                                    },
                                ));
                            }
                            _ => {}
                        }
                    }
                }
                Err(err) => {
                    emit(AgentEvent::ModelResponseStreamFailed {
                        provider_request_hash: provider_request_hash.clone(),
                        error: err.to_string(),
                    })?;
                    emit(AgentEvent::Error {
                        message: err.to_string(),
                        recoverable: err.is_recoverable(),
                    })?;
                    return Err(err);
                }
            }
        }

        emit(AgentEvent::ModelResponseStreamCompleted {
            provider_request_hash: provider_request_hash.clone(),
        })?;

        let assistant_turn = accumulator.finish()?;
        let tool_calls = assistant_turn.tool_calls.clone();
        let assistant_msg = assistant_turn.into_message();
        session.history.push(assistant_msg.clone());

        emit(AgentEvent::AssistantMessageCommitted {
            message: assistant_msg,
        })?;

        let assistant_turn_event = AgentEvent::Stop {
            reason: stop_reason,
        };
        for hook in &self.hooks.model_hooks {
            emit(AgentEvent::HookStarted {
                hook_type: "model".to_string(),
                name: "after_model_response".to_string(),
            })?;
            let hook_res = tokio::select! {
                res = hook.after_model_response(session, &assistant_turn_event) => res,
                _ = cancel_token.cancelled() => return Err(HarnessError::Cancelled),
            };
            match hook_res {
                Ok(events) => {
                    emit(AgentEvent::HookCompleted {
                        hook_type: "model".to_string(),
                        name: "after_model_response".to_string(),
                    })?;
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(err) => {
                    emit(AgentEvent::HookFailed {
                        hook_type: "model".to_string(),
                        name: "after_model_response".to_string(),
                        error: err.to_string(),
                    })?;
                    emit(AgentEvent::Error {
                        message: format!("ModelHook.after_model_response failed: {err}"),
                        recoverable: true,
                    })?;
                }
            }
        }

        if tool_calls.is_empty() {
            return Ok(TurnOutcome::Stop(stop_reason));
        }

        for call in &tool_calls {
            if !emitted_proposals.contains(&call.id) {
                emit(AgentEvent::ToolCallProposed {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                })?;
            }
        }

        let tool_results = self
            .executor
            .execute_tool_batch(
                session,
                tool_calls.clone(),
                &tool_name_map,
                emit,
                session_grants,
                current_turn,
                self.max_turns,
                &self.hooks,
                cancel_token,
                sink,
            )
            .await?;

        for (_, id, result, duration_ms, policy_source) in tool_results {
            let tool_name_opt = tool_calls
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone());
            let tool_name = tool_name_opt.clone().unwrap_or_default();
            let working_dir = Some(session.tool_ctx.working_dir.display().to_string());
            let artifact_refs = result
                .artifact
                .as_ref()
                .map(|art| vec![art.path.display().to_string()]);

            let output_hash = result.output_hash.clone().unwrap_or_else(|| {
                let mut hasher = Sha256::new();
                hasher.update(result.content.as_bytes());
                format!("{:x}", hasher.finalize())
            });

            if let Some(ref artifact) = result.artifact {
                emit(AgentEvent::ArtifactCreated {
                    path: artifact.path.display().to_string(),
                    size_bytes: artifact.size_bytes,
                    mime_type: artifact.mime_type.clone(),
                    hash: output_hash.clone(),
                })?;
            }

            emit(AgentEvent::ToolResult {
                id: id.clone(),
                output: result.content.clone(),
                is_error: result.is_error,
                truncated: result.truncated,
                tool_name: tool_name_opt,
                working_dir,
                duration_ms: Some(duration_ms),
                output_hash: Some(output_hash),
                artifact_refs,
                policy_source: Some(policy_source),
                failure: result.failure.clone(),
            })?;

            for hook in &self.hooks.tool_hooks {
                let mut tool_session = session.clone();
                tool_session.tool_ctx.current_tool_call_id = Some(id.clone());
                emit(AgentEvent::HookStarted {
                    hook_type: "tool".to_string(),
                    name: "after_tool_execution".to_string(),
                })?;
                let hook_res = tokio::select! {
                    res = hook.after_tool_execution(&tool_session, &tool_name, &result) => res,
                    _ = cancel_token.cancelled() => return Err(HarnessError::Cancelled),
                };
                match hook_res {
                    Ok(events) => {
                        emit(AgentEvent::HookCompleted {
                            hook_type: "tool".to_string(),
                            name: "after_tool_execution".to_string(),
                        })?;
                        for ev in events {
                            emit(ev.clone())?;
                            if let AgentEvent::VerificationResult { .. } = ev {
                                for v_hook in &self.hooks.verification_hooks {
                                    emit(AgentEvent::HookStarted {
                                        hook_type: "verification".to_string(),
                                        name: "after_verification".to_string(),
                                    })?;
                                    let v_res = tokio::select! {
                                        res = v_hook.after_verification(&tool_session, &ev) => res,
                                        _ = cancel_token.cancelled() => return Err(HarnessError::Cancelled),
                                    };
                                    match v_res {
                                        Ok(v_events) => {
                                            emit(AgentEvent::HookCompleted {
                                                hook_type: "verification".to_string(),
                                                name: "after_verification".to_string(),
                                            })?;
                                            for v_ev in v_events {
                                                emit(v_ev)?;
                                            }
                                        }
                                        Err(err) => {
                                            emit(AgentEvent::HookFailed {
                                                hook_type: "verification".to_string(),
                                                name: "after_verification".to_string(),
                                                error: err.to_string(),
                                            })?;
                                            emit(AgentEvent::Error {
                                                message: format!("VerificationHook.after_verification failed: {err}"),
                                                recoverable: true,
                                            })?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(err) => {
                        emit(AgentEvent::HookFailed {
                            hook_type: "tool".to_string(),
                            name: "after_tool_execution".to_string(),
                            error: err.to_string(),
                        })?;
                        emit(AgentEvent::Error {
                            message: format!("ToolHook.after_tool_execution failed: {err}"),
                            recoverable: true,
                        })?;
                    }
                }
            }

            if let Some(artifact) = result.artifact.as_ref() {
                artifacts.push(artifact.path.display().to_string());
            }
            session.history.push(Message::ToolResult {
                tool_use_id: id,
                content: result.content,
                is_error: result.is_error,
                failure: result.failure,
            });
        }

        Ok(TurnOutcome::ToolExecuted)
    }

    fn stop_reason(
        &self,
        session: &Session,
        turns: usize,
        outcome: TurnOutcome,
    ) -> Option<StopReason> {
        if turns >= self.max_turns {
            return Some(StopReason::MaxTurns);
        }

        if session.token_budget.exhausted() {
            return Some(StopReason::BudgetExhausted);
        }

        match outcome {
            TurnOutcome::ToolExecuted | TurnOutcome::Stop(StopReason::ToolUse) => None,
            TurnOutcome::Stop(reason) => Some(reason),
        }
    }
}
