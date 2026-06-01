use futures::future::join_all;
use serde_json::Value;
use std::{collections::HashSet, sync::Arc};

use crate::{
    approval::{ApprovalDecision, ApprovalProvider, ApprovalRequest},
    error::Result,
    event::{AgentEvent, PolicyStatus},
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    session::Session,
    tool::{Tool, ToolCatalog, ToolContext, ToolExecutionResult},
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
        allowed_session_tools: &mut HashSet<String>,
    ) -> Result<Vec<(usize, String, ToolExecutionResult)>>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut planned = Vec::with_capacity(tool_calls.len());
        let mut denied_results = Vec::new();
        let mut confirm_queue = Vec::new();

        for (order, call) in tool_calls.into_iter().enumerate() {
            let Some(tool) = self.tools.get(&call.name) else {
                denied_results.push((
                    order,
                    call.id,
                    ToolExecutionResult::error(format!("Tool not found: {}", call.name)),
                ));
                continue;
            };

            let policy = if allowed_session_tools.contains(&call.name) {
                let p = PolicyDecision {
                    status: PolicyStatus::Allowed,
                    reason: Some("Session approved".to_string()),
                    policy_source: "session_bypass".to_string(),
                };
                emit(AgentEvent::PolicyDecision {
                    tool_call_id: call.id.clone(),
                    decision: p.status,
                    reason: p.reason.clone(),
                });
                p
            } else {
                self.evaluate_policy(
                    session,
                    &call.id,
                    &call.name,
                    &call.input,
                    tool.as_ref(),
                    emit,
                )
                .await?
            };

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

            match approval {
                ApprovalDecision::Approve => {
                    planned.push((order, id, name, input, tool, policy));
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
                }
                ApprovalDecision::Edit(new_input) => {
                    let policy = self
                        .evaluate_policy(session, &id, &name, &new_input, tool.as_ref(), emit)
                        .await?;
                    match policy.status {
                        PolicyStatus::Allowed => {
                            planned.push((order, id, name, new_input, tool, policy));
                        }
                        PolicyStatus::Denied => {
                            denied_results.push((
                                order,
                                id,
                                ToolExecutionResult::error(policy.reason.unwrap_or_else(|| {
                                    format!("policy denied edited tool call {name}")
                                })),
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
                }
                ApprovalDecision::AlwaysAllowForSession => {
                    allowed_session_tools.insert(name.clone());
                    planned.push((order, id, name, input, tool, policy));
                }
            }
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
        tool: &dyn Tool,
        emit: &mut impl FnMut(AgentEvent),
    ) -> Result<PolicyDecision> {
        let decision = self
            .policy
            .evaluate(PolicyRequest {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                input: input.clone(),
                risk: tool.risk(input),
                mode: session.mode,
                working_dir: session.tool_ctx.working_dir.clone(),
                workspace_root: session.tool_ctx.workspace_root.clone(),
                user_approved: false,
            })
            .await;

        emit(AgentEvent::PolicyDecision {
            tool_call_id: tool_call_id.to_string(),
            decision: decision.status,
            reason: decision.reason.clone(),
        });
        Ok(decision)
    }
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
