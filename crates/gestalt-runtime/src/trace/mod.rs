//! JSONL trace writer + `EventEnvelope` implementation for `gestalt-runtime`.
//!
pub mod context_artifacts;
pub mod evaluator;
pub mod event;
pub mod fixture;
pub mod golden;
pub mod resume;
pub mod run_manifest;
pub mod tool_metrics;

pub use context_artifacts::{
    load_checkpoint, load_manifest, persist_checkpoint, persist_manifest, CompactionCheckpoint,
    MessageMetadataRef, ProjectionManifest,
};
pub use evaluator::{EvalResult, EvalStatus, EvaluatorHook, NoopTraceEvaluator, TraceEvaluator};
pub use event::{is_known_kind, TraceEventV1};
pub use fixture::{FixtureInput, MockToolConfig, TraceFixture};
pub use golden::{GoldenTrace, GoldenTraceRunner};
pub use resume::{RecoveryStatus, ResumeAnalysis, ResumeAnalyzer};
pub use run_manifest::{CompatibilityFingerprint, LifecycleState, RunKind, RunManifest};
pub use tool_metrics::{analyze_tool_metrics, ToolMetricsReport};

type AgentEvent = TraceEventV1;

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use chrono::{DateTime, Utc};
use gestalt_core::{
    event::AgentEvent as CoreAgentEvent, model::ModelInfo, trace::TraceSink, PromptSnapshot,
    RunResult, StopReason, TraceError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TRACE_EVENT_SCHEMA_VERSION: u32 = 1;
pub const CLIENT_EVENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub v: u32,
    pub session_id: String,
    #[serde(default)]
    pub run_id: String,
    pub turn_id: usize,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub event: TraceEventV1,
    pub redacted: bool,
    #[serde(default)]
    pub workspace_snapshot: Option<gestalt_core::snapshot::WorkspaceSnapshot>,
    #[serde(default)]
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientEventRecordV1 {
    pub v: u32,
    pub session_id: String,
    pub run_id: String,
    pub turn_id: usize,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub payload: ClientEventPayloadV1,
    pub redacted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEventPayloadV1 {
    RunStarted {
        provider: String,
        model: String,
    },
    UserMessage {
        content: String,
    },
    AssistantText {
        delta: String,
    },
    AssistantThinking {
        delta: String,
    },
    Context {
        kind: String,
        token_estimate: Option<usize>,
        detail: Option<String>,
    },
    Model {
        kind: String,
        provider: Option<String>,
        model: Option<String>,
    },
    Tool {
        kind: String,
        call_id: Option<String>,
        name: Option<String>,
        status: Option<String>,
    },
    Policy {
        kind: String,
        tool_call_id: String,
        decision: Option<String>,
        reason: Option<String>,
    },
    Approval {
        kind: String,
        tool_call_id: String,
        decision: Option<String>,
    },
    Artifact {
        size_bytes: usize,
        mime_type: String,
        hash: String,
    },
    Usage {
        input_tokens: usize,
        output_tokens: usize,
    },
    Stop {
        reason: String,
    },
    Error {
        kind: String,
        message: String,
        recoverable: bool,
    },
    Lifecycle {
        kind: String,
    },
    Unknown {
        kind: String,
    },
}

impl From<&EventEnvelope> for ClientEventRecordV1 {
    fn from(envelope: &EventEnvelope) -> Self {
        let (payload, projection_redacted) = project_client_payload(&envelope.event);
        Self {
            v: CLIENT_EVENT_SCHEMA_VERSION,
            session_id: envelope.session_id.clone(),
            run_id: envelope.run_id.clone(),
            turn_id: envelope.turn_id,
            seq: envelope.seq,
            ts: envelope.ts,
            payload,
            redacted: envelope.redacted || projection_redacted,
        }
    }
}

fn project_client_payload(event: &TraceEventV1) -> (ClientEventPayloadV1, bool) {
    use ClientEventPayloadV1::{
        Approval, Artifact, AssistantText, AssistantThinking, Context, Error, Lifecycle, Model,
        Policy, RunStarted, Stop, Tool, Usage, UserMessage,
    };

    let kind = || trace_event_kind(event);
    match event {
        TraceEventV1::RunStarted { resolved_model } => (
            RunStarted {
                provider: resolved_model.selection.provider_id.clone(),
                model: resolved_model.selection.model_id.clone(),
            },
            false,
        ),
        TraceEventV1::UserMessage { content } => {
            let (content, redacted) = redact_string(content);
            (UserMessage { content }, redacted)
        }
        TraceEventV1::Text { delta } => {
            let (delta, redacted) = redact_string(delta);
            (AssistantText { delta }, redacted)
        }
        TraceEventV1::Thinking { delta } => {
            let (delta, redacted) = redact_string(delta);
            (AssistantThinking { delta }, redacted)
        }
        TraceEventV1::ContextBuilt { token_estimate, .. } => (
            Context {
                kind: kind(),
                token_estimate: Some(*token_estimate),
                detail: None,
            },
            false,
        ),
        TraceEventV1::EphemeralContextInjected {
            source,
            token_estimate,
        } => {
            let (detail, redacted) = redact_string(source);
            (
                Context {
                    kind: kind(),
                    token_estimate: Some(*token_estimate),
                    detail: Some(detail),
                },
                redacted,
            )
        }
        TraceEventV1::ContextBuildFailed { reason }
        | TraceEventV1::NextTurnBlocked { reason }
        | TraceEventV1::WorkspaceContextSkipped { reason }
        | TraceEventV1::WorkspaceContextRejected { reason }
        | TraceEventV1::MemoryContextSkipped { reason }
        | TraceEventV1::MemoryContextRejected { reason } => {
            let (detail, redacted) = redact_string(reason);
            (
                Context {
                    kind: kind(),
                    token_estimate: None,
                    detail: Some(detail),
                },
                redacted,
            )
        }
        TraceEventV1::ContextManagementFailed { error }
        | TraceEventV1::ContextExhaustion { details: error }
        | TraceEventV1::WorkspaceContextLoadFailed { error }
        | TraceEventV1::MemoryContextLoadFailed { error } => {
            let (message, redacted) = redact_string(error);
            (
                Error {
                    kind: kind(),
                    message,
                    recoverable: true,
                },
                redacted,
            )
        }
        TraceEventV1::ModelRequest {
            provider, model, ..
        } => (
            Model {
                kind: kind(),
                provider: Some(provider.clone()),
                model: Some(model.clone()),
            },
            false,
        ),
        TraceEventV1::ModelResponseStreamFailed { error, .. } => {
            let (message, redacted) = redact_string(error);
            (
                Error {
                    kind: kind(),
                    message,
                    recoverable: true,
                },
                redacted,
            )
        }
        TraceEventV1::ModelResponseStarted { .. }
        | TraceEventV1::ModelResponseStreamCompleted { .. }
        | TraceEventV1::ModelResponseStreamInterrupted { .. } => (
            Model {
                kind: kind(),
                provider: None,
                model: None,
            },
            false,
        ),
        TraceEventV1::ToolCallStreamed { id, name, .. }
        | TraceEventV1::ToolCallProposed { id, name, .. }
        | TraceEventV1::ToolExecutionStarted {
            id,
            tool_name: name,
            ..
        } => (
            Tool {
                kind: kind(),
                call_id: Some(id.clone()),
                name: Some(name.clone()),
                status: None,
            },
            false,
        ),
        TraceEventV1::ToolResult {
            id,
            tool_name,
            is_error,
            truncated,
            ..
        } => (
            Tool {
                kind: kind(),
                call_id: Some(id.clone()),
                name: tool_name.clone(),
                status: Some(if *is_error {
                    "error".to_string()
                } else if *truncated {
                    "truncated".to_string()
                } else {
                    "ok".to_string()
                }),
            },
            false,
        ),
        TraceEventV1::ToolCallValidationFailed {
            tool_call_id,
            tool_name,
            ..
        }
        | TraceEventV1::PolicyViolation {
            tool_call_id,
            tool_name,
            ..
        } => (
            Tool {
                kind: kind(),
                call_id: Some(tool_call_id.clone()),
                name: Some(tool_name.clone()),
                status: Some("rejected".to_string()),
            },
            false,
        ),
        TraceEventV1::ToolRetryAttempt {
            tool_call_id,
            attempt,
            ..
        } => (
            Tool {
                kind: kind(),
                call_id: Some(tool_call_id.clone()),
                name: None,
                status: Some(format!("retry_{attempt}")),
            },
            false,
        ),
        TraceEventV1::PolicyDecision {
            tool_call_id,
            decision,
            reason,
            ..
        } => {
            let (reason, redacted) = reason.as_ref().map_or((None, false), |reason| {
                let (reason, redacted) = redact_string(reason);
                (Some(reason), redacted)
            });
            (
                Policy {
                    kind: kind(),
                    tool_call_id: tool_call_id.clone(),
                    decision: Some(policy_status_name(*decision).to_string()),
                    reason,
                },
                redacted,
            )
        }
        TraceEventV1::PolicyEvaluationStarted { tool_call_id }
        | TraceEventV1::PolicyEvaluationCancelled { tool_call_id } => (
            Policy {
                kind: kind(),
                tool_call_id: tool_call_id.clone(),
                decision: None,
                reason: None,
            },
            false,
        ),
        TraceEventV1::PolicyEvaluationFailed {
            tool_call_id,
            error,
        } => {
            let (reason, redacted) = redact_string(error);
            (
                Policy {
                    kind: kind(),
                    tool_call_id: tool_call_id.clone(),
                    decision: None,
                    reason: Some(reason),
                },
                redacted,
            )
        }
        TraceEventV1::ApprovalDecision {
            tool_call_id,
            decision,
            ..
        } => (
            Approval {
                kind: kind(),
                tool_call_id: tool_call_id.clone(),
                decision: Some(approval_outcome_name(*decision).to_string()),
            },
            false,
        ),
        TraceEventV1::ApprovalRequested { tool_call_id, .. }
        | TraceEventV1::ApprovalCancelled { tool_call_id } => (
            Approval {
                kind: kind(),
                tool_call_id: tool_call_id.clone(),
                decision: None,
            },
            false,
        ),
        TraceEventV1::ArtifactCreated {
            size_bytes,
            mime_type,
            hash,
            ..
        } => (
            Artifact {
                size_bytes: *size_bytes,
                mime_type: mime_type.clone(),
                hash: hash.clone(),
            },
            false,
        ),
        TraceEventV1::Usage {
            input_tokens,
            output_tokens,
        } => (
            Usage {
                input_tokens: *input_tokens,
                output_tokens: *output_tokens,
            },
            false,
        ),
        TraceEventV1::Stop { reason } => (
            Stop {
                reason: stop_reason_name(*reason).to_string(),
            },
            false,
        ),
        TraceEventV1::Error {
            message,
            recoverable,
        } => {
            let (message, redacted) = redact_string(message);
            (
                Error {
                    kind: kind(),
                    message,
                    recoverable: *recoverable,
                },
                redacted,
            )
        }
        TraceEventV1::Checkpoint { .. }
        | TraceEventV1::AssistantMessageCommitted { .. }
        | TraceEventV1::SessionMessageInjected { .. }
        | TraceEventV1::ToolCatalogSelected { .. }
        | TraceEventV1::ContextCompactionStarted { .. }
        | TraceEventV1::ContextCompacted { .. } => (
            Context {
                kind: kind(),
                token_estimate: None,
                detail: None,
            },
            false,
        ),
        _ => (Lifecycle { kind: kind() }, false),
    }
}

fn trace_event_kind(event: &TraceEventV1) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| value.get("type")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_string())
}

const fn policy_status_name(status: gestalt_core::event::PolicyStatus) -> &'static str {
    match status {
        gestalt_core::event::PolicyStatus::Allowed => "allowed",
        gestalt_core::event::PolicyStatus::Confirm => "confirm",
        gestalt_core::event::PolicyStatus::Denied => "denied",
    }
}

const fn approval_outcome_name(outcome: gestalt_core::event::ApprovalOutcome) -> &'static str {
    match outcome {
        gestalt_core::event::ApprovalOutcome::Approve => "approve",
        gestalt_core::event::ApprovalOutcome::Deny => "deny",
        gestalt_core::event::ApprovalOutcome::Edit => "edit",
        gestalt_core::event::ApprovalOutcome::AlwaysAllow => "always_allow",
    }
}

const fn stop_reason_name(reason: StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::ToolUse => "tool_use",
        StopReason::MaxOutput => "max_output",
        StopReason::ContentFiltered => "content_filtered",
        StopReason::MaxTurns => "max_turns",
        StopReason::BudgetExhausted => "budget_exhausted",
        StopReason::PolicyViolation => "policy_violation",
        StopReason::ProviderError => "provider_error",
        StopReason::HookBlocked => "hook_blocked",
    }
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
    run_id: String,
    pub artifacts_dir: PathBuf,
    state: Mutex<TraceState>,
}

#[derive(Debug)]
struct TraceState {
    writer: BufWriter<File>,
    seq: u64,
    turn_id: usize,
    workspace_snapshot: Option<gestalt_core::snapshot::WorkspaceSnapshot>,
}

impl JsonlTraceSink {
    pub fn new(
        session_id: impl Into<String>,
        run_id: impl Into<String>,
        trace_path: impl AsRef<Path>,
        artifacts_dir: PathBuf,
        workspace_snapshot: Option<gestalt_core::snapshot::WorkspaceSnapshot>,
    ) -> Result<Self, TraceError> {
        let trace_path = trace_path.as_ref();
        if let Some(parent) = trace_path.parent() {
            fs::create_dir_all(parent).map_err(TraceError::WriteFailed)?;
        }

        let file = File::create(trace_path).map_err(TraceError::WriteFailed)?;
        Ok(Self {
            session_id: session_id.into(),
            run_id: run_id.into(),
            artifacts_dir,
            state: Mutex::new(TraceState {
                writer: BufWriter::new(file),
                seq: 0,
                turn_id: 0,
                workspace_snapshot,
            }),
        })
    }

    pub fn create_run(
        base_dir: impl AsRef<Path>,
        session_id: &str,
        run_id: &str,
        workspace_snapshot: Option<gestalt_core::snapshot::WorkspaceSnapshot>,
    ) -> Result<(Self, RunPaths), TraceError> {
        let paths = create_run_paths(base_dir, run_id)?;
        let sink = Self::new(
            session_id.to_string(),
            run_id.to_string(),
            &paths.trace,
            paths.artifacts.clone(),
            workspace_snapshot,
        )?;
        Ok((sink, paths))
    }
}

impl TraceSink for JsonlTraceSink {
    fn run_id(&self) -> Option<&str> {
        Some(&self.run_id)
    }

    fn artifacts_dir(&self) -> Option<&Path> {
        Some(&self.artifacts_dir)
    }

    fn emit(&self, event: CoreAgentEvent) -> Result<(), TraceError> {
        use gestalt_core::event::AgentEvent;

        let mut state = self
            .state
            .lock()
            .map_err(|_| TraceError::WriteFailed(std::io::Error::other("trace sink poisoned")))?;

        if matches!(event, AgentEvent::ContextBuilt { .. }) {
            state.turn_id = state.turn_id.saturating_add(1);
        }

        state.seq = state.seq.saturating_add(1);
        let (event, redacted) = redact_event(&event);
        let snapshot_id = state
            .workspace_snapshot
            .as_ref()
            .map(|s| s.content_hash.chars().take(12).collect::<String>());
        let event = TraceEventV1::try_from(event).map_err(|err| TraceError::InvalidFormat {
            line: 0,
            reason: format!("agent event cannot be represented by trace schema v1: {err}"),
        })?;
        let envelope = EventEnvelope {
            v: 1,
            session_id: self.session_id.clone(),
            run_id: self.run_id.clone(),
            turn_id: state.turn_id,
            seq: state.seq,
            ts: Utc::now(),
            event,
            redacted,
            workspace_snapshot: state.workspace_snapshot.clone(),
            snapshot_id,
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

    fn update_snapshot(&self, snapshot: gestalt_core::snapshot::WorkspaceSnapshot) {
        if let Ok(mut state) = self.state.lock() {
            state.workspace_snapshot = Some(snapshot);
        }
    }
}

impl Drop for JsonlTraceSink {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            let _ = state.writer.flush();
        }
    }
}

pub fn create_run_paths(base_dir: impl AsRef<Path>, run_id: &str) -> Result<RunPaths, TraceError> {
    let stamp = Utc::now().format("%Y%m%dT%H%M%S.%fZ");
    let root = base_dir.as_ref().join(format!("{stamp}-{run_id}"));
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

pub fn write_prompt_snapshot(
    path: impl AsRef<Path>,
    snapshot: &PromptSnapshot,
) -> Result<(), TraceError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(TraceError::WriteFailed)?;
    }
    let bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|err| TraceError::WriteFailed(std::io::Error::other(err)))?;
    fs::write(path, bytes).map_err(TraceError::WriteFailed)
}

pub fn read_prompt_snapshot(path: impl AsRef<Path>) -> Result<PromptSnapshot, TraceError> {
    let file = File::open(path).map_err(TraceError::WriteFailed)?;
    serde_json::from_reader(file).map_err(|err| TraceError::InvalidFormat {
        line: 0,
        reason: err.to_string(),
    })
}

pub fn read_trace(path: impl AsRef<Path>) -> Result<Vec<EventEnvelope>, TraceError> {
    let file = File::open(path).map_err(TraceError::WriteFailed)?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(TraceError::WriteFailed)?;
        let Some(envelope) = parse_trace_envelope_line(&line, index + 1)? else {
            continue;
        };
        events.push(envelope);
    }

    Ok(events)
}

pub fn parse_trace_envelope_line(
    line: &str,
    line_number: usize,
) -> Result<Option<EventEnvelope>, TraceError> {
    let value: Value = serde_json::from_str(line).map_err(|err| TraceError::InvalidFormat {
        line: line_number,
        reason: err.to_string(),
    })?;

    let version =
        value
            .get("v")
            .and_then(Value::as_u64)
            .ok_or_else(|| TraceError::InvalidFormat {
                line: line_number,
                reason: "missing or invalid trace schema version".to_string(),
            })?;

    if version != u64::from(TRACE_EVENT_SCHEMA_VERSION) {
        return Err(TraceError::InvalidFormat {
            line: line_number,
            reason: format!(
                "unsupported trace schema version {version} (expected {})",
                TRACE_EVENT_SCHEMA_VERSION
            ),
        });
    }

    let Some(kind) = value
        .get("event")
        .and_then(|event| event.get("type"))
        .and_then(Value::as_str)
    else {
        return Err(TraceError::InvalidFormat {
            line: line_number,
            reason: "missing trace event kind".to_string(),
        });
    };

    if !is_known_kind(kind) {
        tracing::warn!(
            line = line_number,
            event_kind = kind,
            "skipping unknown trace event"
        );
        return Ok(None);
    }

    serde_json::from_value(value)
        .map(Some)
        .map_err(|err| TraceError::InvalidFormat {
            line: line_number,
            reason: err.to_string(),
        })
}

#[derive(Deserialize)]
struct ClientEnvelopeMetadata {
    v: u32,
    session_id: String,
    #[serde(default)]
    run_id: String,
    turn_id: usize,
    seq: u64,
    ts: DateTime<Utc>,
    #[serde(default)]
    redacted: bool,
}

/// Projects a raw trace JSON line into the stable client event contract.
///
/// Unknown event kinds retain their envelope ordering metadata and become
/// [`ClientEventPayloadV1::Unknown`]. Known kinds must satisfy the complete
/// trace schema.
pub fn project_client_event_line(
    line: &str,
    line_number: usize,
) -> Result<ClientEventRecordV1, TraceError> {
    let value: Value = serde_json::from_str(line).map_err(|err| TraceError::InvalidFormat {
        line: line_number,
        reason: err.to_string(),
    })?;
    let metadata: ClientEnvelopeMetadata =
        serde_json::from_value(value.clone()).map_err(|err| TraceError::InvalidFormat {
            line: line_number,
            reason: err.to_string(),
        })?;
    if metadata.v != TRACE_EVENT_SCHEMA_VERSION {
        return Err(TraceError::InvalidFormat {
            line: line_number,
            reason: format!(
                "unsupported trace schema version {} (expected {})",
                metadata.v, TRACE_EVENT_SCHEMA_VERSION
            ),
        });
    }
    let kind = value
        .get("event")
        .and_then(|event| event.get("type"))
        .and_then(Value::as_str)
        .ok_or_else(|| TraceError::InvalidFormat {
            line: line_number,
            reason: "missing trace event kind".to_string(),
        })?;

    if is_known_kind(kind) {
        let envelope: EventEnvelope =
            serde_json::from_value(value).map_err(|err| TraceError::InvalidFormat {
                line: line_number,
                reason: err.to_string(),
            })?;
        return Ok(ClientEventRecordV1::from(&envelope));
    }

    Ok(ClientEventRecordV1 {
        v: CLIENT_EVENT_SCHEMA_VERSION,
        session_id: metadata.session_id,
        run_id: metadata.run_id,
        turn_id: metadata.turn_id,
        seq: metadata.seq,
        ts: metadata.ts,
        payload: ClientEventPayloadV1::Unknown {
            kind: kind.to_string(),
        },
        redacted: metadata.redacted,
    })
}

#[allow(clippy::format_push_string)]
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
            AgentEvent::PromptSnapshotCreated {
                snapshot_hash,
                prefix_hash,
                created_turn,
            } => lines.push(format!(
                "snapshot-created> {} prefix={} turn={created_turn}",
                &snapshot_hash[..8.min(snapshot_hash.len())],
                &prefix_hash[..8.min(prefix_hash.len())]
            )),
            AgentEvent::PromptSnapshotLoaded {
                snapshot_hash,
                source,
            } => lines.push(format!(
                "snapshot-loaded> {} source={source}",
                &snapshot_hash[..8.min(snapshot_hash.len())]
            )),
            AgentEvent::PromptSnapshotReused {
                snapshot_hash,
                prefix_hash,
            } => lines.push(format!(
                "snapshot-reused> {} prefix={}",
                &snapshot_hash[..8.min(snapshot_hash.len())],
                &prefix_hash[..8.min(prefix_hash.len())]
            )),
            AgentEvent::PromptCachePlanGenerated {
                snapshot_hash,
                prefix_hash,
                prefix_message_count,
            } => lines.push(format!(
                "cache-plan> {} prefix={} messages={prefix_message_count}",
                &snapshot_hash[..8.min(snapshot_hash.len())],
                &prefix_hash[..8.min(prefix_hash.len())]
            )),
            AgentEvent::EphemeralContextInjected {
                source,
                token_estimate,
            } => lines.push(format!("ephemeral> {source} ({token_estimate} tokens)")),
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
                    extra.push_str(&format!(" temp={t}"));
                }
                if let Some(m) = max_tokens {
                    extra.push_str(&format!(" max_tokens={m}"));
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
                failure,
            } => {
                let mut extra = String::new();
                if let Some(name) = tool_name {
                    extra.push_str(&format!(" name={name}"));
                }
                if let Some(dir) = working_dir {
                    extra.push_str(&format!(" dir={dir}"));
                }
                if let Some(ms) = duration_ms {
                    extra.push_str(&format!(" duration={ms}ms"));
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
                    extra.push_str(&format!(" policy_source={src}"));
                }
                if let Some(failure) = failure {
                    extra.push_str(&format!(" failure={}", failure.kind));
                    if let Some(guidance) = &failure.repair_guidance {
                        extra.push_str(&format!(
                            " repair={}",
                            guidance.chars().take(60).collect::<String>()
                        ));
                    }
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
                ..
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
            AgentEvent::WorkspaceSnapshotCaptured { snapshot_id, dirty } => {
                lines.push(format!("snapshot> id={snapshot_id} dirty={dirty}"));
            }
            AgentEvent::ToolCatalogSelected { tools } => {
                // Surface the alias-to-canonical mapping so replay
                // readers can recover internal identity from the
                // provider-facing name. We render each entry on its
                // own line for grep-friendly output, and a header
                // line carries the count for quick scanning.
                lines.push(format!("tool-catalog> selected={} tools", tools.len()));
                for mapping in tools {
                    lines.push(format!(
                        "tool-catalog> alias={} internal={} hash={} strict={}",
                        mapping.provider_name,
                        mapping.internal_id,
                        &mapping.descriptor_hash[..8.min(mapping.descriptor_hash.len())],
                        mapping.strict.unwrap_or(false),
                    ));
                }
            }
            AgentEvent::ToolCallValidationFailed {
                tool_call_id,
                tool_name,
                error,
            } => {
                let mut line = format!(
                    "tool-validation> {tool_call_id} tool={tool_name} kind={} msg={}",
                    error.kind, error.message
                );
                if let Some(guidance) = &error.repair_guidance {
                    line.push_str(&format!(" repair={}", guidance));
                }
                lines.push(line);
            }
            AgentEvent::ToolRetryAttempt {
                tool_call_id,
                attempt,
                error,
                delay_ms,
            } => {
                lines.push(format!(
                    "tool-retry> {tool_call_id} attempt={attempt} delay={delay_ms}ms reason={error}"
                ));
            }
            AgentEvent::SessionMessageInjected { message } => {
                lines.push(format!(
                    "steering-injected> id={} source={:?} content={}",
                    message.id, message.source, message.content
                ));
            }
            AgentEvent::SessionMessageQueueDrained { count } => {
                lines.push(format!("steering-drained> count={count}"));
            }
            _ => {}
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
    let snapshot_str = result
        .workspace_snapshot_id
        .as_ref()
        .map(|id| format!("- Workspace snapshot: {id}\n"))
        .unwrap_or_default();
    let summary = format!(
        "# Run Summary\n\n- Session: {}\n{}- Turns: {}\n- Stop reason: {:?}\n- Input tokens: {}\n- Output tokens: {}\n- Artifacts: {}\n",
        result.session_id,
        snapshot_str,
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

fn redact_event(event: &CoreAgentEvent) -> (CoreAgentEvent, bool) {
    use gestalt_core::event::AgentEvent;

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
            failure,
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
                    failure: failure.clone(),
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
            findings,
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
                    findings: findings.clone(),
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

pub(crate) fn redact_string(input: &str) -> (String, bool) {
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
        StopReason::HookBlocked => "hook_blocked",
    }
}
