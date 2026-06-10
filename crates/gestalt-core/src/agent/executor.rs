use std::collections::HashSet;
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
    tool_descriptor::{ToolAnnotations, ToolDescriptor, ToolNamespace, ToolRetryPolicy},
    tool_failure::{ToolErrorReport, ToolFailureKind},
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
        tool_name_mappings: &[crate::tool_name_mapping::ToolNameMapping],
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
        let mut seen_ids = HashSet::new();
        let mut validated_calls = Vec::with_capacity(tool_calls.len());

        // 1. Early validation loop
        for (order, call) in tool_calls.into_iter().enumerate() {
            match crate::tool_validation::ToolCallValidator::validate(
                &call,
                self.tools.as_ref(),
                tool_name_mappings,
                &mut seen_ids,
            ) {
                Ok(()) => {
                    validated_calls.push((order, call));
                }
                Err(report) => {
                    emit(AgentEvent::ToolCallValidationFailed {
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        error: report.clone(),
                    })?;

                    denied_results.push((
                        order,
                        call.id,
                        ToolExecutionResult::error_with_failure(report),
                        0,
                        "validation_failed".to_string(),
                    ));
                }
            }
        }

        // 1b. Strict schema validation against the provider-rendered
        // contract. This catches `additionalProperties` violations and
        // type mismatches the raw descriptor check would miss. We
        // run it after the basic existence/duplicate checks so the
        // canonical ID resolution has already happened.
        let mut strict_denials = Vec::new();
        for (order, call) in &validated_calls {
            let mapping = tool_name_mappings
                .iter()
                .find(|m| m.provider_name == call.name);
            let Some(mapping) = mapping else { continue };
            if mapping.strict != Some(true) {
                continue;
            }
            let Some(input_schema) = mapping.input_schema.as_ref() else {
                continue;
            };
            let canonical_id = resolve_tool_id(&call.name, tool_name_mappings);
            let Some(tool) = self.tools.get_by_id(&canonical_id) else {
                continue;
            };
            // Re-validate the input against the strict schema the
            // provider actually saw. The basic validator already passed
            // against the raw schema; the strict check is a refinement
            // and we only emit a failure if it surfaces a real
            // mismatch.
            if let Err(reason) = crate::tool_validation::ToolCallValidator::validate_against_strict(
                &call.input,
                input_schema,
            ) {
                let report = ToolErrorReport::new(
                    ToolFailureKind::SchemaMismatch,
                    format!(
                        "Strict schema validation failed for tool '{}': {reason}",
                        call.name
                    ),
                )
                .with_repair(format!(
                    "Provider-strict input contract was not satisfied. Expected schema: {}",
                    serde_json::to_string(input_schema).unwrap_or_default()
                ));
                emit(AgentEvent::ToolCallValidationFailed {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    error: report.clone(),
                })?;
                strict_denials.push((*order, call.id.clone(), report, tool));
            }
        }
        for (order, id, report, _tool) in strict_denials {
            validated_calls.retain(|(o, c)| !(*o == order && c.id == id));
            denied_results.push((
                order,
                id,
                ToolExecutionResult::error_with_failure(report),
                0,
                "validation_failed_strict".to_string(),
            ));
        }

        // 2. Policy evaluation for validated calls
        for (order, call) in validated_calls {
            let canonical_id = resolve_tool_id(&call.name, tool_name_mappings);
            let tool = self
                .tools
                .get_by_id(&canonical_id)
                .expect("Tool validated but not found in catalog");
            let descriptor = tool.descriptor();

            let risk = tool.risk(&call.input);
            let applicable_grant = self.find_applicable_grant(
                session_grants,
                &canonical_id.to_string(),
                &call.input,
                risk,
                current_turn,
            );

            let policy = self
                .evaluate_policy(
                    session,
                    &call.id,
                    &canonical_id.to_string(),
                    canonical_id.namespace.clone(),
                    descriptor.annotations.clone(),
                    &call.input,
                    risk,
                    applicable_grant.as_ref(),
                    emit,
                    cancel_token,
                )
                .await?;

            match policy.status {
                PolicyStatus::Allowed => {
                    planned.push((order, call.id, canonical_id, call.input, tool, policy));
                }
                PolicyStatus::Denied => {
                    let reason = policy
                        .reason
                        .clone()
                        .unwrap_or_else(|| format!("policy denied tool call {canonical_id}"));
                    let failure = ToolErrorReport::new(ToolFailureKind::PolicyDenied, reason)
                        .with_repair("If the model should retry, ask the user to allow the action or adjust the policy.");
                    denied_results.push((
                        order,
                        call.id,
                        ToolExecutionResult::error_with_failure(failure),
                        0,
                        policy.policy_source.clone(),
                    ));
                }
                PolicyStatus::Confirm => {
                    confirm_queue.push((order, call.id, canonical_id, call.input, tool, policy));
                }
            }
        }

        // 3. Confirm / approval queue
        for (order, id, canonical_id, input, tool, policy) in confirm_queue {
            let call_id = id.clone();
            let original_input_hash = hash_input(&input);

            emit(AgentEvent::ApprovalRequested {
                tool_call_id: id.clone(),
                tool_name: canonical_id.to_string(),
                input: input.clone(),
                risk: tool.risk(&input),
            })?;

            let approval_res = self
                .approval
                .approve_cancellable(
                    ApprovalRequest {
                        tool_call_id: id.clone(),
                        tool_name: canonical_id.to_string(),
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
                    planned.push((order, id, canonical_id, input, tool, policy));
                    (ApprovalOutcome::Approve, None, None)
                }
                ApprovalDecision::Deny => {
                    let reason = policy
                        .reason
                        .clone()
                        .unwrap_or_else(|| format!("approval denied tool call {canonical_id}"));
                    let failure = ToolErrorReport::new(ToolFailureKind::ApprovalDenied, reason)
                        .with_repair(
                        "The user denied the approval. Adjust the request or ask before retrying.",
                    );
                    denied_results.push((
                        order,
                        id,
                        ToolExecutionResult::error_with_failure(failure),
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
                            &canonical_id.to_string(),
                            canonical_id.namespace.clone(),
                            tool.descriptor().annotations.clone(),
                            &new_input,
                            new_risk,
                            None,
                            emit,
                            cancel_token,
                        )
                        .await?;
                    match re_evaluated.status {
                        PolicyStatus::Allowed => {
                            planned.push((order, id, canonical_id, new_input, tool, re_evaluated));
                        }
                        PolicyStatus::Denied => {
                            let reason = re_evaluated.reason.clone().unwrap_or_else(|| {
                                format!("policy denied edited tool call {canonical_id}")
                            });
                            let failure = ToolErrorReport::new(
                                ToolFailureKind::PolicyDenied,
                                reason,
                            )
                            .with_repair(
                                "The edited input is still not allowed. Reconsider the approach.",
                            );
                            denied_results.push((
                                order,
                                id,
                                ToolExecutionResult::error_with_failure(failure),
                                0,
                                re_evaluated.policy_source.clone(),
                            ));
                        }
                        PolicyStatus::Confirm => {
                            let reason = format!(
                                "approval edit still requires confirmation for {canonical_id}"
                            );
                            let failure = ToolErrorReport::new(
                                ToolFailureKind::ApprovalDenied,
                                reason,
                            )
                            .with_repair(
                                "Even after the edit, the call still needs approval. Ask the user.",
                            );
                            denied_results.push((
                                order,
                                id,
                                ToolExecutionResult::error_with_failure(failure),
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
                        canonical_id.to_string(),
                        &input,
                        risk_ceiling,
                        policy.policy_source.clone(),
                        "session_grant",
                        current_turn,
                        loop_max_turns.max(1),
                    );
                    session_grants.push(grant.clone());
                    planned.push((order, id, canonical_id, input, tool, policy));
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
        let (retry_tx, mut retry_rx) = tokio::sync::mpsc::unbounded_channel();

        // 4. Execution of planned calls
        for (order, id, canonical_id, input, tool, policy) in planned {
            let mut tool_ctx = session.tool_ctx.clone();
            tool_ctx.current_tool_call_id = Some(id.clone());
            let mut tool_session = session.clone();
            tool_session.tool_ctx = tool_ctx.clone();

            if tool.can_run_in_parallel(&input) {
                emit_tool_hooks_before(
                    hooks,
                    &tool_session,
                    &canonical_id.to_string(),
                    &input,
                    emit,
                    cancel_token,
                )
                .await?;
                current_parallel.push((order, id, canonical_id, input, tool, policy, tool_ctx));
                continue;
            }

            if !current_parallel.is_empty() {
                for (_, id, canonical_id, input, _, policy, tool_ctx) in &current_parallel {
                    let input_hash = hash_input(input);
                    emit(AgentEvent::ToolExecutionStarted {
                        id: id.clone(),
                        tool_name: canonical_id.to_string(),
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
                let retry_tx_clone = retry_tx.clone();
                let futures = std::mem::take(&mut current_parallel).into_iter().map(
                    |(order, id, _canonical_id, input, tool, policy, tool_ctx)| {
                        let tool_call_id = id.clone();
                        let c = cancel_clone.clone();
                        let r_tx = retry_tx_clone.clone();
                        async move {
                            let descriptor = tool.descriptor();
                            let retry_policy = descriptor.retry_policy.as_ref();
                            let start = std::time::Instant::now();
                            let mut result = execute_tool_with_retry(
                                tool.as_ref(),
                                input,
                                &tool_ctx,
                                &tool_call_id,
                                &c,
                                retry_policy,
                                &descriptor,
                                &r_tx,
                            )
                            .await;
                            tool.shape_output(&mut result);
                            let duration =
                                u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                            (order, id, result, duration, policy.policy_source)
                        }
                    },
                );

                let mut join_all_fut = Box::pin(join_all(futures));
                let parallel_results = loop {
                    tokio::select! {
                        res = &mut join_all_fut => {
                            break res;
                        }
                        Some(event) = retry_rx.recv() => {
                            emit(event)?;
                        }
                        _ = cancel_token.cancelled() => {
                            return Err(crate::error::HarnessError::Cancelled);
                        }
                    }
                };

                while let Ok(event) = retry_rx.try_recv() {
                    emit(event)?;
                }

                for (order, id, result, duration, policy_source) in parallel_results {
                    results.push((order, id, result, duration, policy_source));
                }
            }

            emit_tool_hooks_before(
                hooks,
                &tool_session,
                &canonical_id.to_string(),
                &input,
                emit,
                cancel_token,
            )
            .await?;
            let input_hash = hash_input(&input);
            emit(AgentEvent::ToolExecutionStarted {
                id: id.clone(),
                tool_name: canonical_id.to_string(),
                input_hash,
                policy_source: policy.policy_source.clone(),
                working_dir: tool_ctx.working_dir.display().to_string(),
                parallel_group_id: None,
                parallel_safe: false,
            })?;
            if let Some(s) = sink {
                let _ = s.flush();
            }

            let descriptor = tool.descriptor();
            let retry_policy = descriptor.retry_policy.as_ref();
            let start = std::time::Instant::now();
            let retry_tx_clone = retry_tx.clone();
            let mut result = tokio::select! {
                res = execute_tool_with_retry(tool.as_ref(), input, &tool_ctx, &id, cancel_token, retry_policy, &descriptor, &retry_tx_clone) => res,
                _ = cancel_token.cancelled() => {
                    return Err(crate::error::HarnessError::Cancelled);
                }
            };
            tool.shape_output(&mut result);

            while let Ok(event) = retry_rx.try_recv() {
                emit(event)?;
            }

            let duration = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
            results.push((order, id, result, duration, policy.policy_source));
        }

        if !current_parallel.is_empty() {
            for (_, id, canonical_id, input, _, policy, tool_ctx) in &current_parallel {
                let input_hash = hash_input(input);
                emit(AgentEvent::ToolExecutionStarted {
                    id: id.clone(),
                    tool_name: canonical_id.to_string(),
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
            let retry_tx_clone = retry_tx.clone();
            let futures = current_parallel.into_iter().map(
                |(order, id, _canonical_id, input, tool, policy, tool_ctx)| {
                    let tool_call_id = id.clone();
                    let c = cancel_clone.clone();
                    let r_tx = retry_tx_clone.clone();
                    async move {
                        let descriptor = tool.descriptor();
                        let retry_policy = descriptor.retry_policy.as_ref();
                        let start = std::time::Instant::now();
                        let mut result = execute_tool_with_retry(
                            tool.as_ref(),
                            input,
                            &tool_ctx,
                            &tool_call_id,
                            &c,
                            retry_policy,
                            &descriptor,
                            &r_tx,
                        )
                        .await;
                        tool.shape_output(&mut result);
                        let duration =
                            u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                        (order, id, result, duration, policy.policy_source)
                    }
                },
            );

            let mut join_all_fut = Box::pin(join_all(futures));
            let parallel_results = loop {
                tokio::select! {
                    res = &mut join_all_fut => {
                        break res;
                    }
                    Some(event) = retry_rx.recv() => {
                        emit(event)?;
                    }
                    _ = cancel_token.cancelled() => {
                        return Err(crate::error::HarnessError::Cancelled);
                    }
                }
            };

            while let Ok(event) = retry_rx.try_recv() {
                emit(event)?;
            }

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
        namespace: ToolNamespace,
        annotations: ToolAnnotations,
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
        if (tool_name == "bash" || tool_name == "builtin:bash") && !session.tool_ctx.allow_network {
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
            namespace,
            annotations,
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
        let hook_res = crate::hook::HookDispatcher::dispatch(
            "tool",
            "before_tool_execution",
            cancel_token,
            &mut *emit,
            || hook.before_tool_execution(session, tool_name, input),
        )
        .await;
        match hook_res {
            Ok(events) => {
                for ev in events {
                    emit(ev)?;
                }
            }
            Err(crate::error::HarnessError::Cancelled) => {
                return Err(crate::error::HarnessError::Cancelled);
            }
            Err(err) => {
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
                Ok(Err(err)) => {
                    let failure = match &err {
                        crate::error::ToolError::Timeout { tool_name, timeout_secs } => {
                            let msg = format!(
                                "tool timed out after {}s: {}",
                                timeout_secs, tool_name
                            );
                            ToolErrorReport::new(ToolFailureKind::Timeout, msg)
                                .with_repair("Retry with the same input; trusted read-only tools retry automatically per their retry policy.")
                        }
                        crate::error::ToolError::NotFound(name) => {
                            ToolErrorReport::new(
                                ToolFailureKind::ToolNotFound,
                                format!("tool not found: {}", name),
                            )
                        }
                        crate::error::ToolError::InvalidInput { tool_name, reason } => {
                            ToolErrorReport::new(
                                ToolFailureKind::InvalidArguments,
                                format!("invalid input for {}: {}", tool_name, reason),
                            )
                        }
                        crate::error::ToolError::OutputTooLarge { tool_name, limit } => {
                            ToolErrorReport::new(
                                ToolFailureKind::ExecutionFailed,
                                format!(
                                    "output too large for {}: exceeded {} bytes",
                                    tool_name, limit
                                ),
                            )
                        }
                        crate::error::ToolError::PathNotAllowed(path) => {
                            ToolErrorReport::new(
                                ToolFailureKind::ExecutionFailed,
                                format!("path not allowed: {}", path),
                            )
                        }
                        crate::error::ToolError::NetworkDenied(msg) => {
                            ToolErrorReport::new(
                                ToolFailureKind::ExecutionFailed,
                                format!("network denied: {}", msg),
                            )
                        }
                        crate::error::ToolError::Denied(msg) => {
                            ToolErrorReport::new(ToolFailureKind::PolicyDenied, msg.clone())
                        }
                        crate::error::ToolError::ExecutionFailed(io) => {
                            ToolErrorReport::new(
                                ToolFailureKind::ExecutionFailed,
                                format!("execution failed: {}", io),
                            )
                        }
                    };
                    ToolExecutionResult::error_with_failure(failure)
                }
                Err(_) => {
                    let message = format!(
                        "tool timed out after {}s: {}",
                        ctx.timeout.as_secs(),
                        tool.name()
                    );
                    let failure = ToolErrorReport::new(ToolFailureKind::Timeout, message)
                        .with_repair("Retry with the same input; trusted read-only tools retry automatically per their retry policy.");
                    ToolExecutionResult::error_with_failure(failure)
                }
            }
        }
        _ = cancel_token.cancelled() => {
            let failure = ToolErrorReport::new(
                ToolFailureKind::ExecutionFailed,
                "tool execution interrupted by user".to_string(),
            )
            .with_repair("The user cancelled execution. Wait for further instructions before retrying.");
            ToolExecutionResult::error_with_failure(failure)
        }
    }
}

fn resolve_tool_id(
    provider_name: &str,
    name_mappings: &[crate::tool_name_mapping::ToolNameMapping],
) -> crate::tool_descriptor::CanonicalToolId {
    name_mappings
        .iter()
        .find(|m| m.provider_name == provider_name)
        .map(|m| m.internal_id.clone())
        .expect("resolve_tool_id called for a name not in name_mappings — caller must guard with validation first")
}

async fn execute_tool_with_retry(
    tool: &dyn Tool,
    input: Value,
    ctx: &ToolContext,
    tool_call_id: &str,
    cancel_token: &crate::cancel::CancelToken,
    retry_policy: Option<&ToolRetryPolicy>,
    descriptor: &ToolDescriptor,
    emit_retry_tx: &tokio::sync::mpsc::UnboundedSender<AgentEvent>,
) -> ToolExecutionResult {
    let mut attempt = 0;
    loop {
        let result = execute_tool(tool, input.clone(), ctx, tool_call_id, cancel_token).await;

        if !result.is_error {
            return result;
        }

        // Decide whether this error is retryable. The plan locks
        // this down in U6B: retries are only for transient
        // failures on trusted read-only, idempotent tools, with an
        // explicit retry policy. We classify the failure via
        // `ToolFailureKind::is_transient` so the structured failure
        // plumbing is the single source of truth — no more
        // "any `is_error` is retryable" code path.
        let failure_kind = result
            .failure
            .as_ref()
            .map_or(ToolFailureKind::ExecutionFailed, |f| f.kind);
        let is_transient = failure_kind.is_transient();

        let can_retry = if let Some(policy) = retry_policy {
            let is_idempotent = descriptor.annotations.get_trusted_bool("idempotent");
            let is_read_only = descriptor.annotations.get_trusted_bool("read_only");

            attempt < policy.max_retries && is_idempotent && is_read_only && is_transient
        } else {
            false
        };

        if !can_retry {
            return result;
        }

        attempt += 1;
        let delay_ms = retry_policy.unwrap().backoff_ms;

        // Emit retry event
        let _ = emit_retry_tx.send(AgentEvent::ToolRetryAttempt {
            tool_call_id: tool_call_id.to_string(),
            attempt,
            error: result.content.clone(),
            delay_ms,
        });

        // Wait before retrying
        tokio::select! {
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)) => {}
            _ = cancel_token.cancelled() => {
                let failure = ToolErrorReport::new(
                    ToolFailureKind::ExecutionFailed,
                    "tool execution interrupted by user".to_string(),
                )
                .with_repair("The user cancelled execution. Wait for further instructions before retrying.");
                return ToolExecutionResult::error_with_failure(failure);
            }
        }
    }
}
