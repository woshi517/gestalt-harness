use std::sync::Arc;

use futures::future::join_all;
use serde_json::Value;

use crate::{
    approval::{
        hash_input, hash_input_short, ApprovalDecision, ApprovalProvider, ApprovalRequest,
        SessionGrant,
    },
    error::Result,
    event::{AgentEvent, ApprovalOutcome, PolicyStatus},
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    session::Session,
    tool::{RiskLevel, Tool, ToolCatalog, ToolContext, ToolExecutionResult},
};

pub struct ToolExecutor {
    tools: Arc<dyn ToolCatalog>,
    policy: Arc<dyn PolicyEngine>,
    approval: Arc<dyn ApprovalProvider>,
}

impl ToolExecutor {
    pub fn new(
        tools: Arc<dyn ToolCatalog>,
        policy: Arc<dyn PolicyEngine>,
        approval: Arc<dyn ApprovalProvider>,
    ) -> Self {
        Self {
            tools,
            policy,
            approval,
        }
    }

    pub fn tools(&self) -> &dyn ToolCatalog {
        self.tools.as_ref()
    }

    pub async fn execute_tool_batch<F>(
        &self,
        session: &Session,
        tool_calls: Vec<crate::turn::ProposedToolCall>,
        emit: &mut F,
        session_grants: &mut Vec<SessionGrant>,
        current_turn: usize,
        loop_max_turns: usize,
        hooks: &crate::hook::HookRegistry,
        cancel_token: &crate::cancel::CancelToken,
        sink: Option<&dyn crate::trace::TraceSink>,
    ) -> Result<Vec<(usize, String, ToolExecutionResult, u64, String)>>
    where
        F: FnMut(AgentEvent) -> crate::error::Result<()> + Send,
    {
        let mut planned = Vec::with_capacity(tool_calls.len());
        let mut denied_results = Vec::new();
        let mut confirm_queue = Vec::new();

        for (order, call) in tool_calls.into_iter().enumerate() {
            let Some(tool) = self.tools.get(&call.name) else {
                let policy = PolicyDecision::denied(
                    format!("Tool not found: {}", call.name),
                    "tool.not_found".to_string(),
                );
                emit_policy_decision(
                    emit,
                    &call.id,
                    &call.name,
                    &call.input,
                    RiskLevel::Critical,
                    session.mode,
                    &policy,
                    Some("tool.not_found".to_string()),
                )?;
                denied_results.push((
                    order,
                    call.id,
                    ToolExecutionResult::error(format!("Tool not found: {}", call.name)),
                    0,
                    "tool.not_found".to_string(),
                ));
                continue;
            };

            let risk = tool.risk(&call.input);
            let applicable_grant = self.find_applicable_grant(
                session_grants,
                &call.name,
                &call.input,
                risk,
                current_turn,
            );

            let policy = self
                .evaluate_policy(
                    session,
                    &call.id,
                    &call.name,
                    &call.input,
                    risk,
                    applicable_grant.as_ref(),
                    emit,
                    cancel_token,
                )
                .await?;

            match policy.status {
                PolicyStatus::Allowed => {
                    planned.push((order, call.id, call.name, call.input, tool, policy));
                }
                PolicyStatus::Denied => {
                    denied_results.push((
                        order,
                        call.id,
                        ToolExecutionResult::error(
                            policy.reason.clone().unwrap_or_else(|| {
                                format!("policy denied tool call {}", call.name)
                            }),
                        ),
                        0,
                        policy.policy_source.clone(),
                    ));
                }
                PolicyStatus::Confirm => {
                    confirm_queue.push((order, call.id, call.name, call.input, tool, policy));
                }
            }
        }

        for (order, id, name, input, tool, policy) in confirm_queue {
            let call_id = id.clone();
            let original_input_hash = hash_input(&input);

            emit(AgentEvent::ApprovalRequested {
                tool_call_id: id.clone(),
                tool_name: name.clone(),
                input: input.clone(),
                risk: tool.risk(&input),
            })?;

            let approval_res = self
                .approval
                .approve_cancellable(
                    ApprovalRequest {
                        tool_call_id: id.clone(),
                        tool_name: name.clone(),
                        input: input.clone(),
                        description: tool.description().to_string(),
                        decision: policy.clone(),
                    },
                    cancel_token,
                )
                .await;

            let approval = match approval_res {
                Ok(a) => a,
                Err(crate::error::HarnessError::Cancelled) => {
                    emit(AgentEvent::ApprovalCancelled {
                        tool_call_id: call_id.clone(),
                    })?;
                    return Err(crate::error::HarnessError::Cancelled);
                }
                Err(e) => return Err(e),
            };

            let (outcome, edited_input_hash, grant_terms) = match approval {
                ApprovalDecision::Approve => {
                    planned.push((order, id, name, input, tool, policy));
                    (ApprovalOutcome::Approve, None, None)
                }
                ApprovalDecision::Deny => {
                    denied_results.push((
                        order,
                        id,
                        ToolExecutionResult::error(
                            policy
                                .reason
                                .clone()
                                .unwrap_or_else(|| format!("approval denied tool call {name}")),
                        ),
                        0,
                        policy.policy_source.clone(),
                    ));
                    (ApprovalOutcome::Deny, None, None)
                }
                ApprovalDecision::Edit(new_input) => {
                    let edited_hash = hash_input(&new_input);
                    let new_risk = tool.risk(&new_input);
                    let re_evaluated = self
                        .evaluate_policy(
                            session,
                            &id,
                            &name,
                            &new_input,
                            new_risk,
                            None,
                            emit,
                            cancel_token,
                        )
                        .await?;
                    match re_evaluated.status {
                        PolicyStatus::Allowed => {
                            planned.push((order, id, name, new_input, tool, re_evaluated));
                        }
                        PolicyStatus::Denied => {
                            denied_results.push((
                                order,
                                id,
                                ToolExecutionResult::error(
                                    re_evaluated.reason.clone().unwrap_or_else(|| {
                                        format!("policy denied edited tool call {name}")
                                    }),
                                ),
                                0,
                                re_evaluated.policy_source.clone(),
                            ));
                        }
                        PolicyStatus::Confirm => {
                            denied_results.push((
                                order,
                                id,
                                ToolExecutionResult::error(format!(
                                    "approval edit still requires confirmation for {name}"
                                )),
                                0,
                                re_evaluated.policy_source.clone(),
                            ));
                        }
                    }
                    (ApprovalOutcome::Edit, Some(edited_hash), None)
                }
                ApprovalDecision::AlwaysAllowForSession => {
                    let risk_ceiling = tool.risk(&input);
                    let grant = SessionGrant::new(
                        name.clone(),
                        &input,
                        risk_ceiling,
                        policy.policy_source.clone(),
                        "session_grant",
                        current_turn,
                        loop_max_turns.max(1),
                    );
                    session_grants.push(grant.clone());
                    planned.push((order, id, name, input, tool, policy));
                    (ApprovalOutcome::AlwaysAllow, None, Some(grant))
                }
            };

            emit(AgentEvent::ApprovalDecision {
                tool_call_id: call_id,
                decision: outcome,
                original_input_hash,
                edited_input_hash,
                grant_terms,
            })?;
        }

        let mut results = denied_results;
        let mut current_parallel = Vec::new();

        for (order, id, name, input, tool, policy) in planned {
            let mut tool_ctx = session.tool_ctx.clone();
            tool_ctx.current_tool_call_id = Some(id.clone());
            let mut tool_session = session.clone();
            tool_session.tool_ctx = tool_ctx.clone();

            if tool.can_run_in_parallel(&input) {
                emit_tool_hooks_before(hooks, &tool_session, &name, &input, emit, cancel_token)
                    .await?;
                current_parallel.push((order, id, name, input, tool, policy, tool_ctx));
                continue;
            }

            if !current_parallel.is_empty() {
                for (_, id, name, input, _, policy, tool_ctx) in &current_parallel {
                    let input_hash = hash_input(input);
                    emit(AgentEvent::ToolExecutionStarted {
                        id: id.clone(),
                        tool_name: name.clone(),
                        input_hash,
                        policy_source: policy.policy_source.clone(),
                        working_dir: tool_ctx.working_dir.display().to_string(),
                        parallel_group_id: Some("group-1".to_string()),
                        parallel_safe: true,
                    })?;
                }
                if let Some(s) = sink {
                    let _ = s.flush();
                }

                let cancel_clone = cancel_token.clone();
                let futures = std::mem::take(&mut current_parallel).into_iter().map(
                    |(order, id, _name, input, tool, policy, tool_ctx)| {
                        let tool_call_id = id.clone();
                        let c = cancel_clone.clone();
                        async move {
                            let start = std::time::Instant::now();
                            let result =
                                execute_tool(tool.as_ref(), input, &tool_ctx, &tool_call_id, &c)
                                    .await;
                            let duration =
                                u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                            (order, id, result, duration, policy.policy_source)
                        }
                    },
                );

                let parallel_results = tokio::select! {
                    res = join_all(futures) => res,
                    _ = cancel_token.cancelled() => {
                        return Err(crate::error::HarnessError::Cancelled);
                    }
                };
                for (order, id, result, duration, policy_source) in parallel_results {
                    results.push((order, id, result, duration, policy_source));
                }
            }

            emit_tool_hooks_before(hooks, &tool_session, &name, &input, emit, cancel_token).await?;
            let input_hash = hash_input(&input);
            emit(AgentEvent::ToolExecutionStarted {
                id: id.clone(),
                tool_name: name.clone(),
                input_hash,
                policy_source: policy.policy_source.clone(),
                working_dir: tool_ctx.working_dir.display().to_string(),
                parallel_group_id: None,
                parallel_safe: false,
            })?;
            if let Some(s) = sink {
                let _ = s.flush();
            }

            let start = std::time::Instant::now();
            let execute_future = execute_tool(tool.as_ref(), input, &tool_ctx, &id, cancel_token);
            let result = tokio::select! {
                res = execute_future => res,
                _ = cancel_token.cancelled() => {
                    return Err(crate::error::HarnessError::Cancelled);
                }
            };
            let duration = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            results.push((order, id, result, duration, policy.policy_source));
        }

        if !current_parallel.is_empty() {
            for (_, id, name, input, _, policy, tool_ctx) in &current_parallel {
                let input_hash = hash_input(input);
                emit(AgentEvent::ToolExecutionStarted {
                    id: id.clone(),
                    tool_name: name.clone(),
                    input_hash,
                    policy_source: policy.policy_source.clone(),
                    working_dir: tool_ctx.working_dir.display().to_string(),
                    parallel_group_id: Some("group-2".to_string()),
                    parallel_safe: true,
                })?;
            }
            if let Some(s) = sink {
                let _ = s.flush();
            }

            let cancel_clone = cancel_token.clone();
            let futures = current_parallel.into_iter().map(
                |(order, id, _name, input, tool, policy, tool_ctx)| {
                    let tool_call_id = id.clone();
                    let c = cancel_clone.clone();
                    async move {
                        let start = std::time::Instant::now();
                        let result =
                            execute_tool(tool.as_ref(), input, &tool_ctx, &tool_call_id, &c).await;
                        let duration =
                            u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                        (order, id, result, duration, policy.policy_source)
                    }
                },
            );

            let parallel_results = tokio::select! {
                res = join_all(futures) => res,
                _ = cancel_token.cancelled() => {
                    return Err(crate::error::HarnessError::Cancelled);
                }
            };
            for (order, id, result, duration, policy_source) in parallel_results {
                results.push((order, id, result, duration, policy_source));
            }
        }

        results.sort_by_key(|left| left.0);
        Ok(results)
    }

    async fn evaluate_policy(
        &self,
        session: &Session,
        tool_call_id: &str,
        tool_name: &str,
        input: &Value,
        risk: RiskLevel,
        grant: Option<&SessionGrant>,
        emit: &mut impl FnMut(AgentEvent) -> crate::error::Result<()>,
        cancel_token: &crate::cancel::CancelToken,
    ) -> Result<PolicyDecision> {
        emit(AgentEvent::PolicyEvaluationStarted {
            tool_call_id: tool_call_id.to_string(),
        })?;

        // Network policy check
        if tool_name == "bash" && !session.tool_ctx.allow_network {
            let command = input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !crate::tool::is_audited_local_command(command) {
                let reason = format!(
                    "Command '{command}' violates network policy (no network access allowed)"
                );
                emit(AgentEvent::PolicyViolation {
                    tool_call_id: tool_call_id.to_string(),
                    tool_name: tool_name.to_string(),
                    reason: reason.clone(),
                })?;
                let decision = PolicyDecision {
                    status: PolicyStatus::Denied,
                    reason: Some(reason),
                    policy_source: "policy_violation:network_denied".to_string(),
                };
                emit_policy_decision(
                    emit,
                    tool_call_id,
                    tool_name,
                    input,
                    risk,
                    session.mode,
                    &decision,
                    Some("policy_violation:network_denied".to_string()),
                )?;
                return Ok(decision);
            }
        }

        let request = PolicyRequest {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            input: input.clone(),
            risk,
            mode: session.mode,
            working_dir: session.tool_ctx.working_dir.clone(),
            workspace_root: session.tool_ctx.workspace_root.clone(),
            user_approved: false,
        };

        let decision_res = self
            .policy
            .evaluate_cancellable(request, cancel_token)
            .await;

        let decision = match decision_res {
            Ok(d) => d,
            Err(crate::error::HarnessError::Cancelled) => {
                emit(AgentEvent::PolicyEvaluationCancelled {
                    tool_call_id: tool_call_id.to_string(),
                })?;
                return Err(crate::error::HarnessError::Cancelled);
            }
            Err(e) => {
                emit(AgentEvent::PolicyEvaluationFailed {
                    tool_call_id: tool_call_id.to_string(),
                    error: e.to_string(),
                })?;
                return Err(e);
            }
        };

        let final_decision = if let Some(grant) = grant {
            apply_grant_override(decision, grant, tool_name)
        } else {
            decision
        };

        let matched_rule = grant.map_or(final_decision.policy_source.as_str(), |grant| {
            grant.matched_rule.as_str()
        });

        emit_policy_decision(
            emit,
            tool_call_id,
            tool_name,
            input,
            risk,
            session.mode,
            &final_decision,
            Some(matched_rule.to_string()),
        )?;
        Ok(final_decision)
    }

    fn find_applicable_grant(
        &self,
        grants: &[SessionGrant],
        tool_name: &str,
        input: &Value,
        risk: RiskLevel,
        current_turn: usize,
    ) -> Option<SessionGrant> {
        grants
            .iter()
            .rev()
            .find(|grant| grant.covers(tool_name, input, risk, current_turn))
            .cloned()
    }
}

fn apply_grant_override(
    policy: PolicyDecision,
    grant: &SessionGrant,
    tool_name: &str,
) -> PolicyDecision {
    match policy.status {
        PolicyStatus::Denied => policy,
        PolicyStatus::Allowed | PolicyStatus::Confirm => {
            let tool_tag = hash_input_short(&Value::String(tool_name.to_string()));
            let policy_source = format!("session_grant:{}:{}", grant.matched_rule, tool_tag);
            PolicyDecision {
                status: PolicyStatus::Allowed,
                reason: Some(format!(
                    "matched session grant (rule={}, risk_ceiling={:?})",
                    grant.matched_rule, grant.risk_ceiling
                )),
                policy_source,
            }
        }
    }
}

fn emit_policy_decision(
    emit: &mut impl FnMut(AgentEvent) -> crate::error::Result<()>,
    tool_call_id: &str,
    tool_name: &str,
    input: &Value,
    risk: RiskLevel,
    mode: crate::session::ExecutionMode,
    decision: &PolicyDecision,
    matched_rule: Option<String>,
) -> crate::error::Result<()> {
    emit(AgentEvent::PolicyDecision {
        tool_call_id: tool_call_id.to_string(),
        tool_name: Some(tool_name.to_string()),
        input_hash: Some(hash_input(input)),
        risk: Some(risk),
        mode: Some(mode),
        matched_rule,
        decision: decision.status,
        reason: decision.reason.clone(),
        policy_source: decision.policy_source.clone(),
    })
}

async fn emit_tool_hooks_before<F>(
    hooks: &crate::hook::HookRegistry,
    session: &Session,
    tool_name: &str,
    input: &Value,
    emit: &mut F,
    cancel_token: &crate::cancel::CancelToken,
) -> crate::error::Result<()>
where
    F: FnMut(AgentEvent) -> crate::error::Result<()> + Send,
{
    for hook in &hooks.tool_hooks {
        emit(AgentEvent::HookStarted {
            hook_type: "tool".to_string(),
            name: "before_tool_execution".to_string(),
        })?;
        let hook_res = tokio::select! {
            res = hook.before_tool_execution(session, tool_name, input) => res,
            _ = cancel_token.cancelled() => return Err(crate::error::HarnessError::Cancelled),
        };
        match hook_res {
            Ok(events) => {
                emit(AgentEvent::HookCompleted {
                    hook_type: "tool".to_string(),
                    name: "before_tool_execution".to_string(),
                })?;
                for ev in events {
                    emit(ev)?;
                }
            }
            Err(err) => {
                emit(AgentEvent::HookFailed {
                    hook_type: "tool".to_string(),
                    name: "before_tool_execution".to_string(),
                    error: err.to_string(),
                })?;
                emit(AgentEvent::Error {
                    message: format!("ToolHook.before_tool_execution failed: {err}"),
                    recoverable: true,
                })?;
            }
        }
    }
    Ok(())
}

pub fn emit_tool_call_proposals<F>(emit: &mut F, tool_calls: &[crate::turn::ProposedToolCall])
where
    F: FnMut(AgentEvent),
{
    for call in tool_calls {
        emit(AgentEvent::ToolCallProposed {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
        });
    }
}

pub async fn execute_tool(
    tool: &dyn Tool,
    input: Value,
    ctx: &ToolContext,
    tool_call_id: &str,
    cancel_token: &crate::cancel::CancelToken,
) -> ToolExecutionResult {
    tokio::select! {
        res = tokio::time::timeout(ctx.timeout, tool.execute(input, ctx)) => {
            match res {
                Ok(Ok(output)) => output
                    .into_execution_result(false, ctx.max_output_bytes, ctx, tool_call_id)
                    .unwrap_or_else(|err| ToolExecutionResult::error(err.to_string())),
                Ok(Err(err)) => ToolExecutionResult::error(err.to_string()),
                Err(_) => ToolExecutionResult::error(format!(
                    "tool timed out after {}s: {}",
                    ctx.timeout.as_secs(),
                    tool.name()
                )),
            }
        }
        _ = cancel_token.cancelled() => {
            ToolExecutionResult::error("tool execution interrupted by user".to_string())
        }
    }
}
