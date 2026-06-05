use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};

use gestalt_core::event::PolicyStatus;
use gestalt_core::AgentEvent;
use gestalt_trace::{aggregate_costs, read_trace};

use crate::config::EffectiveConfig;
use crate::output::{PolicyOutcomesSummary, ReplayReport, TraceInspectReport, TraceValidateReport};
use crate::replay::replay_display;
use crate::runs;

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
    let mut redacted = false;
    let mut warnings = Vec::new();

    for env in &envelopes {
        let variant_name = match &env.event {
            AgentEvent::UserMessage { .. } => "user_message",
            AgentEvent::ContextBuilt { .. } => "context_built",
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
