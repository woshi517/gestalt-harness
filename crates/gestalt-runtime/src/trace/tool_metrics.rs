use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::read_trace;
use crate::TraceEvent as AgentEvent;
use gestalt_core::{event::PolicyStatus, model::ModelInfo, TraceError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolMetricsReport {
    pub total_proposed_calls: usize,
    pub total_validation_failures: usize,
    pub invalid_tool_call_rate: f64,

    pub total_policy_decisions: usize,
    pub total_policy_denials: usize,
    pub policy_denied_rate: f64,

    pub total_tool_results: usize,
    pub total_truncated_results: usize,
    pub truncation_rate: f64,

    pub total_executed_calls: usize,
    pub first_call_success_count: usize,
    pub first_call_success_rate: f64,

    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub estimated_cost_usd: Option<f64>,

    pub total_exposure_count: usize,
    pub total_turns_with_tool_selection: usize,
    pub tool_exposure_count_per_turn: f64,
}

fn token_cost(tokens: usize, rate_per_million: f64) -> f64 {
    usize_to_f64(tokens) * rate_per_million / 1_000_000.0
}

fn collect_trace_paths_flexible(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }

    let mut traces = Vec::new();

    // Check if the directory itself has expected.jsonl or trace.jsonl
    let direct_trace = path.join("trace.jsonl");
    if direct_trace.exists() {
        traces.push(direct_trace);
    }
    let direct_expected = path.join("expected.jsonl");
    if direct_expected.exists() {
        traces.push(direct_expected);
    }

    // Now recursively/sub-directory search for expected.jsonl or trace.jsonl
    fn walk_dir(dir: &Path, traces: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                let t = p.join("trace.jsonl");
                if t.exists() {
                    traces.push(t);
                }
                let e = p.join("expected.jsonl");
                if e.exists() {
                    traces.push(e);
                }
                let _ = walk_dir(&p, traces);
            }
        }
        Ok(())
    }
    let _ = walk_dir(path, &mut traces);

    traces.sort();
    traces.dedup();
    traces
}

fn usize_to_f64(value: usize) -> f64 {
    let narrowed = u32::try_from(value).unwrap_or(u32::MAX);
    f64::from(narrowed)
}

pub fn analyze_tool_metrics(
    path: impl AsRef<Path>,
    resolver: impl Fn(&str) -> Option<ModelInfo>,
) -> Result<ToolMetricsReport, TraceError> {
    let trace_paths = collect_trace_paths_flexible(path.as_ref());

    let mut report = ToolMetricsReport::default();
    let mut pricing_failed = false;
    let mut total_cost = 0.0;

    for trace_path in trace_paths {
        let events = read_trace(&trace_path)?;
        let mut current_model = None::<String>;
        let mut retry_counts = HashMap::new();
        let mut results = HashMap::new();

        // Pass 1: collect retry attempts
        for envelope in &events {
            if let AgentEvent::ToolRetryAttempt { tool_call_id, .. } = &envelope.event {
                *retry_counts.entry(tool_call_id.clone()).or_insert(0) += 1;
            }
        }

        // Pass 2: calculate metrics from events
        for envelope in events {
            match envelope.event {
                AgentEvent::ModelRequest {
                    provider, model, ..
                } => {
                    current_model = Some(format!("{provider}/{model}"));
                }
                AgentEvent::ToolCallProposed { .. } => {
                    report.total_proposed_calls = report.total_proposed_calls.saturating_add(1);
                }
                AgentEvent::ToolCallValidationFailed { .. } => {
                    report.total_validation_failures =
                        report.total_validation_failures.saturating_add(1);
                }
                AgentEvent::PolicyDecision { decision, .. } => {
                    report.total_policy_decisions = report.total_policy_decisions.saturating_add(1);
                    if decision == PolicyStatus::Denied {
                        report.total_policy_denials = report.total_policy_denials.saturating_add(1);
                    }
                }
                AgentEvent::ToolResult {
                    id,
                    is_error,
                    truncated,
                    failure,
                    ..
                } => {
                    report.total_tool_results = report.total_tool_results.saturating_add(1);
                    if truncated {
                        report.total_truncated_results =
                            report.total_truncated_results.saturating_add(1);
                    }
                    let is_pre = failure.as_ref().is_some_and(|f| f.kind.is_pre_execution());
                    if !is_pre {
                        results.insert(id, is_error);
                    }
                }
                AgentEvent::ToolCatalogSelected { tools } => {
                    report.total_turns_with_tool_selection =
                        report.total_turns_with_tool_selection.saturating_add(1);
                    report.total_exposure_count =
                        report.total_exposure_count.saturating_add(tools.len());
                }
                AgentEvent::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    report.total_input_tokens =
                        report.total_input_tokens.saturating_add(input_tokens);
                    report.total_output_tokens =
                        report.total_output_tokens.saturating_add(output_tokens);

                    if !pricing_failed {
                        if let Some(model_id) = current_model.as_deref() {
                            if let Some(info) = resolver(model_id) {
                                if let (Some(input), Some(output)) =
                                    (info.input_cost_per_million, info.output_cost_per_million)
                                {
                                    total_cost += token_cost(input_tokens, input)
                                        + token_cost(output_tokens, output);
                                } else {
                                    pricing_failed = true;
                                }
                            } else {
                                pricing_failed = true;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Check first-call success
        for (id, is_error) in results {
            report.total_executed_calls = report.total_executed_calls.saturating_add(1);
            if !is_error {
                let retries = retry_counts.get(&id).copied().unwrap_or(0);
                if retries == 0 {
                    report.first_call_success_count =
                        report.first_call_success_count.saturating_add(1);
                }
            }
        }
    }

    // Rates calculation
    if report.total_proposed_calls > 0 {
        report.invalid_tool_call_rate = usize_to_f64(report.total_validation_failures)
            / usize_to_f64(report.total_proposed_calls);
    }
    if report.total_policy_decisions > 0 {
        report.policy_denied_rate =
            usize_to_f64(report.total_policy_denials) / usize_to_f64(report.total_policy_decisions);
    }
    if report.total_tool_results > 0 {
        report.truncation_rate =
            usize_to_f64(report.total_truncated_results) / usize_to_f64(report.total_tool_results);
    }
    if report.total_executed_calls > 0 {
        report.first_call_success_rate = usize_to_f64(report.first_call_success_count)
            / usize_to_f64(report.total_executed_calls);
    }
    if report.total_turns_with_tool_selection > 0 {
        report.tool_exposure_count_per_turn = usize_to_f64(report.total_exposure_count)
            / usize_to_f64(report.total_turns_with_tool_selection);
    }

    if !pricing_failed {
        report.estimated_cost_usd = Some(total_cost);
    }

    Ok(report)
}
