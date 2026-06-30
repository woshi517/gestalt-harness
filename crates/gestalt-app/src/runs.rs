use crate::config::EffectiveConfig;
use crate::reports::{
    RunIndexEntry, RunsDeleteReport, RunsInspectReport, RunsListReport, RunsPruneReport,
};
use chrono::Utc;
use gestalt_core::HarnessError;
use gestalt_runtime::TraceEvent as AgentEvent;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Detailed cost and token usage report for runs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CostReport {
    /// Total number of runs aggregated.
    pub runs: usize,
    /// Total input tokens consumed.
    pub input_tokens: usize,
    /// Total output tokens consumed.
    pub output_tokens: usize,
    /// Total estimated cost in USD, if available.
    pub estimated_cost_usd: Option<f64>,
    /// Warnings generated during cost calculation.
    pub warnings: Vec<String>,
}

/// Extracted metadata from a trace file.
pub struct ScannedTrace {
    /// The provider used (e.g., openai, anthropic).
    pub provider: Option<String>,
    /// The model name used.
    pub model: Option<String>,
    /// The apparent run status derived from events.
    pub apparent_status: String,
    /// Total turns executed.
    pub turns: usize,
    /// Stop reason description, if stopped.
    pub stop_reason: Option<String>,
    /// Accumulated input tokens from usage events.
    pub total_input_tokens: Option<usize>,
    /// Accumulated output tokens from usage events.
    pub total_output_tokens: Option<usize>,
    /// Workspace snapshot identifier.
    pub workspace_snapshot_id: Option<String>,
}

/// Helper function to check if `child` is a subdirectory/descendant of `parent`.
/// Prevents directory traversal attacks in destructive operations.
fn is_descendant(parent: &Path, child: &Path) -> bool {
    let parent_canonical = match parent.canonicalize() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let child_canonical = match child.canonicalize() {
        Ok(p) => p,
        Err(_) => return false,
    };
    child_canonical.starts_with(&parent_canonical) && child_canonical != parent_canonical
}

/// Parses the timestamp from a standard Run ID string (e.g. "20260602T100000Z-session").
pub fn parse_run_timestamp(run_id: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if !run_id.is_ascii() {
        return None;
    }
    if run_id.len() >= 17 && run_id.as_bytes()[16] == b'-' {
        let stamp = &run_id[..16];
        if stamp.len() == 16 && stamp.as_bytes()[8] == b'T' && stamp.as_bytes()[15] == b'Z' {
            let formatted_rfc = format!(
                "{}-{}-{}T{}:{}:{}Z",
                &stamp[0..4],
                &stamp[4..6],
                &stamp[6..8],
                &stamp[9..11],
                &stamp[11..13],
                &stamp[13..15]
            );
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&formatted_rfc) {
                return Some(dt.with_timezone(&chrono::Utc));
            }
        }
    }
    None
}

/// Parses a duration string (e.g. "7d", "24h", "60s") into a `chrono::Duration`.
pub fn parse_duration(s: &str) -> Result<chrono::Duration, String> {
    if s.is_empty() {
        return Err("empty duration".to_string());
    }
    let mut chars = s.chars();
    let suffix = chars
        .next_back()
        .ok_or_else(|| "empty duration".to_string())?;
    let val_str = chars.as_str();
    let val: i64 = val_str
        .parse()
        .map_err(|_| format!("invalid duration number: {}", val_str))?;

    if val <= 0 {
        return Err(format!("duration must be positive, got: {}", val));
    }

    match suffix {
        'd' => Ok(chrono::Duration::days(val)),
        'h' => Ok(chrono::Duration::hours(val)),
        'm' => Ok(chrono::Duration::minutes(val)),
        's' => Ok(chrono::Duration::seconds(val)),
        _ => Err(format!("unknown duration suffix: {}", suffix)),
    }
}

/// Recursively calculates the total size of a file or directory in bytes.
pub fn get_dir_size(path: &Path) -> std::io::Result<u64> {
    let mut total_size = 0;
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                if !file_type.is_symlink() {
                    total_size += get_dir_size(&p)?;
                }
            } else {
                total_size += entry.metadata()?.len();
            }
        }
    } else {
        total_size = path.metadata()?.len();
    }
    Ok(total_size)
}

/// Resolves a run path from an ID, directory name, or unique prefix.
/// If `input` points to a file, its parent directory is returned only if it's named `trace.jsonl`.
pub fn resolve_run_path(config: &EffectiveConfig, input: &str) -> Result<PathBuf, HarnessError> {
    let path = PathBuf::from(input);
    if path.exists() {
        if path.is_dir() {
            if path.join("trace.jsonl").exists() {
                return Ok(path);
            }
        } else if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some("trace.jsonl")
        {
            if let Some(parent) = path.parent() {
                return Ok(parent.to_path_buf());
            }
        }
    }

    let run_log_dir = config.run_log_dir();
    if run_log_dir.exists() {
        let entries = fs::read_dir(&run_log_dir)
            .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
        let mut matches = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
            let name = entry.file_name().to_string_lossy().into_owned();

            let mut is_match = false;
            if name == input || name.starts_with(input) {
                is_match = true;
            }
            if !is_match {
                if let Some(pos) = name.find('-') {
                    let suffix = &name[pos + 1..];
                    if suffix == input || suffix.starts_with(input) {
                        is_match = true;
                    }
                }
            }
            if !is_match {
                let manifest_path = entry.path().join("run.json");
                if manifest_path.exists() {
                    if let Ok(manifest) =
                        gestalt_runtime::run_manifest::RunManifest::load_from(&manifest_path)
                    {
                        if manifest.run_id == input || manifest.run_id.starts_with(input) {
                            is_match = true;
                        }
                    }
                }
            }

            if is_match {
                matches.push(entry.path());
            }
        }

        if matches.len() == 1 {
            return Ok(matches[0].clone());
        } else if matches.len() > 1 {
            return Err(HarnessError::Config(
                gestalt_core::ConfigError::InvalidValue {
                    field: "run-id".to_string(),
                    reason: format!("ambiguous run ID: '{}' matched multiple runs", input),
                },
            ));
        }
    }

    Err(HarnessError::Config(
        gestalt_core::ConfigError::InvalidValue {
            field: "run-id".to_string(),
            reason: format!("run ID or path not found: '{}'", input),
        },
    ))
}

/// Scans a trace file to extract model, provider, status, turns, and usage metrics.
/// Optimized to stop reading once a terminal Stop event or Error event is encountered.
pub fn scan_trace_file(trace_path: &Path) -> Result<ScannedTrace, gestalt_core::TraceError> {
    let file = fs::File::open(trace_path).map_err(|e| gestalt_core::TraceError::ReadFailed {
        reason: e.to_string(),
    })?;
    let reader = BufReader::new(file);

    let mut provider = None;
    let mut model = None;
    let mut apparent_status = "interrupted".to_string();
    let mut turns = 0;
    let mut stop_reason = None;
    let mut total_input_tokens = Some(0);
    let mut total_output_tokens = Some(0);
    let mut workspace_snapshot_id = None;

    for line in reader.lines() {
        let line = line.map_err(|e| gestalt_core::TraceError::ReadFailed {
            reason: e.to_string(),
        })?;
        let Some(envelope) = gestalt_runtime::parse_trace_envelope_line(&line, 0).map_err(
            |err| gestalt_core::TraceError::ReadFailed {
                reason: err.to_string(),
            },
        )? else {
            continue;
        };

        if envelope.turn_id > turns {
            turns = envelope.turn_id;
        }
        if let Some(ref snapshot) = envelope.workspace_snapshot {
            workspace_snapshot_id = Some(snapshot.content_hash.clone());
        } else if let Some(ref snapshot_id) = envelope.snapshot_id {
            workspace_snapshot_id = Some(snapshot_id.clone());
        }

        match envelope.event {
            AgentEvent::ModelRequest {
                provider: ref p,
                model: ref m,
                ..
            } => {
                if provider.is_none() {
                    provider = Some(p.clone());
                }
                if model.is_none() {
                    model = Some(m.clone());
                }
            }
            AgentEvent::Stop { reason } => {
                stop_reason = Some(format!("{:?}", reason));
                match reason {
                    gestalt_core::StopReason::EndTurn => {
                        apparent_status = "completed".to_string();
                        break;
                    }
                    gestalt_core::StopReason::PolicyViolation
                    | gestalt_core::StopReason::ProviderError => {
                        apparent_status = "failed".to_string();
                        break;
                    }
                    _ => {
                        if reason != gestalt_core::StopReason::ToolUse {
                            break;
                        }
                    }
                }
            }
            AgentEvent::Error { recoverable, .. } => {
                if !recoverable {
                    apparent_status = "failed".to_string();
                    break;
                }
            }
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                if let Some(i) = total_input_tokens {
                    total_input_tokens = Some(i + input_tokens);
                }
                if let Some(o) = total_output_tokens {
                    total_output_tokens = Some(o + output_tokens);
                }
            }
            _ => {}
        }
    }

    Ok(ScannedTrace {
        provider,
        model,
        apparent_status,
        turns,
        stop_reason,
        total_input_tokens,
        total_output_tokens,
        workspace_snapshot_id,
    })
}

pub struct RunSummary {
    pub run_id: String,
    pub path: PathBuf,
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub session_id: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub trace_exists: bool,
    pub summary_exists: bool,
    pub cost_exists: bool,
    pub apparent_status: String,
    pub turns: Option<usize>,
    pub stop_reason: Option<String>,
    pub total_input_tokens: Option<usize>,
    pub total_output_tokens: Option<usize>,
    pub estimated_cost_usd: Option<f64>,
    pub workspace_snapshot_id: Option<String>,
    pub artifacts: Vec<String>,
    pub parent_run_id: Option<String>,
    pub run_kind: Option<String>,
    pub lifecycle_state: Option<String>,
}

pub fn summarize_run_dir(path: &Path) -> Result<RunSummary, HarnessError> {
    let run_manifest_path = path.join("run.json");
    let manifest = if run_manifest_path.exists() {
        gestalt_runtime::run_manifest::RunManifest::load_from(&run_manifest_path).ok()
    } else {
        None
    };

    let folder_name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let start_time = parse_run_timestamp(&folder_name);
    let run_id = if let Some(ref m) = manifest {
        m.run_id.clone()
    } else {
        folder_name.clone()
    };
    let session_id = if let Some(ref m) = manifest {
        m.session_id.clone()
    } else {
        folder_name.split('-').skip(1).collect::<Vec<_>>().join("-")
    };

    let trace_path = path.join("trace.jsonl");
    let summary_path = path.join("summary.md");
    let cost_path = path.join("cost.json");

    let trace_exists = trace_path.exists();
    let summary_exists = summary_path.exists();
    let cost_exists = cost_path.exists();

    let mut provider = None;
    let mut model = None;
    let mut apparent_status = if let Some(ref m) = manifest {
        format!("{:?}", m.lifecycle_state).to_lowercase()
    } else {
        "interrupted".to_string()
    };
    let mut turns = None;
    let mut stop_reason = None;
    let mut total_input_tokens = Some(0);
    let mut total_output_tokens = Some(0);
    let mut estimated_cost_usd = None;
    let mut workspace_snapshot_id = None;

    if let Ok(content) = fs::read_to_string(&cost_path) {
        if let Ok(cost_rep) = serde_json::from_str::<CostReport>(&content) {
            total_input_tokens = Some(cost_rep.input_tokens);
            total_output_tokens = Some(cost_rep.output_tokens);
            estimated_cost_usd = cost_rep.estimated_cost_usd;
        }
    }

    if trace_exists {
        if let Ok(meta) = scan_trace_file(&trace_path) {
            provider = meta.provider;
            model = meta.model;
            if manifest.is_none() {
                apparent_status = meta.apparent_status;
            }
            turns = Some(meta.turns);
            stop_reason = meta.stop_reason;
            if total_input_tokens == Some(0) || total_input_tokens.is_none() {
                total_input_tokens = meta.total_input_tokens;
            }
            if total_output_tokens == Some(0) || total_output_tokens.is_none() {
                total_output_tokens = meta.total_output_tokens;
            }
            workspace_snapshot_id = meta.workspace_snapshot_id;
        }
    }

    let artifacts_dir = path.join("artifacts");
    let mut artifacts = Vec::new();
    if let Ok(entries) = fs::read_dir(artifacts_dir) {
        for entry in entries.flatten() {
            artifacts.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    artifacts.sort();

    let parent_run_id = manifest.as_ref().and_then(|m| m.parent_run_id.clone());
    let run_kind = manifest
        .as_ref()
        .map(|m| format!("{:?}", m.run_kind).to_lowercase());
    let lifecycle_state = manifest
        .as_ref()
        .map(|m| format!("{:?}", m.lifecycle_state).to_lowercase());

    Ok(RunSummary {
        run_id,
        path: path.to_path_buf(),
        start_time,
        session_id,
        provider,
        model,
        trace_exists,
        summary_exists,
        cost_exists,
        apparent_status,
        turns,
        stop_reason,
        total_input_tokens,
        total_output_tokens,
        estimated_cost_usd,
        workspace_snapshot_id,
        artifacts,
        parent_run_id,
        run_kind,
        lifecycle_state,
    })
}

/// Lists run indices under the run log directory.
pub fn list_runs(
    config: &EffectiveConfig,
    limit: Option<usize>,
) -> Result<RunsListReport, HarnessError> {
    let run_log_dir = config.run_log_dir();
    let mut runs = Vec::new();

    if run_log_dir.exists() {
        let entries = fs::read_dir(&run_log_dir)
            .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;

        for entry in entries {
            let entry =
                entry.map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
            let path = entry.path();
            if path.is_dir() {
                if let Ok(summary) = summarize_run_dir(&path) {
                    runs.push(RunIndexEntry {
                        run_id: summary.run_id,
                        path: summary.path,
                        start_time: summary.start_time,
                        session_id: summary.session_id,
                        provider: summary.provider,
                        model: summary.model,
                        trace_exists: summary.trace_exists,
                        summary_exists: summary.summary_exists,
                        cost_exists: summary.cost_exists,
                        artifact_count: summary.artifacts.len(),
                        apparent_status: summary.apparent_status,
                        total_input_tokens: summary.total_input_tokens,
                        total_output_tokens: summary.total_output_tokens,
                        estimated_cost_usd: summary.estimated_cost_usd,
                    });
                }
            }
        }
    }

    runs.sort_by(|a, b| match (a.start_time, b.start_time) {
        (Some(ta), Some(tb)) => tb.cmp(&ta).then_with(|| b.run_id.cmp(&a.run_id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.run_id.cmp(&a.run_id),
    });

    if let Some(l) = limit {
        runs.truncate(l);
    }

    Ok(RunsListReport { runs })
}

/// Inspects a specific run and returns a structured report.
pub fn inspect_run(
    config: &EffectiveConfig,
    run_id_or_path: &str,
) -> Result<RunsInspectReport, HarnessError> {
    let resolved_path = resolve_run_path(config, run_id_or_path)?;
    let summary = summarize_run_dir(&resolved_path)?;

    Ok(RunsInspectReport {
        run_id: summary.run_id,
        path: summary.path,
        start_time: summary.start_time,
        session_id: summary.session_id,
        parent_run_id: summary.parent_run_id,
        run_kind: summary.run_kind,
        lifecycle_state: summary.lifecycle_state,
        provider: summary.provider,
        model: summary.model,
        trace_exists: summary.trace_exists,
        summary_exists: summary.summary_exists,
        cost_exists: summary.cost_exists,
        apparent_status: summary.apparent_status,
        turns: summary.turns,
        stop_reason: summary.stop_reason,
        total_input_tokens: summary.total_input_tokens,
        total_output_tokens: summary.total_output_tokens,
        estimated_cost_usd: summary.estimated_cost_usd,
        workspace_snapshot_id: summary.workspace_snapshot_id,
        artifacts: summary.artifacts,
    })
}

fn has_descendants(config: &EffectiveConfig, run_id: &str) -> bool {
    let run_log_dir = config.run_log_dir();
    if !run_log_dir.exists() {
        return false;
    }
    if let Ok(entries) = fs::read_dir(&run_log_dir) {
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("run.json");
            if manifest_path.exists() {
                if let Ok(manifest) =
                    gestalt_runtime::run_manifest::RunManifest::load_from(&manifest_path)
                {
                    if let Some(ref parent_id) = manifest.parent_run_id {
                        if parent_id == run_id {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn gather_descendants(
    config: &EffectiveConfig,
    run_id: &str,
    out: &mut Vec<(String, PathBuf, u64)>,
) {
    let run_log_dir = config.run_log_dir();
    if !run_log_dir.exists() {
        return;
    }
    if let Ok(entries) = fs::read_dir(&run_log_dir) {
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("run.json");
            if manifest_path.exists() {
                if let Ok(manifest) =
                    gestalt_runtime::run_manifest::RunManifest::load_from(&manifest_path)
                {
                    if let Some(ref parent_id) = manifest.parent_run_id {
                        if parent_id == run_id {
                            let child_run_id = manifest.run_id.clone();
                            let child_path = entry.path();
                            let child_size = get_dir_size(&child_path).unwrap_or(0);
                            let folder_name = child_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned();
                            let actual_child_id = if !child_run_id.is_empty() {
                                child_run_id.clone()
                            } else {
                                folder_name.clone()
                            };
                            if !out.iter().any(|(id, _, _)| id == &actual_child_id) {
                                out.push((actual_child_id.clone(), child_path, child_size));
                                gather_descendants(config, &child_run_id, out);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Prunes old runs matching the specified age duration criteria.
pub fn prune_runs(
    config: &EffectiveConfig,
    older_than: Option<String>,
    dry_run: bool,
    skip_confirm: bool,
    cascade: bool,
    interaction: Option<&dyn crate::InteractionProvider>,
) -> Result<RunsPruneReport, HarnessError> {
    let duration_str = older_than.unwrap_or_else(|| "7d".to_string());
    let duration = parse_duration(&duration_str).map_err(|reason| {
        HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
            field: "older-than".to_string(),
            reason,
        })
    })?;

    let run_log_dir = config.run_log_dir();
    let mut runs_to_prune = Vec::new();
    let mut total_reclaimed_bytes = 0;

    let now = Utc::now();
    let threshold = now - duration;

    if run_log_dir.exists() {
        let entries = fs::read_dir(&run_log_dir)
            .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;

        for entry in entries {
            let entry =
                entry.map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
            let path = entry.path();
            if path.is_dir() {
                let run_id = entry.file_name().to_string_lossy().into_owned();
                let start_time = parse_run_timestamp(&run_id).unwrap_or_else(|| {
                    if let Ok(metadata) = entry.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            chrono::DateTime::<chrono::Utc>::from(modified)
                        } else {
                            now
                        }
                    } else {
                        now
                    }
                });

                if start_time < threshold {
                    let run_manifest_path = path.join("run.json");
                    let mut r_id = None;
                    if run_manifest_path.exists() {
                        if let Ok(m) = gestalt_runtime::run_manifest::RunManifest::load_from(
                            &run_manifest_path,
                        ) {
                            r_id = Some(m.run_id);
                        }
                    }

                    let folder_name = entry.file_name().to_string_lossy().into_owned();
                    let actual_run_id = r_id.clone().unwrap_or_else(|| folder_name.clone());

                    if let Some(ref rid) = r_id {
                        if !cascade && has_descendants(config, rid) {
                            return Err(HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
                                field: "older-than".to_string(),
                                reason: format!("Cannot prune run '{}' because it has descendant runs. Use --cascade to prune it and all descendants.", actual_run_id),
                            }));
                        }
                    }

                    let size = get_dir_size(&path).unwrap_or(0);
                    if !runs_to_prune.iter().any(|(_, p, _)| p == &path) {
                        runs_to_prune.push((actual_run_id.clone(), path.clone(), size));
                    }

                    if cascade {
                        if let Some(ref rid) = r_id {
                            let mut descendants = Vec::new();
                            gather_descendants(config, rid, &mut descendants);
                            for (desc_id, desc_path, desc_size) in descendants {
                                if !runs_to_prune.iter().any(|(_, p, _)| p == &desc_path) {
                                    runs_to_prune.push((desc_id, desc_path, desc_size));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if runs_to_prune.is_empty() {
        return Ok(RunsPruneReport {
            pruned_runs: Vec::new(),
            reclaimed_bytes: 0,
            dry_run,
        });
    }

    if !dry_run && !skip_confirm {
        let confirmed = if let Some(i) = interaction {
            let size_mb = runs_to_prune.iter().map(|(_, _, s)| s).sum::<u64>() as f64 / 1_048_576.0;
            i.confirm(&format!(
                "Are you sure you want to prune {} runs (reclaiming {:.2} MB)?",
                runs_to_prune.len(),
                size_mb
            ))
        } else {
            false
        };
        if !confirmed {
            return Err(HarnessError::Approval(gestalt_core::ApprovalError::Rejected(
                "cancelled by user or non-interactive execution requires explicit confirmation bypass flag (--yes)".to_string()
            )));
        }
    }

    let mut pruned_runs = Vec::new();
    for (run_id, path, size) in runs_to_prune {
        if !dry_run {
            if !is_descendant(&run_log_dir, &path) {
                return Err(HarnessError::Config(
                    gestalt_core::ConfigError::InvalidValue {
                        field: "run-log-dir".to_string(),
                        reason: format!(
                            "run path '{}' is not within the run log directory '{}'",
                            path.display(),
                            run_log_dir.display()
                        ),
                    },
                ));
            }
            fs::remove_dir_all(&path)
                .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
        }
        total_reclaimed_bytes += size;
        pruned_runs.push(run_id);
    }

    Ok(RunsPruneReport {
        pruned_runs,
        reclaimed_bytes: total_reclaimed_bytes,
        dry_run,
    })
}

/// Deletes a specific run folder and returns a report of the space reclaimed.
pub fn delete_run(
    config: &EffectiveConfig,
    run_id_or_path: &str,
    skip_confirm: bool,
    cascade: bool,
    interaction: Option<&dyn crate::InteractionProvider>,
) -> Result<RunsDeleteReport, HarnessError> {
    let resolved_path = resolve_run_path(config, run_id_or_path)?;
    let run_log_dir = config.run_log_dir();

    if !is_descendant(&run_log_dir, &resolved_path) {
        return Err(HarnessError::Config(
            gestalt_core::ConfigError::InvalidValue {
                field: "run-id".to_string(),
                reason: format!(
                    "resolved path '{}' is not within the run log directory '{}'",
                    resolved_path.display(),
                    run_log_dir.display()
                ),
            },
        ));
    }

    let run_manifest_path = resolved_path.join("run.json");
    let mut target_run_id = None;
    if run_manifest_path.exists() {
        if let Ok(m) = gestalt_runtime::run_manifest::RunManifest::load_from(&run_manifest_path) {
            target_run_id = Some(m.run_id);
        }
    }

    let folder_name = resolved_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let actual_run_id = target_run_id.clone().unwrap_or_else(|| folder_name.clone());

    if let Some(ref r_id) = target_run_id {
        if !cascade && has_descendants(config, r_id) {
            return Err(HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
                field: "run-id".to_string(),
                reason: format!("Cannot delete run '{}' because it has descendant runs. Use --cascade to delete it and all descendants.", actual_run_id),
            }));
        }
    }

    let mut runs_to_delete = vec![(
        actual_run_id.clone(),
        resolved_path.clone(),
        get_dir_size(&resolved_path).unwrap_or(0),
    )];
    if cascade {
        if let Some(ref r_id) = target_run_id {
            let mut descendants = Vec::new();
            gather_descendants(config, r_id, &mut descendants);
            for (desc_id, desc_path, desc_size) in descendants {
                if !runs_to_delete.iter().any(|(_, p, _)| p == &desc_path) {
                    runs_to_delete.push((desc_id, desc_path, desc_size));
                }
            }
        }
    }

    let total_size: u64 = runs_to_delete.iter().map(|(_, _, s)| s).sum();

    if !skip_confirm {
        let confirmed = if let Some(i) = interaction {
            let size_mb = total_size as f64 / 1_048_576.0;
            let prompt = if runs_to_delete.len() > 1 {
                format!("Are you sure you want to delete run {} and its {} descendants (reclaiming {:.2} MB)?", actual_run_id, runs_to_delete.len() - 1, size_mb)
            } else {
                format!(
                    "Are you sure you want to delete run {} (reclaiming {:.2} MB)?",
                    actual_run_id, size_mb
                )
            };
            i.confirm(&prompt)
        } else {
            false
        };
        if !confirmed {
            return Err(HarnessError::Approval(gestalt_core::ApprovalError::Rejected(
                "cancelled by user or non-interactive execution requires explicit confirmation bypass flag (--yes)".to_string()
            )));
        }
    }

    for (_, path, _) in &runs_to_delete {
        if !is_descendant(&run_log_dir, path) {
            return Err(HarnessError::Config(
                gestalt_core::ConfigError::InvalidValue {
                    field: "run-id".to_string(),
                    reason: format!(
                        "run path '{}' is not within the run log directory '{}'",
                        path.display(),
                        run_log_dir.display()
                    ),
                },
            ));
        }
        fs::remove_dir_all(path)
            .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
    }

    Ok(RunsDeleteReport {
        deleted_run: actual_run_id,
        reclaimed_bytes: total_size,
    })
}
