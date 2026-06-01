use std::{collections::HashSet, sync::Arc};

use futures::StreamExt;

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
        }
    }

    pub async fn run<F>(&self, session: &mut Session, mut emit: F) -> Result<RunResult>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut turns = 0_usize;
        let mut total_input_tokens = 0_usize;
        let mut total_output_tokens = 0_usize;
        let mut artifacts = Vec::new();
        let mut session_grants: Vec<SessionGrant> = Vec::new();
        let final_stop = loop {
            if turns >= self.max_turns {
                let reason = StopReason::MaxTurns;
                emit(AgentEvent::Stop { reason });
                break reason;
            }
            if session.token_budget.exhausted() {
                let reason = StopReason::BudgetExhausted;
                emit(AgentEvent::Stop { reason });
                break reason;
            }

            let request = self.build_request(session, &mut emit)?;
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
                )
                .await?;

            turns = turns.saturating_add(1);

            if let Some(reason) = self.stop_reason(session, turns, outcome) {
                if !matches!(reason, StopReason::EndTurn | StopReason::ToolUse) {
                    emit(AgentEvent::Stop { reason });
                }

                break reason;
            }
        };

        Ok(RunResult {
            session_id: session.id.clone(),
            turns,
            stop_reason: final_stop,
            total_input_tokens,
            total_output_tokens,
            artifacts,
        })
    }

    fn build_request<F>(&self, session: &Session, emit: &mut F) -> Result<ProviderRequest>
    where
        F: FnMut(AgentEvent),
    {
        let messages = self
            .middleware
            .process(&session.history, &session.token_budget);

        let model = if session.config.model.is_empty() {
            self.provider.default_model().to_string()
        } else {
            session.config.model.clone()
        };

        let token_estimate = self.provider.count_tokens(&model, &messages)?;

        emit(AgentEvent::ContextBuilt {
            packet_id: session.id.clone(),
            token_estimate,
        });

        let request = ProviderRequest {
            model,
            messages,
            tools: self.executor.tools().schemas(),
            max_tokens: session.config.max_tokens,
            temperature: session.config.temperature,
            top_p: None,
            stop_sequences: Vec::new(),
            metadata: serde_json::Value::Null,
        };

        emit(AgentEvent::ModelRequest {
            provider: self.provider.id().to_string(),
            model: request.model.clone(),
        });

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
    ) -> Result<TurnOutcome>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut stream = self.provider.stream(request).await?;
        let mut accumulator = crate::turn::TurnAccumulator::default();
        let mut stop_reason = StopReason::EndTurn;
        let mut emitted_proposals = HashSet::new();

        while let Some(event) = stream.next().await {
            match event {
                Ok(event) => {
                    let accumulated = accumulator.push(event)?;
                    for acc_ev in accumulated {
                        if let AgentEvent::ToolCallProposed { id, .. } = &acc_ev {
                            emitted_proposals.insert(id.clone());
                        }
                        emit(acc_ev.clone());
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
                    emit(AgentEvent::Error {
                        message: err.to_string(),
                        recoverable: err.is_recoverable(),
                    });
                    return Err(err);
                }
            }
        }

        let assistant_turn = accumulator.finish()?;
        let tool_calls = assistant_turn.tool_calls.clone();
        session.history.push(assistant_turn.into_message());

        if tool_calls.is_empty() {
            return Ok(TurnOutcome::Stop(stop_reason));
        }

        for call in &tool_calls {
            if !emitted_proposals.contains(&call.id) {
                emit(AgentEvent::ToolCallProposed {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                });
            }
        }

        let tool_results = self
            .executor
            .execute_tool_batch(
                session,
                tool_calls,
                emit,
                session_grants,
                current_turn,
                self.max_turns,
            )
            .await?;

        for (_, id, result) in tool_results {
            emit(AgentEvent::ToolResult {
                id: id.clone(),
                output: result.content.clone(),
                is_error: result.is_error,
                truncated: result.truncated,
            });
            if let Some(artifact) = result.artifact.as_ref() {
                artifacts.push(artifact.path.display().to_string());
            }
            session.history.push(Message::ToolResult {
                tool_use_id: id,
                content: result.content,
                is_error: result.is_error,
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
