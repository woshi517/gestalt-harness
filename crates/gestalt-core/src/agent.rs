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
    steering_queue: Option<Arc<dyn crate::session_queue::SteeringQueue>>,
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
            steering_queue: None,
        }
    }

    pub fn with_hooks(mut self, hooks: crate::hook::HookRegistry) -> Self {
        self.hooks = hooks;
        self
    }

    pub fn with_steering_queue(
        mut self,
        queue: Arc<dyn crate::session_queue::SteeringQueue>,
    ) -> Self {
        self.steering_queue = Some(queue);
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
            let hook_res = crate::hook::HookDispatcher::dispatch(
                "session",
                "on_session_start",
                cancel_token,
                &mut emit,
                || hook.on_session_start(session),
            )
            .await;
            match hook_res {
                Ok(events) => {
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(HarnessError::Cancelled) => {
                    return Err(HarnessError::Cancelled);
                }
                Err(err) => {
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

            let injected_count = self
                .drain_session_messages(session, &mut emit, turns, cancel_token)
                .await?;

            if injected_count > 0 {
                emit_checkpoint(
                    &mut emit,
                    session,
                    last_packet_hash.clone(),
                    last_prompt_source.clone(),
                )?;
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

            if let Some(block_reason) = self
                .invoke_next_turn_hooks(session, turns, &mut emit, cancel_token)
                .await?
            {
                let reason = StopReason::HookBlocked;
                emit(AgentEvent::HookFailed {
                    hook_type: "next_turn".to_string(),
                    name: "prepare_next_turn".to_string(),
                    error: block_reason.clone(),
                })?;
                emit(AgentEvent::Error {
                    message: block_reason,
                    recoverable: false,
                })?;
                emit(AgentEvent::Stop { reason })?;
                break reason;
            }
        };

        if let Some(ref queue) = self.steering_queue {
            // `Closing` belongs to AgentLoop, not runtime: this is the exact
            // boundary where no further drain/build_request cycle can occur,
            // so late steering must stop being accepted before session-end
            // hooks run.
            let _ = queue
                .update_lifecycle(crate::session_queue::QueueLifecycle::Closing)
                .await;
        }

        for hook in &self.hooks.session_hooks {
            let hook_res = crate::hook::HookDispatcher::dispatch(
                "session",
                "on_session_end",
                cancel_token,
                &mut emit,
                || hook.on_session_end(session),
            )
            .await;
            match hook_res {
                Ok(events) => {
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(HarnessError::Cancelled) => {
                    return Err(HarnessError::Cancelled);
                }
                Err(err) => {
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
        session: &mut Session,
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
            let hook_res = crate::hook::HookDispatcher::dispatch(
                "context",
                "before_context_build",
                cancel_token,
                &mut *emit,
                || hook.before_context_build(session),
            )
            .await;
            match hook_res {
                Ok(events) => {
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(HarnessError::Cancelled) => {
                    emit(AgentEvent::ContextBuildFailed {
                        reason: "cancelled".to_string(),
                    })?;
                    return Err(HarnessError::Cancelled);
                }
                Err(err) => {
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
            let hook_res = crate::hook::HookDispatcher::dispatch(
                "context",
                "after_context_build",
                cancel_token,
                &mut *emit,
                || hook.after_context_build(session, &packet),
            )
            .await;
            match hook_res {
                Ok(events) => {
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(HarnessError::Cancelled) => {
                    emit(AgentEvent::ContextBuildFailed {
                        reason: "cancelled".to_string(),
                    })?;
                    return Err(HarnessError::Cancelled);
                }
                Err(err) => {
                    emit(AgentEvent::Error {
                        message: format!("ContextHook.after_context_build failed: {err}"),
                        recoverable: true,
                    })?;
                }
            }
        }

        let override_state = session.next_turn_override.take();
        if let Some(ref pending) = override_state {
            if let Some(ref provider_override) = pending.provider {
                if provider_override != self.provider.id() {
                    emit(AgentEvent::HookFailed {
                        hook_type: "next_turn".to_string(),
                        name: "prepare_next_turn".to_string(),
                        error: format!(
                            "Cross-provider override requested for '{}', but current provider is '{}'; using model override only",
                            provider_override,
                            self.provider.id(),
                        ),
                    })?;
                    emit(AgentEvent::Error {
                        message: format!(
                            "Cross-provider override requested for '{}', but current provider is '{}'; using model override only",
                            provider_override,
                            self.provider.id(),
                        ),
                        recoverable: true,
                    })?;
                }
            }
        }

        let effective_model = match override_state {
            Some(override_state) => {
                if override_state.model.is_empty() {
                    session.config.model.clone()
                } else {
                    override_state.model
                }
            }
            None => session.config.model.clone(),
        };

        let model = if effective_model.is_empty() {
            self.provider.default_model().to_string()
        } else {
            effective_model
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
            top_p: session.config.top_p,
            stop_sequences: Vec::new(),
            cache_plan: packet.cache_plan.clone(),
            metadata: session.config.metadata.clone(),
            reasoning_effort: session.config.reasoning_effort,
            text_verbosity: session.config.text_verbosity,
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
            let hook_res = crate::hook::HookDispatcher::dispatch(
                "model",
                "before_model_request",
                cancel_token,
                &mut *emit,
                || hook.before_model_request(session, &request),
            )
            .await;
            match hook_res {
                Ok(events) => {
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(HarnessError::Cancelled) => {
                    return Err(HarnessError::Cancelled);
                }
                Err(err) => {
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
            let hook_res = crate::hook::HookDispatcher::dispatch(
                "model",
                "after_model_response",
                cancel_token,
                &mut *emit,
                || hook.after_model_response(session, &assistant_turn_event),
            )
            .await;
            match hook_res {
                Ok(events) => {
                    for ev in events {
                        emit(ev)?;
                    }
                }
                Err(HarnessError::Cancelled) => {
                    return Err(HarnessError::Cancelled);
                }
                Err(err) => {
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
                tool_name: tool_name_opt.clone(),
                working_dir,
                duration_ms: Some(duration_ms),
                output_hash: Some(output_hash.clone()),
                artifact_refs: artifact_refs.clone(),
                policy_source: Some(policy_source),
                failure: result.failure.clone(),
            })?;

            for hook in &self.hooks.tool_hooks {
                let mut tool_session = session.clone();
                tool_session.tool_ctx.current_tool_call_id = Some(id.clone());
                let hook_res = crate::hook::HookDispatcher::dispatch(
                    "tool",
                    "after_tool_execution",
                    cancel_token,
                    &mut *emit,
                    || hook.after_tool_execution(&tool_session, &tool_name, &result),
                )
                .await;
                match hook_res {
                    Ok(events) => {
                        for ev in events {
                            emit(ev.clone())?;
                            if let AgentEvent::VerificationResult { .. } = ev {
                                for v_hook in &self.hooks.verification_hooks {
                                    let v_res = crate::hook::HookDispatcher::dispatch(
                                        "verification",
                                        "after_verification",
                                        cancel_token,
                                        &mut *emit,
                                        || v_hook.after_verification(&tool_session, &ev),
                                    )
                                    .await;
                                    match v_res {
                                        Ok(v_events) => {
                                            for v_ev in v_events {
                                                emit(v_ev)?;
                                            }
                                        }
                                        Err(HarnessError::Cancelled) => {
                                            return Err(HarnessError::Cancelled);
                                        }
                                        Err(err) => {
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
                    Err(HarnessError::Cancelled) => {
                        return Err(HarnessError::Cancelled);
                    }
                    Err(err) => {
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
                tool_name: tool_name_opt,
                output_hash: Some(output_hash),
                artifact_refs,
            });
        }

        Ok(TurnOutcome::ToolExecuted)
    }

    async fn invoke_next_turn_hooks<F>(
        &self,
        session: &mut Session,
        current_turn: usize,
        emit: &mut F,
        cancel_token: &crate::cancel::CancelToken,
    ) -> Result<Option<String>>
    where
        F: FnMut(AgentEvent) -> Result<()> + Send,
    {
        for hook in &self.hooks.next_turn_hooks {
            let hook_res = crate::hook::HookDispatcher::dispatch(
                "next_turn",
                "prepare_next_turn",
                cancel_token,
                &mut *emit,
                || hook.prepare_next_turn(session, current_turn),
            )
            .await;
            match hook_res {
                Ok(events) => {
                    for ev in events {
                        match &ev {
                            AgentEvent::NextTurnOverrideRequested { model, provider, variant } => {
                                let effective_model = if model.is_empty() {
                                    session.config.model.clone()
                                } else {
                                    model.clone()
                                };
                                session.next_turn_override =
                                    Some(crate::session::NextTurnOverride {
                                        model: effective_model,
                                        provider: provider.clone(),
                                        variant: variant.clone(),
                                    });
                            }
                            AgentEvent::NextTurnBlocked { reason } => {
                                emit(ev.clone())?;
                                return Ok(Some(reason.clone()));
                            }
                            _ => {}
                        }
                        emit(ev)?;
                    }
                }
                Err(HarnessError::Cancelled) => {
                    return Err(HarnessError::Cancelled);
                }
                Err(err) => {
                    emit(AgentEvent::Error {
                        message: format!("NextTurnHook.prepare_next_turn failed: {err}"),
                        recoverable: true,
                    })?;
                }
            }
        }

        Ok(None)
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

    async fn drain_session_messages<F>(
        &self,
        session: &mut Session,
        emit: &mut F,
        current_turn: usize,
        cancel_token: &crate::cancel::CancelToken,
    ) -> Result<usize>
    where
        F: FnMut(AgentEvent) -> Result<()> + Send,
    {
        let queue = match &self.steering_queue {
            Some(q) => q,
            None => return Ok(0),
        };

        if cancel_token.is_cancelled() {
            return Err(HarnessError::Cancelled);
        }

        let mut messages = queue.drain().await?;
        if messages.is_empty() {
            return Ok(0);
        }

        let count = messages.len();

        for msg in &mut messages {
            msg.injected_at_turn = Some(current_turn);

            let content_block = crate::message::ContentBlock::Text {
                text: msg.content.clone(),
            };
            let history_msg = crate::message::Message::User {
                content: vec![content_block],
                metadata: Some(crate::message::MessageMetadata {
                    source: Some(msg.source),
                    queued_message_id: Some(msg.id.clone()),
                    injected_at_turn: msg.injected_at_turn,
                }),
            };
            session.history.push(history_msg);

            emit(AgentEvent::SessionMessageInjected {
                message: msg.clone(),
            })?;
        }

        emit(AgentEvent::SessionMessageQueueDrained { count })?;

        Ok(count)
    }
}
