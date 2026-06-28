use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};

use gestalt_core::event::PolicyStatus;
use gestalt_core::AgentEvent;
use gestalt_trace::{aggregate_costs, analyze_tool_metrics, read_trace};

use crate::output::{
    PolicyOutcomesSummary, ReplayReport, TraceAnalyzeReport, TraceInspectReport,
    TraceValidateReport,
};
use crate::replay::replay_display;
use crate::runs;
use gestalt_app::config::EffectiveConfig;

/// Replays the trace events in a human-readable transcript.
pub fn replay_trace(
    config: &EffectiveConfig,
    run_id_or_path: &str,
) -> Result<ReplayReport, Box<dyn std::error::Error>> {
    let (_run_id, _run_dir, trace_path) = resolve_trace_target(config, run_id_or_path)?;
    let rendered = replay_display(&trace_path)?;
    Ok(ReplayReport { rendered })
}

pub fn resolve_trace_target(
    config: &EffectiveConfig,
    run_id_or_path: &str,
) -> Result<(String, std::path::PathBuf, std::path::PathBuf), Box<dyn std::error::Error>> {
    let path = std::path::Path::new(run_id_or_path);
    if path.exists() {
        if path.is_dir() {
            let run_id = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let trace_path = path.join("trace.jsonl");
            Ok((run_id, path.to_path_buf(), trace_path))
        } else {
            let run_dir = path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf();
            let run_id = run_dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            Ok((run_id, run_dir, path.to_path_buf()))
        }
    } else {
        let run_dir = runs::resolve_run_path(config, run_id_or_path)?;
        let run_id = run_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let trace_path = run_dir.join("trace.jsonl");
        Ok((run_id, run_dir, trace_path))
    }
}

/// Inspects trace events and aggregates execution statistics.
pub fn inspect_trace(
    config: &EffectiveConfig,
    run_id_or_path: &str,
) -> Result<TraceInspectReport, Box<dyn std::error::Error>> {
    let (run_id, _run_dir, trace_path) = resolve_trace_target(config, run_id_or_path)?;

    if !trace_path.exists() {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("trace.jsonl does not exist at {}", trace_path.display()),
        )));
    }

    let envelopes = read_trace(&trace_path)?;

    let mut event_types = HashMap::new();
    let mut turns = 0;
    let mut tool_calls = 0;
    let mut policy_decisions = 0;
    let mut policy_outcomes = PolicyOutcomesSummary::default();
    let mut verification_results = 0;
    let mut verification_status = None;
    let mut artifacts = Vec::new();
    let mut total_input_tokens = 0;
    let mut total_output_tokens = 0;
    let mut prompt_snapshots_created = 0;
    let mut prompt_snapshots_loaded = 0;
    let mut prompt_snapshots_reused = 0;
    let mut prompt_cache_plans = 0;
    let mut ephemeral_context_injections = 0;
    let mut redacted = false;
    let mut warnings = Vec::new();

    for env in &envelopes {
        let variant_name = match &env.event {
            AgentEvent::RunStarted { .. } => "run_started",
            AgentEvent::UserMessage { .. } => "user_message",
            AgentEvent::ContextBuilt { .. } => "context_built",
            AgentEvent::PromptSnapshotCreated { .. } => "prompt_snapshot_created",
            AgentEvent::PromptSnapshotLoaded { .. } => "prompt_snapshot_loaded",
            AgentEvent::PromptSnapshotReused { .. } => "prompt_snapshot_reused",
            AgentEvent::PromptCachePlanGenerated { .. } => "prompt_cache_plan_generated",
            AgentEvent::EphemeralContextInjected { .. } => "ephemeral_context_injected",
            AgentEvent::ModelRequest { .. } => "model_request",
            AgentEvent::Text { .. } => "text",
            AgentEvent::Thinking { .. } => "thinking",
            AgentEvent::ToolCallStreamed { .. } => "tool_call_streamed",
            AgentEvent::ToolCallProposed { .. } => "tool_call_proposed",
            AgentEvent::PolicyDecision { .. } => "policy_decision",
            AgentEvent::ApprovalDecision { .. } => "approval_decision",
            AgentEvent::ToolResult { .. } => "tool_result",
            AgentEvent::ArtifactCreated { .. } => "artifact_created",
            AgentEvent::PolicyViolation { .. } => "policy_violation",
            AgentEvent::MemoryProposal { .. } => "memory_proposal",
            AgentEvent::VerificationResult { .. } => "verification_result",
            AgentEvent::Usage { .. } => "usage",
            AgentEvent::Stop { .. } => "stop",
            AgentEvent::WorkspaceSnapshotCaptured { .. } => "workspace_snapshot_captured",
            AgentEvent::Checkpoint { .. } => "checkpoint",
            AgentEvent::AssistantMessageCommitted { .. } => "assistant_message_committed",
            AgentEvent::Interrupted { .. } => "interrupted",
            AgentEvent::ContextBuildStarted => "context_build_started",
            AgentEvent::ContextBuildFailed { .. } => "context_build_failed",
            AgentEvent::ModelResponseStarted { .. } => "model_response_started",
            AgentEvent::ModelResponseStreamCompleted { .. } => "model_response_stream_completed",
            AgentEvent::ModelResponseStreamFailed { .. } => "model_response_stream_failed",
            AgentEvent::ModelResponseStreamInterrupted { .. } => {
                "model_response_stream_interrupted"
            }
            AgentEvent::PolicyEvaluationStarted { .. } => "policy_evaluation_started",
            AgentEvent::PolicyEvaluationFailed { .. } => "policy_evaluation_failed",
            AgentEvent::PolicyEvaluationCancelled { .. } => "policy_evaluation_cancelled",
            AgentEvent::ApprovalRequested { .. } => "approval_requested",
            AgentEvent::ApprovalCancelled { .. } => "approval_cancelled",
            AgentEvent::ToolExecutionStarted { .. } => "tool_execution_started",
            AgentEvent::HookStarted { .. } => "hook_started",
            AgentEvent::HookCompleted { .. } => "hook_completed",
            AgentEvent::HookFailed { .. } => "hook_failed",
            AgentEvent::Error { .. } => "error",
            AgentEvent::ToolCatalogSelected { .. } => "tool_catalog_selected",
            AgentEvent::ToolCallValidationFailed { .. } => "tool_call_validation_failed",
            AgentEvent::ToolRetryAttempt { .. } => "tool_retry_attempt",
            AgentEvent::NextTurnOverrideRequested { .. } => "next_turn_override_requested",
            AgentEvent::NextTurnBlocked { .. } => "next_turn_blocked",
            AgentEvent::SessionMessageInjected { .. } => "session_message_injected",
            AgentEvent::SessionMessageQueueDrained { .. } => "session_message_queue_drained",
            AgentEvent::ContextContributorResolved { .. } => "context_contributor_resolved",
            AgentEvent::WorkspaceContextLoaded { .. } => "workspace_context_loaded",
            AgentEvent::WorkspaceContextSkipped { .. } => "workspace_context_skipped",
            AgentEvent::WorkspaceContextRejected { .. } => "workspace_context_rejected",
            AgentEvent::WorkspaceContextLoadFailed { .. } => "workspace_context_load_failed",
            AgentEvent::MemoryContextLoadFailed { .. } => "memory_context_load_failed",
            AgentEvent::MemoryContextLoaded { .. } => "memory_context_loaded",
            AgentEvent::MemoryContextSkipped { .. } => "memory_context_skipped",
            AgentEvent::MemoryContextRejected { .. } => "memory_context_rejected",
            AgentEvent::MemoryEntriesSelected { .. } => "memory_entries_selected",
            AgentEvent::ContextSnapshotCreated { .. } => "context_snapshot_created",
            AgentEvent::MemoryProposalCreated { .. } => "memory_proposal_created",
            AgentEvent::MemoryProposalDecisionRecorded { .. } => {
                "memory_proposal_decision_recorded"
            }
            AgentEvent::MemoryWriteSucceeded { .. } => "memory_write_succeeded",
            AgentEvent::MemoryWriteConflict { .. } => "memory_write_conflict",
            AgentEvent::MemoryWriteFailed { .. } => "memory_write_failed",
            AgentEvent::ContextPressure { .. } => "context_pressure",
            AgentEvent::ContextClearing { .. } => "context_clearing",
            AgentEvent::ContextCompactionStarted { .. } => "context_compaction_started",
            AgentEvent::ContextCompacted { .. } => "context_compacted",
            AgentEvent::ContextManagementFailed { .. } => "context_management_failed",
            AgentEvent::ContextExhaustion { .. } => "context_exhaustion",
        };

        *event_types.entry(variant_name.to_string()).or_insert(0) += 1;

        if env.turn_id > turns {
            turns = env.turn_id;
        }

        if env.redacted {
            redacted = true;
        }

        match &env.event {
            AgentEvent::ToolCallProposed { .. } => {
                tool_calls += 1;
            }
            AgentEvent::PolicyDecision { decision, .. } => {
                policy_decisions += 1;
                match decision {
                    PolicyStatus::Allowed => policy_outcomes.allowed += 1,
                    PolicyStatus::Confirm => policy_outcomes.confirmed += 1,
                    PolicyStatus::Denied => policy_outcomes.denied += 1,
                }
            }
            AgentEvent::VerificationResult { status, .. } => {
                verification_results += 1;
                verification_status = Some(*status);
            }
            AgentEvent::ArtifactCreated { path, .. } => {
                artifacts.push(path.clone());
            }
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                total_input_tokens += input_tokens;
                total_output_tokens += output_tokens;
            }
            AgentEvent::PromptSnapshotCreated { .. } => prompt_snapshots_created += 1,
            AgentEvent::PromptSnapshotLoaded { .. } => prompt_snapshots_loaded += 1,
            AgentEvent::PromptSnapshotReused { .. } => prompt_snapshots_reused += 1,
            AgentEvent::PromptCachePlanGenerated { .. } => prompt_cache_plans += 1,
            AgentEvent::EphemeralContextInjected { .. } => ephemeral_context_injections += 1,
            AgentEvent::Error { message, .. } => {
                warnings.push(format!("Error event in trace: {message}"));
            }
            _ => {}
        }
    }

    // Cost calculation
    let resolver =
        |model_id: &str| gestalt_models::ModelCatalog::built_in().get_qualified(model_id);
    let cost_rep = aggregate_costs(&trace_path, resolver).ok();
    let estimated_cost_usd = cost_rep.and_then(|c| c.estimated_cost_usd);

    Ok(TraceInspectReport {
        run_id,
        path: trace_path,
        total_events: envelopes.len(),
        event_types,
        turns,
        tool_calls,
        policy_decisions,
        policy_outcomes,
        verification_results,
        verification_status,
        artifacts,
        total_input_tokens,
        total_output_tokens,
        estimated_cost_usd,
        prompt_snapshots_created,
        prompt_snapshots_loaded,
        prompt_snapshots_reused,
        prompt_cache_plans,
        ephemeral_context_injections,
        redacted,
        warnings,
    })
}

/// Validates trace envelope schemas, monotonic sequence ordering, and artifact presence.
pub fn validate_trace(
    config: &EffectiveConfig,
    run_id_or_path: &str,
) -> Result<TraceValidateReport, Box<dyn std::error::Error>> {
    let (run_id, run_dir, trace_path) = resolve_trace_target(config, run_id_or_path)?;

    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut valid = true;

    if !trace_path.exists() {
        errors.push(format!(
            "trace.jsonl file does not exist at {}",
            trace_path.display()
        ));
        return Ok(TraceValidateReport {
            run_id,
            path: trace_path,
            valid: false,
            errors,
            warnings,
        });
    }

    let file = match fs::File::open(&trace_path) {
        Ok(f) => f,
        Err(e) => {
            errors.push(format!("failed to open trace file: {e}"));
            return Ok(TraceValidateReport {
                run_id,
                path: trace_path,
                valid: false,
                errors,
                warnings,
            });
        }
    };

    let reader = BufReader::new(file);
    let mut prev_seq = None;

    for (index, line) in reader.lines().enumerate() {
        let line_num = index + 1;
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                valid = false;
                errors.push(format!("Line {line_num}: read error: {e}"));
                break;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let envelope = match serde_json::from_str::<gestalt_trace::EventEnvelope>(&line) {
            Ok(env) => env,
            Err(e) => {
                valid = false;
                errors.push(format!("Line {line_num}: invalid JSON: {e}"));
                continue;
            }
        };

        if envelope.v != 1 {
            valid = false;
            errors.push(format!(
                "Line {line_num}: invalid schema version (expected 1, got {})",
                envelope.v
            ));
        }

        if let Some(prev) = prev_seq {
            if envelope.seq <= prev {
                valid = false;
                errors.push(format!(
                    "Line {line_num}: sequence number regression (expected > {prev}, got {})",
                    envelope.seq
                ));
            }
        }
        prev_seq = Some(envelope.seq);

        match &envelope.event {
            AgentEvent::ArtifactCreated { path, .. } => {
                let p = run_dir.join(path);
                if !p.exists() {
                    warnings.push(format!(
                        "Line {line_num}: referenced artifact does not exist at {path}"
                    ));
                }
            }
            AgentEvent::ToolResult { artifact_refs, .. } => {
                if let Some(refs) = artifact_refs {
                    for path in refs {
                        let p = run_dir.join(path);
                        if !p.exists() {
                            warnings.push(format!("Line {line_num}: referenced tool result artifact does not exist at {path}"));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if !errors.is_empty() {
        valid = false;
    }

    Ok(TraceValidateReport {
        run_id,
        path: trace_path,
        valid,
        errors,
        warnings,
    })
}

/// Analyze trace events for tool-calling reliability metrics. The
/// path may point to a run directory, a single `trace.jsonl`, or a
/// directory of fixture traces. The `kind` selector is forward
/// compatible: today only `tools` is implemented, but the wrapper
/// makes it easy to add `--kind cost` or similar without rewriting
/// the CLI wiring.
pub fn analyze_trace(
    config: &EffectiveConfig,
    run_id_or_path: &str,
    kind: &str,
) -> Result<TraceAnalyzeReport, Box<dyn std::error::Error>> {
    let (_run_id, _run_dir, trace_path) = resolve_trace_target(config, run_id_or_path)?;
    let resolver =
        |model_id: &str| gestalt_models::ModelCatalog::built_in().get_qualified(model_id);
    match kind {
        "tools" => {
            let tools_metrics = analyze_tool_metrics(&trace_path, resolver)?;
            Ok(TraceAnalyzeReport {
                path: trace_path,
                tools_metrics,
            })
        }
        other => Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Unknown analyze kind '{other}'. Supported kinds: tools. (Note: the legacy --tools flag is the default and may be omitted.)"
            ),
        ))),
    }
}
