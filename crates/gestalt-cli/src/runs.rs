use std::fs;
use std::io::{BufRead, BufReader, IsTerminal, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use gestalt_core::HarnessError;
use crate::config::EffectiveConfig;
use chrono::Utc;

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
            let rfc_str = format!(
                "{}-{}-{}T{}:{}:{}Z",
                &stamp[0..4],
                &stamp[4..6],
                &stamp[6..8],
                &stamp[9..11],
                &stamp[11..13],
                &stamp[13..15]
            );
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&rfc_str) {
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
    let suffix = chars.next_back().ok_or_else(|| "empty duration".to_string())?;
    let val_str = chars.as_str();
    let val: i64 = val_str.parse().map_err(|_| format!("invalid duration number: {}", val_str))?;

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
        } else if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some("trace.jsonl") {
            if let Some(parent) = path.parent() {
                return Ok(parent.to_path_buf());
            }
        }
    }

    let run_log_dir = config.run_log_dir();
    if run_log_dir.exists() {
        let entries = fs::read_dir(&run_log_dir).map_err(|e| {
            HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e))
        })?;
        let mut matches = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| {
                HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e))
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == input || name.starts_with(input) {
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
                }
            ));
        }
    }

    Err(HarnessError::Config(
        gestalt_core::ConfigError::InvalidValue {
            field: "run-id".to_string(),
            reason: format!("run ID or path not found: '{}'", input),
        }
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
        let envelope = match serde_json::from_str::<gestalt_trace::EventEnvelope>(&line) {
            Ok(env) => env,
            Err(_) => continue,
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
            gestalt_core::AgentEvent::ModelRequest { provider: ref p, model: ref m, .. } => {
                if provider.is_none() {
                    provider = Some(p.clone());
                }
                if model.is_none() {
                    model = Some(m.clone());
                }
            }
            gestalt_core::AgentEvent::Stop { reason } => {
                stop_reason = Some(format!("{:?}", reason));
                match reason {
                    gestalt_core::StopReason::EndTurn => {
                        apparent_status = "completed".to_string();
                        break;
                    }
                    gestalt_core::StopReason::PolicyViolation | gestalt_core::StopReason::ProviderError => {
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
            gestalt_core::AgentEvent::Error { recoverable, .. } => {
                if !recoverable {
                    apparent_status = "failed".to_string();
                    break;
                }
            }
            gestalt_core::AgentEvent::Usage { input_tokens, output_tokens } => {
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
}

pub fn summarize_run_dir(path: &Path) -> Result<RunSummary, HarnessError> {
    let run_id = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let start_time = parse_run_timestamp(&run_id);
    let session_id = run_id.split('-').skip(1).collect::<Vec<_>>().join("-");

    let trace_path = path.join("trace.jsonl");
    let summary_path = path.join("summary.md");
    let cost_path = path.join("cost.json");

    let trace_exists = trace_path.exists();
    let summary_exists = summary_path.exists();
    let cost_exists = cost_path.exists();

    let mut provider = None;
    let mut model = None;
    let mut apparent_status = "interrupted".to_string();
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
            apparent_status = meta.apparent_status;
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
    })
}

/// Lists run indices under the run log directory.
pub fn list_runs(config: &EffectiveConfig, limit: Option<usize>) -> Result<crate::output::RunsListReport, HarnessError> {
    let run_log_dir = config.run_log_dir();
    let mut runs = Vec::new();

    if run_log_dir.exists() {
        let entries = fs::read_dir(&run_log_dir).map_err(|e| {
            HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e))
            })?;
            let path = entry.path();
            if path.is_dir() {
                if let Ok(summary) = summarize_run_dir(&path) {
                    runs.push(crate::output::RunIndexEntry {
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

    runs.sort_by(|a, b| {
        match (a.start_time, b.start_time) {
            (Some(ta), Some(tb)) => tb.cmp(&ta).then_with(|| b.run_id.cmp(&a.run_id)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b.run_id.cmp(&a.run_id),
        }
    });

    if let Some(l) = limit {
        runs.truncate(l);
    }

    Ok(crate::output::RunsListReport { runs })
}

/// Inspects a specific run and returns a structured report.
pub fn inspect_run(config: &EffectiveConfig, run_id_or_path: &str) -> Result<crate::output::RunsInspectReport, HarnessError> {
    let resolved_path = resolve_run_path(config, run_id_or_path)?;
    let summary = summarize_run_dir(&resolved_path)?;

    Ok(crate::output::RunsInspectReport {
        run_id: summary.run_id,
        path: summary.path,
        start_time: summary.start_time,
        session_id: summary.session_id,
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

/// Custom line reader that performs byte-by-byte reads.
/// If EOF is reached before a trailing newline `\n`, it seeks back to the start of the line.
/// This prevents reading partial lines when tailing a trace file that is actively being written to.
pub fn read_next_line(file: &mut fs::File, buf: &mut String) -> std::io::Result<usize> {
    buf.clear();
    let mut bytes = Vec::new();
    let mut temp = [0u8; 1];
    let start_pos = file.stream_position()?;

    loop {
        match file.read(&mut temp) {
            Ok(0) => {
                if !bytes.is_empty() {
                    if bytes.last() == Some(&b'\n') {
                        *buf = String::from_utf8(bytes).map_err(|e| {
                            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
                        })?;
                        return Ok(buf.len());
                    } else {
                        file.seek(SeekFrom::Start(start_pos))?;
                        return Ok(0);
                    }
                }
                return Ok(0);
            }
            Ok(1) => {
                let b = temp[0];
                bytes.push(b);
                if b == b'\n' {
                    *buf = String::from_utf8(bytes).map_err(|e| {
                        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
                    })?;
                    return Ok(buf.len());
                }
            }
            Ok(_) => unreachable!(),
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
        }
    }
}

/// Streams new lines appended to the run's trace log in real-time.
pub fn tail_run(config: &EffectiveConfig, run_id_or_path: &str, format: crate::output::OutputFormat) -> Result<(), HarnessError> {
    let resolved_path = resolve_run_path(config, run_id_or_path)?;
    let trace_path = resolved_path.join("trace.jsonl");

    if !trace_path.exists() {
        return Err(HarnessError::Trace(gestalt_core::TraceError::WriteFailed(
            std::io::Error::new(std::io::ErrorKind::NotFound, "trace.jsonl file not found")
        )));
    }

    let mut file = fs::File::open(&trace_path).map_err(|e| {
        HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e))
    })?;
    let mut line = String::new();

    // Read all existing complete lines
    loop {
        match read_next_line(&mut file, &mut line) {
            Ok(0) => break,
            Ok(_) => {
                print_tailed_line(&line, format)?;
            }
            Err(e) => {
                return Err(HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)));
            }
        }
    }

    // Keep tailing for new complete lines
    loop {
        match read_next_line(&mut file, &mut line) {
            Ok(0) => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Ok(_) => {
                print_tailed_line(&line, format)?;
            }
            Err(e) => {
                return Err(HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)));
            }
        }
    }
}

fn print_tailed_line(line: &str, format: crate::output::OutputFormat) -> Result<(), HarnessError> {
    let envelope = serde_json::from_str::<gestalt_trace::EventEnvelope>(line).map_err(|err| {
        HarnessError::Trace(gestalt_core::TraceError::InvalidFormat {
            line: 0,
            reason: err.to_string(),
        })
    })?;

    match format {
        crate::output::OutputFormat::Json => {
            let wrapped = crate::output::JsonEnvelope {
                schema_version: 1,
                kind: "runs.tail.event".to_string(),
                data: envelope,
            };
            println!("{}", serde_json::to_string(&wrapped).unwrap_or_default());
        }
        crate::output::OutputFormat::Text => {
            if let Some(rendered) = crate::output::render_event(&envelope.event) {
                println!("{rendered}");
            }
        }
    }
    Ok(())
}

/// Prunes old runs matching the specified age duration criteria.
pub fn prune_runs(
    config: &EffectiveConfig,
    older_than: Option<String>,
    dry_run: bool,
    skip_confirm: bool,
) -> Result<crate::output::RunsPruneReport, HarnessError> {
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
        let entries = fs::read_dir(&run_log_dir).map_err(|e| {
            HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e))
            })?;
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
                    let size = get_dir_size(&path).unwrap_or(0);
                    runs_to_prune.push((run_id, path, size));
                }
            }
        }
    }

    if runs_to_prune.is_empty() {
        return Ok(crate::output::RunsPruneReport {
            pruned_runs: Vec::new(),
            reclaimed_bytes: 0,
            dry_run,
        });
    }

    if !dry_run && !skip_confirm {
        if std::io::stdin().is_terminal() {
            let size_mb = runs_to_prune.iter().map(|(_, _, s)| s).sum::<u64>() as f64 / 1_048_576.0;
            println!("Are you sure you want to prune {} runs (reclaiming {:.2} MB)? [y/N]", runs_to_prune.len(), size_mb);
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).is_ok() {
                let trimmed = input.trim().to_lowercase();
                if trimmed != "y" && trimmed != "yes" {
                    println!("Prune cancelled.");
                    return Ok(crate::output::RunsPruneReport {
                        pruned_runs: Vec::new(),
                        reclaimed_bytes: 0,
                        dry_run,
                    });
                }
            } else {
                return Err(HarnessError::Approval(gestalt_core::ApprovalError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "failed to read confirmation"))));
            }
        } else {
            return Err(HarnessError::Approval(gestalt_core::ApprovalError::Rejected(
                "non-interactive execution requires interactive terminal or explicit confirmation bypass flag (--yes)".to_string()
            )));
        }
    }

    let mut pruned_runs = Vec::new();
    for (run_id, path, size) in runs_to_prune {
        if !dry_run {
            if !is_descendant(&run_log_dir, &path) {
                return Err(HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
                    field: "run-log-dir".to_string(),
                    reason: format!(
                        "run path '{}' is not within the run log directory '{}'",
                        path.display(),
                        run_log_dir.display()
                    ),
                }));
            }
            fs::remove_dir_all(&path).map_err(|e| {
                HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e))
            })?;
        }
        total_reclaimed_bytes += size;
        pruned_runs.push(run_id);
    }

    Ok(crate::output::RunsPruneReport {
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
) -> Result<crate::output::RunsDeleteReport, HarnessError> {
    let resolved_path = resolve_run_path(config, run_id_or_path)?;
    let run_id = resolved_path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let run_log_dir = config.run_log_dir();

    if !is_descendant(&run_log_dir, &resolved_path) {
        return Err(HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
            field: "run-id".to_string(),
            reason: format!(
                "resolved path '{}' is not within the run log directory '{}'",
                resolved_path.display(),
                run_log_dir.display()
            ),
        }));
    }

    let size = get_dir_size(&resolved_path).unwrap_or(0);

    if !skip_confirm {
        if std::io::stdin().is_terminal() {
            let size_mb = size as f64 / 1_048_576.0;
            println!("Are you sure you want to delete run {} (reclaiming {:.2} MB)? [y/N]", run_id, size_mb);
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).is_ok() {
                let trimmed = input.trim().to_lowercase();
                if trimmed != "y" && trimmed != "yes" {
                    println!("Delete cancelled.");
                    return Err(HarnessError::Approval(gestalt_core::ApprovalError::Rejected("cancelled by user".to_string())));
                }
            } else {
                return Err(HarnessError::Approval(gestalt_core::ApprovalError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "failed to read confirmation"))));
            }
        } else {
            return Err(HarnessError::Approval(gestalt_core::ApprovalError::Rejected(
                "non-interactive execution requires interactive terminal or explicit confirmation bypass flag (--yes)".to_string()
            )));
        }
    }

    fs::remove_dir_all(&resolved_path).map_err(|e| {
        HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e))
    })?;

    Ok(crate::output::RunsDeleteReport {
        deleted_run: run_id,
        reclaimed_bytes: size,
    })
}
