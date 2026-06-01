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
    ) -> Result<Vec<(usize, String, ToolExecutionResult)>>
    where
        F: FnMut(AgentEvent) + Send,
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
                );
                denied_results.push((
                    order,
                    call.id,
                    ToolExecutionResult::error(format!("Tool not found: {}", call.name)),
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
                            policy.reason.unwrap_or_else(|| {
                                format!("policy denied tool call {}", call.name)
                            }),
                        ),
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
            let approval = self
                .approval
                .approve(ApprovalRequest {
                    tool_call_id: id.clone(),
                    tool_name: name.clone(),
                    input: input.clone(),
                    decision: policy.clone(),
                    description: tool.description().to_string(),
                })
                .await;

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
                    ));
                    (ApprovalOutcome::Deny, None, None)
                }
                ApprovalDecision::Edit(new_input) => {
                    let edited_hash = hash_input(&new_input);
                    let new_risk = tool.risk(&new_input);
                    let re_evaluated = self
                        .evaluate_policy(session, &id, &name, &new_input, new_risk, None, emit)
                        .await?;
                    match re_evaluated.status {
                        PolicyStatus::Allowed => {
                            planned.push((order, id, name, new_input, tool, re_evaluated));
                        }
                        PolicyStatus::Denied => {
                            denied_results.push((
                                order,
                                id,
                                ToolExecutionResult::error(re_evaluated.reason.unwrap_or_else(
                                    || format!("policy denied edited tool call {name}"),
                                )),
                            ));
                        }
                        PolicyStatus::Confirm => {
                            denied_results.push((
                                order,
                                id,
                                ToolExecutionResult::error(format!(
                                    "approval edit still requires confirmation for {name}"
                                )),
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
            });
        }

        let mut results = denied_results;
        let mut current_parallel = Vec::new();

        for (order, id, name, input, tool, _policy) in planned {
            if tool.can_run_in_parallel(&input) {
                current_parallel.push((order, id, name, input, tool));
            } else {
                if !current_parallel.is_empty() {
                    let futures = std::mem::take(&mut current_parallel).into_iter().map(
                        |(order, id, _name, input, tool)| {
                            let tool_ctx = session.tool_ctx.clone();
                            async move {
                                let result = execute_tool(tool.as_ref(), input, &tool_ctx).await;
                                (order, id, result)
                            }
                        },
                    );
                    let parallel_results = join_all(futures).await;
                    for (order, id, result) in parallel_results {
                        results.push((order, id, result));
                    }
                }
                let result = execute_tool(tool.as_ref(), input, &session.tool_ctx).await;
                results.push((order, id, result));
            }
        }

        if !current_parallel.is_empty() {
            let futures = current_parallel
                .into_iter()
                .map(|(order, id, _name, input, tool)| {
                    let tool_ctx = session.tool_ctx.clone();
                    async move {
                        let result = execute_tool(tool.as_ref(), input, &tool_ctx).await;
                        (order, id, result)
                    }
                });
            let parallel_results = join_all(futures).await;
            for (order, id, result) in parallel_results {
                results.push((order, id, result));
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
        emit: &mut impl FnMut(AgentEvent),
    ) -> Result<PolicyDecision> {
        let decision = self
            .policy
            .evaluate(PolicyRequest {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                input: input.clone(),
                risk,
                mode: session.mode,
                working_dir: session.tool_ctx.working_dir.clone(),
                workspace_root: session.tool_ctx.workspace_root.clone(),
                user_approved: false,
            })
            .await;

        let final_decision = if let Some(grant) = grant {
            apply_grant_override(decision, grant, tool_name)
        } else {
            decision
        };

        let matched_rule_id = grant
            .map(|grant| grant.matched_rule.as_str())
            .unwrap_or(final_decision.policy_source.as_str());

        emit_policy_decision(
            emit,
            tool_call_id,
            tool_name,
            input,
            risk,
            session.mode,
            &final_decision,
            Some(matched_rule_id.to_string()),
        );
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
    emit: &mut impl FnMut(AgentEvent),
    tool_call_id: &str,
    tool_name: &str,
    input: &Value,
    risk: RiskLevel,
    execution_mode: crate::session::ExecutionMode,
    decision: &PolicyDecision,
    matched_rule_id: Option<String>,
) {
    emit(AgentEvent::PolicyDecision {
        tool_call_id: tool_call_id.to_string(),
        tool_name: Some(tool_name.to_string()),
        input_hash: Some(hash_input(input)),
        risk: Some(risk),
        execution_mode: Some(execution_mode),
        matched_rule_id,
        decision: decision.status,
        reason: decision.reason.clone(),
        policy_source: decision.policy_source.clone(),
    });
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

pub async fn execute_tool(tool: &dyn Tool, input: Value, ctx: &ToolContext) -> ToolExecutionResult {
    match tokio::time::timeout(ctx.timeout, tool.execute(input, ctx)).await {
        Ok(Ok(output)) => output.into_execution_result(false, ctx.max_output_bytes),
        Ok(Err(err)) => ToolExecutionResult::error(err.to_string()),
        Err(_) => ToolExecutionResult::error(format!(
            "tool timed out after {}s: {}",
            ctx.timeout.as_secs(),
            tool.name()
        )),
    }
}
