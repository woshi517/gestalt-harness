//! `gestalt-trace` — JSONL trace writer + `EventEnvelope`

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use chrono::{DateTime, Utc};
use gestalt_core::{
    model::ModelInfo, trace::TraceSink, AgentEvent, RunResult, StopReason, TraceError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub v: u32,
    pub session_id: String,
    pub turn_id: usize,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub event: AgentEvent,
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPaths {
    pub root: PathBuf,
    pub trace: PathBuf,
    pub summary: PathBuf,
    pub cost: PathBuf,
    pub artifacts: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostReport {
    pub runs: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub estimated_cost_usd: Option<f64>,
    pub warnings: Vec<String>,
}

impl CostReport {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            runs: 0,
            input_tokens: 0,
            output_tokens: 0,
            estimated_cost_usd: Some(0.0),
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct JsonlTraceSink {
    session_id: String,
    state: Mutex<TraceState>,
}

#[derive(Debug)]
struct TraceState {
    writer: BufWriter<File>,
    seq: u64,
    turn_id: usize,
}

impl JsonlTraceSink {
    pub fn new(
        session_id: impl Into<String>,
        trace_path: impl AsRef<Path>,
    ) -> Result<Self, TraceError> {
        let trace_path = trace_path.as_ref();
        if let Some(parent) = trace_path.parent() {
            fs::create_dir_all(parent).map_err(TraceError::WriteFailed)?;
        }

        let file = File::create(trace_path).map_err(TraceError::WriteFailed)?;
        Ok(Self {
            session_id: session_id.into(),
            state: Mutex::new(TraceState {
                writer: BufWriter::new(file),
                seq: 0,
                turn_id: 0,
            }),
        })
    }

    pub fn create_run(
        base_dir: impl AsRef<Path>,
        session_id: &str,
    ) -> Result<(Self, RunPaths), TraceError> {
        let paths = create_run_paths(base_dir, session_id)?;
        let sink = Self::new(session_id.to_string(), &paths.trace)?;
        Ok((sink, paths))
    }
}

impl TraceSink for JsonlTraceSink {
    fn emit(&self, event: AgentEvent) -> Result<(), TraceError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| TraceError::WriteFailed(std::io::Error::other("trace sink poisoned")))?;

        if matches!(event, AgentEvent::ContextBuilt { .. }) {
            state.turn_id = state.turn_id.saturating_add(1);
        }

        state.seq = state.seq.saturating_add(1);
        let (event, redacted) = redact_event(&event);
        let envelope = EventEnvelope {
            v: 1,
            session_id: self.session_id.clone(),
            turn_id: state.turn_id,
            seq: state.seq,
            ts: Utc::now(),
            event,
            redacted,
        };

        serde_json::to_writer(&mut state.writer, &envelope)
            .map_err(|err| TraceError::WriteFailed(std::io::Error::other(err)))?;
        state
            .writer
            .write_all(b"\n")
            .map_err(TraceError::WriteFailed)?;
        drop(state);
        Ok(())
    }

    fn flush(&self) -> Result<(), TraceError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| TraceError::WriteFailed(std::io::Error::other("trace sink poisoned")))?;
        state.writer.flush().map_err(TraceError::WriteFailed)
    }
}

pub fn create_run_paths(
    base_dir: impl AsRef<Path>,
    session_id: &str,
) -> Result<RunPaths, TraceError> {
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let root = base_dir.as_ref().join(format!("{stamp}-{session_id}"));
    let artifacts = root.join("artifacts");
    fs::create_dir_all(&artifacts).map_err(TraceError::WriteFailed)?;

    let trace = root.join("trace.jsonl");
    let summary = root.join("summary.md");
    let cost = root.join("cost.json");
    File::create(&summary).map_err(TraceError::WriteFailed)?;
    File::create(&cost).map_err(TraceError::WriteFailed)?;

    Ok(RunPaths {
        root,
        trace,
        summary,
        cost,
        artifacts,
    })
}

pub fn read_trace(path: impl AsRef<Path>) -> Result<Vec<EventEnvelope>, TraceError> {
    let file = File::open(path).map_err(TraceError::WriteFailed)?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(TraceError::WriteFailed)?;
        let envelope = serde_json::from_str::<EventEnvelope>(&line).map_err(|err| {
            TraceError::InvalidFormat {
                line: index + 1,
                reason: err.to_string(),
            }
        })?;
        events.push(envelope);
    }

    Ok(events)
}

pub fn render_display(events: &[EventEnvelope]) -> String {
    let mut lines = Vec::new();

    for envelope in events {
        match &envelope.event {
            AgentEvent::UserMessage { content } => lines.push(format!("user> {content}")),
            AgentEvent::ContextBuilt {
                packet_id,
                token_estimate,
                packet_hash,
                ..
            } => {
                let mut extra = String::new();
                if let Some(h) = packet_hash {
                    extra.push_str(&format!(" packet_hash={}", &h[..8.min(h.len())]));
                }
                lines.push(format!(
                    "context> {packet_id} ({token_estimate} tokens){extra}"
                ));
            }
            AgentEvent::ModelRequest {
                provider,
                model,
                packet_hash,
                temperature,
                max_tokens,
                provider_request_hash,
                ..
            } => {
                let mut extra = String::new();
                if let Some(h) = packet_hash {
                    extra.push_str(&format!(" packet_hash={}", &h[..8.min(h.len())]));
                }
                if let Some(t) = temperature {
                    extra.push_str(&format!(" temp={}", t));
                }
                if let Some(m) = max_tokens {
                    extra.push_str(&format!(" max_tokens={}", m));
                }
                if let Some(h) = provider_request_hash {
                    extra.push_str(&format!(" request_hash={}", &h[..8.min(h.len())]));
                }
                lines.push(format!("model> {provider}/{model}{extra}"));
            }
            AgentEvent::Text { delta } => lines.push(format!("assistant> {delta}")),
            AgentEvent::Thinking { delta } => lines.push(format!("thinking> {delta}")),
            AgentEvent::ToolCallStreamed { .. } => {}
            AgentEvent::ToolCallProposed { id, name, input } => {
                lines.push(format!("tool> {name}#{id} {input}"));
            }
            AgentEvent::PolicyDecision {
                tool_call_id,
                tool_name,
                input_hash,
                risk,
                mode,
                matched_rule,
                decision,
                reason,
                policy_source,
            } => {
                let mut extra = String::new();
                if let Some(name) = tool_name {
                    extra.push_str(&format!(" tool={name}"));
                }
                if let Some(level) = risk {
                    extra.push_str(&format!(" risk={level:?}"));
                }
                if let Some(m) = mode {
                    extra.push_str(&format!(" mode={m:?}"));
                }
                if let Some(rule) = matched_rule {
                    extra.push_str(&format!(" rule={rule}"));
                }
                if let Some(hash) = input_hash {
                    extra.push_str(&format!(" input={}", &hash[..8.min(hash.len())]));
                }
                lines.push(format!(
                    "policy> {tool_call_id} {decision:?} source={policy_source}{extra} {}",
                    reason.clone().unwrap_or_default()
                ));
            }
            AgentEvent::ApprovalDecision {
                tool_call_id,
                decision,
                original_input_hash,
                edited_input_hash,
                grant_terms,
            } => {
                let grant = grant_terms
                    .as_ref()
                    .map(|g| {
                        format!(
                            " grant={}#{}",
                            g.tool_name,
                            &g.input_hash[..8.min(g.input_hash.len())]
                        )
                    })
                    .unwrap_or_default();
                let edited = edited_input_hash
                    .as_ref()
                    .map(|h| format!(" edited={}", &h[..8.min(h.len())]))
                    .unwrap_or_default();
                lines.push(format!(
                    "approval> {tool_call_id} {decision:?} orig={}{}{}",
                    &original_input_hash[..8.min(original_input_hash.len())],
                    edited,
                    grant
                ));
            }
            AgentEvent::ToolResult {
                id,
                output,
                is_error,
                truncated,
                tool_name,
                working_dir,
                duration_ms,
                output_hash,
                artifact_refs,
                policy_source,
            } => {
                let mut extra = String::new();
                if let Some(name) = tool_name {
                    extra.push_str(&format!(" name={}", name));
                }
                if let Some(dir) = working_dir {
                    extra.push_str(&format!(" dir={}", dir));
                }
                if let Some(ms) = duration_ms {
                    extra.push_str(&format!(" duration={}ms", ms));
                }
                if let Some(h) = output_hash {
                    extra.push_str(&format!(" hash={}", &h[..8.min(h.len())]));
                }
                if let Some(refs) = artifact_refs {
                    if !refs.is_empty() {
                        extra.push_str(&format!(" artifacts={}", refs.join(",")));
                    }
                }
                if let Some(src) = policy_source {
                    extra.push_str(&format!(" policy_source={}", src));
                }
                lines.push(format!(
                    "tool-result> {id} error={is_error} truncated={truncated}{extra} {output}"
                ));
            }
            AgentEvent::ArtifactCreated {
                path,
                size_bytes,
                mime_type,
                hash,
            } => lines.push(format!(
                "artifact-created> {path} size={size_bytes} mime={mime_type} hash={}",
                &hash[..8.min(hash.len())]
            )),
            AgentEvent::PolicyViolation {
                tool_call_id,
                tool_name,
                reason,
            } => lines.push(format!(
                "policy-violation> {tool_call_id} tool={tool_name} reason={reason}"
            )),
            AgentEvent::MemoryProposal { diff } => lines.push(format!("memory> {diff}")),
            AgentEvent::VerificationResult {
                status,
                checks,
                failed,
                report,
            } => lines.push(format!(
                "verification> {status:?} checks={checks} failed={failed} {}",
                report.clone().unwrap_or_default()
            )),
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
            } => lines.push(format!("usage> in={input_tokens} out={output_tokens}")),
            AgentEvent::Stop { reason } => lines.push(format!("stop> {reason:?}")),
            AgentEvent::Error {
                message,
                recoverable,
            } => lines.push(format!("error> recoverable={recoverable} {message}")),
        }
    }

    lines.join("\n")
}

pub fn aggregate_costs(
    path: impl AsRef<Path>,
    resolver: impl Fn(&str) -> Option<ModelInfo>,
) -> Result<CostReport, TraceError> {
    let trace_paths = collect_trace_paths(path.as_ref())?;
    let mut report = CostReport::empty();
    report.runs = trace_paths.len();

    for trace_path in trace_paths {
        let events = read_trace(trace_path)?;
        let mut current_model = None::<String>;
        for envelope in events {
            match envelope.event {
                AgentEvent::ModelRequest {
                    provider, model, ..
                } => {
                    current_model = Some(format!("{provider}/{model}"));
                }
                AgentEvent::Usage {
                    input_tokens,
                    output_tokens,
                } => {
                    report.input_tokens = report.input_tokens.saturating_add(input_tokens);
                    report.output_tokens = report.output_tokens.saturating_add(output_tokens);

                    if let Some(model_id) = current_model.as_deref() {
                        if let Some(info) = resolver(model_id) {
                            if let (Some(input), Some(output)) =
                                (info.input_cost_per_million, info.output_cost_per_million)
                            {
                                let delta = token_cost(input_tokens, input)
                                    + token_cost(output_tokens, output);
                                report.estimated_cost_usd =
                                    Some(report.estimated_cost_usd.unwrap_or(0.0) + delta);
                            } else {
                                report.estimated_cost_usd = None;
                                report
                                    .warnings
                                    .push(format!("missing pricing metadata for {model_id}"));
                            }
                        } else {
                            report.estimated_cost_usd = None;
                            report
                                .warnings
                                .push(format!("unknown model pricing for {model_id}"));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(report)
}

pub fn write_summary(path: impl AsRef<Path>, result: &RunResult) -> Result<(), TraceError> {
    let summary = format!(
        "# Run Summary\n\n- Session: {}\n- Turns: {}\n- Stop reason: {:?}\n- Input tokens: {}\n- Output tokens: {}\n- Artifacts: {}\n",
        result.session_id,
        result.turns,
        result.stop_reason,
        result.total_input_tokens,
        result.total_output_tokens,
        result.artifacts.len(),
    );
    fs::write(path, summary).map_err(TraceError::WriteFailed)
}

pub fn write_cost_report(path: impl AsRef<Path>, report: &CostReport) -> Result<(), TraceError> {
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|err| TraceError::WriteFailed(std::io::Error::other(err)))?;
    fs::write(path, bytes).map_err(TraceError::WriteFailed)
}

fn collect_trace_paths(path: &Path) -> Result<Vec<PathBuf>, TraceError> {
    if path.is_file() {
        return Ok(vec![path.to_path_buf()]);
    }

    let direct_trace = path.join("trace.jsonl");
    if direct_trace.exists() {
        return Ok(vec![direct_trace]);
    }

    let mut traces = Vec::new();
    for entry in fs::read_dir(path).map_err(TraceError::WriteFailed)? {
        let entry = entry.map_err(TraceError::WriteFailed)?;
        let candidate = entry.path().join("trace.jsonl");
        if candidate.exists() {
            traces.push(candidate);
        }
    }
    traces.sort();
    Ok(traces)
}

fn redact_event(event: &AgentEvent) -> (AgentEvent, bool) {
    match event {
        AgentEvent::UserMessage { content } => {
            let (content, redacted) = redact_string(content);
            (AgentEvent::UserMessage { content }, redacted)
        }
        AgentEvent::Text { delta } => {
            let (delta, redacted) = redact_string(delta);
            (AgentEvent::Text { delta }, redacted)
        }
        AgentEvent::Thinking { delta } => {
            let (delta, redacted) = redact_string(delta);
            (AgentEvent::Thinking { delta }, redacted)
        }
        AgentEvent::ToolCallStreamed {
            id,
            name,
            input_delta,
        } => {
            let (input_delta, redacted) = redact_string(input_delta);
            (
                AgentEvent::ToolCallStreamed {
                    id: id.clone(),
                    name: name.clone(),
                    input_delta,
                },
                redacted,
            )
        }
        AgentEvent::ToolCallProposed { id, name, input } => {
            let (input, redacted) = redact_value(input);
            (
                AgentEvent::ToolCallProposed {
                    id: id.clone(),
                    name: name.clone(),
                    input,
                },
                redacted,
            )
        }
        AgentEvent::PolicyDecision {
            tool_call_id,
            tool_name,
            input_hash,
            risk,
            mode,
            matched_rule,
            decision,
            reason,
            policy_source,
        } => {
            let (reason, redacted) = reason.as_ref().map_or((None, false), |reason| {
                let (reason, redacted) = redact_string(reason);
                (Some(reason), redacted)
            });
            (
                AgentEvent::PolicyDecision {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    input_hash: input_hash.clone(),
                    risk: *risk,
                    mode: *mode,
                    matched_rule: matched_rule.clone(),
                    decision: *decision,
                    reason,
                    policy_source: policy_source.clone(),
                },
                redacted,
            )
        }
        AgentEvent::ToolResult {
            id,
            output,
            is_error,
            truncated,
            tool_name,
            working_dir,
            duration_ms,
            output_hash,
            artifact_refs,
            policy_source,
        } => {
            let (output, redacted) = redact_string(output);
            (
                AgentEvent::ToolResult {
                    id: id.clone(),
                    output,
                    is_error: *is_error,
                    truncated: *truncated,
                    tool_name: tool_name.clone(),
                    working_dir: working_dir.clone(),
                    duration_ms: *duration_ms,
                    output_hash: output_hash.clone(),
                    artifact_refs: artifact_refs.clone(),
                    policy_source: policy_source.clone(),
                },
                redacted,
            )
        }
        AgentEvent::ArtifactCreated {
            path,
            size_bytes,
            mime_type,
            hash,
        } => (
            AgentEvent::ArtifactCreated {
                path: path.clone(),
                size_bytes: *size_bytes,
                mime_type: mime_type.clone(),
                hash: hash.clone(),
            },
            false,
        ),
        AgentEvent::PolicyViolation {
            tool_call_id,
            tool_name,
            reason,
        } => {
            let (reason, redacted) = redact_string(reason);
            (
                AgentEvent::PolicyViolation {
                    tool_call_id: tool_call_id.clone(),
                    tool_name: tool_name.clone(),
                    reason,
                },
                redacted,
            )
        }
        AgentEvent::MemoryProposal { diff } => {
            let (diff, redacted) = redact_string(diff);
            (AgentEvent::MemoryProposal { diff }, redacted)
        }
        AgentEvent::VerificationResult {
            status,
            checks,
            failed,
            report,
        } => {
            let (report, redacted) = report.as_ref().map_or((None, false), |report| {
                let (report, redacted) = redact_string(report);
                (Some(report), redacted)
            });
            (
                AgentEvent::VerificationResult {
                    status: *status,
                    checks: *checks,
                    failed: *failed,
                    report,
                },
                redacted,
            )
        }
        AgentEvent::Error {
            message,
            recoverable,
        } => {
            let (message, redacted) = redact_string(message);
            (
                AgentEvent::Error {
                    message,
                    recoverable: *recoverable,
                },
                redacted,
            )
        }
        other => (other.clone(), false),
    }
}

fn redact_value(value: &Value) -> (Value, bool) {
    match value {
        Value::String(text) => {
            let (text, redacted) = redact_string(text);
            (Value::String(text), redacted)
        }
        Value::Array(items) => {
            let mut any = false;
            let items = items
                .iter()
                .map(|item| {
                    let (item, redacted) = redact_value(item);
                    any |= redacted;
                    item
                })
                .collect();
            (Value::Array(items), any)
        }
        Value::Object(map) => {
            let mut any = false;
            let map = map
                .iter()
                .map(|(key, value)| {
                    let (value, redacted) = redact_value(value);
                    any |= redacted;
                    (key.clone(), value)
                })
                .collect();
            (Value::Object(map), any)
        }
        other => (other.clone(), false),
    }
}

fn redact_string(input: &str) -> (String, bool) {
    let mut changed = false;
    let redacted = input
        .split_whitespace()
        .map(|token| {
            if looks_like_secret(token) {
                changed = true;
                "[REDACTED]".to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    (redacted, changed)
}

fn looks_like_secret(token: &str) -> bool {
    let trimmed = token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | ',' | ';' | ')' | '('));
    let lowered = trimmed.to_ascii_lowercase();
    trimmed.starts_with("sk-")
        || trimmed.starts_with("sk_ant_")
        || trimmed.starts_with("sk-ant-")
        || is_jwt_like(trimmed)
        || (trimmed.contains("://") && trimmed.contains('@'))
        || lowered.contains("api_key=")
}

fn is_jwt_like(token: &str) -> bool {
    let parts = token.split('.').collect::<Vec<_>>();
    parts.len() == 3 && parts.iter().all(|part| part.len() >= 8)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "approximate token pricing is reported in USD"
)]
fn token_cost(tokens: usize, rate_per_million: f64) -> f64 {
    (tokens as f64 / 1_000_000.0) * rate_per_million
}

#[must_use]
pub fn stop_reason_label(reason: StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::ToolUse => "tool_use",
        StopReason::MaxOutput => "max_output",
        StopReason::ContentFiltered => "content_filtered",
        StopReason::MaxTurns => "max_turns",
        StopReason::BudgetExhausted => "budget_exhausted",
        StopReason::PolicyViolation => "policy_violation",
        StopReason::ProviderError => "provider_error",
    }
}
