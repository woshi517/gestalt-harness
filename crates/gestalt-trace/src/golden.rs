use async_trait::async_trait;
use serde_json::Value;
use sha2::Digest;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gestalt_core::{
    agent::AgentLoop,
    approval::{ApprovalDecision, ApprovalProvider, ApprovalRequest},
    context::{ContextPacket, ContextPipeline, SessionMessage, TokenBudget},
    error::{HarnessError, ToolError},
    event::AgentEvent,
    message::Message,
    policy::PolicyEngine,
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    session::Session,
    snapshot::WorkspaceSnapshot,
    tool::{RiskLevel, Tool, ToolCatalog, ToolContext, ToolOutput, ToolSchema},
    trace::TraceSink,
};
use gestalt_policy::MinimalPolicyEngine;

use crate::fixture::{FixtureInput, MockToolConfig};
use crate::{read_trace, EventEnvelope, JsonlTraceSink};

#[derive(Debug, Clone)]
pub struct GoldenTrace {
    pub dir: PathBuf,
    pub input: FixtureInput,
    pub context_packet: ContextPacket,
    pub expected: Vec<EventEnvelope>,
}

impl GoldenTrace {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, TraceErrorWrapper> {
        let dir = dir.as_ref().to_path_buf();
        let input_path = dir.join("input.json");
        let context_path = dir.join("context.json");
        let expected_path = dir.join("expected.jsonl");

        let input_file = std::fs::File::open(&input_path).map_err(|err| {
            TraceErrorWrapper::Io(
                err,
                format!("Failed to open input.json in {}", dir.display()),
            )
        })?;
        let input: FixtureInput = serde_json::from_reader(input_file).map_err(|err| {
            TraceErrorWrapper::Serde(
                err,
                format!("Failed to parse input.json in {}", dir.display()),
            )
        })?;

        let context_file = std::fs::File::open(&context_path).map_err(|err| {
            TraceErrorWrapper::Io(
                err,
                format!("Failed to open context.json in {}", dir.display()),
            )
        })?;
        let context_packet: ContextPacket =
            serde_json::from_reader(context_file).map_err(|err| {
                TraceErrorWrapper::Serde(
                    err,
                    format!("Failed to parse context.json in {}", dir.display()),
                )
            })?;

        let expected = read_trace(&expected_path).map_err(|err| {
            TraceErrorWrapper::Trace(
                err,
                format!("Failed to read expected.jsonl in {}", dir.display()),
            )
        })?;

        Ok(Self {
            dir,
            input,
            context_packet,
            expected,
        })
    }
}

#[derive(Debug)]
pub enum TraceErrorWrapper {
    Io(std::io::Error, String),
    Serde(serde_json::Error, String),
    Trace(gestalt_core::TraceError, String),
}

impl std::fmt::Display for TraceErrorWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err, msg) => write!(f, "IO Error: {err} ({msg})"),
            Self::Serde(err, msg) => write!(f, "Serde Error: {err} ({msg})"),
            Self::Trace(err, msg) => write!(f, "Trace Error: {err:?} ({msg})"),
        }
    }
}

impl std::error::Error for TraceErrorWrapper {}

struct FixtureTool {
    config: MockToolConfig,
}

#[async_trait]
impl Tool for FixtureTool {
    fn name(&self) -> &str {
        &self.config.name
    }
    fn description(&self) -> &str {
        &self.config.description
    }
    fn schema(&self) -> ToolSchema {
        self.config.schema.clone()
    }
    fn risk(&self, _input: &Value) -> RiskLevel {
        if self.config.parallel_safe {
            RiskLevel::Low
        } else {
            RiskLevel::Medium
        }
    }
    async fn execute(&self, _input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        if self.config.is_error {
            Err(ToolError::ExecutionFailed(std::io::Error::other(
                self.config.output.clone(),
            )))
        } else {
            Ok(ToolOutput::Text {
                content: self.config.output.clone(),
            })
        }
    }
}

struct FixtureToolCatalog {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolCatalog for FixtureToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|t| t.schema()).collect()
    }
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
    fn descriptors(&self) -> Vec<gestalt_core::tool_descriptor::ToolDescriptor> {
        self.tools.values().map(|t| t.descriptor()).collect()
    }
}

struct FixturePipeline {
    captured: Arc<Mutex<Option<ContextPacket>>>,
}

impl ContextPipeline for FixturePipeline {
    fn process(&self, history: &[SessionMessage], _budget: &TokenBudget) -> Vec<Message> {
        history.iter().map(|entry| entry.message.clone()).collect()
    }
    fn version(&self) -> &str {
        "fixture-pipeline"
    }
    fn build_packet(&self, history: &[SessionMessage], budget: &TokenBudget) -> ContextPacket {
        let messages = self.process(history, budget);
        let version = self.version().to_string();
        let serialized_messages = serde_json::to_string(&messages).unwrap_or_default();
        let to_hash = format!("{serialized_messages}:{version}");

        let mut hasher = sha2::Sha256::new();
        hasher.update(to_hash.as_bytes());
        let packet_hash = format!("{:x}", hasher.finalize());

        let message_hashes = messages
            .iter()
            .map(|msg| {
                let msg_ser = serde_json::to_string(msg).unwrap_or_default();
                let mut hasher = sha2::Sha256::new();
                hasher.update(msg_ser.as_bytes());
                format!("{:x}", hasher.finalize())
            })
            .collect();

        let packet = ContextPacket {
            messages,
            packet_hash,
            pipeline_version: version,
            tokenizer_id: "default".to_string(),
            token_estimate: 0,
            sources: vec![],
            omissions: vec![],
            message_hashes,
            prompt_assembly_strategy: gestalt_core::PromptAssemblyStrategy::Dynamic,
            snapshot_hash: None,
            cache_prefix_hash: None,
            segments: vec![],
            cache_plan: None,
            prompt_source: Some("default".to_string()),
        };
        let mut guard = self.captured.lock().unwrap();
        *guard = Some(packet.clone());
        packet
    }
}

struct FixtureProvider {
    turns: Mutex<VecDeque<Vec<AgentEvent>>>,
    capabilities: ProviderCapabilities,
}

#[async_trait]
impl Provider for FixtureProvider {
    fn id(&self) -> &str {
        "fixture-provider"
    }

    fn display_name(&self) -> &str {
        "Fixture Provider"
    }

    fn default_model(&self) -> &str {
        "fixture-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(&self, _model: &str, messages: &[Message]) -> Result<usize, HarnessError> {
        Ok(messages.len().saturating_mul(8))
    }

    async fn stream(&self, _request: ProviderRequest) -> Result<EventStream, HarnessError> {
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![AgentEvent::Stop {
                reason: gestalt_core::event::StopReason::EndTurn,
            }]
        });

        let stream = futures::stream::iter(events.into_iter().map(Ok::<_, HarnessError>));
        Ok(Box::pin(stream))
    }
}

struct FixtureApprovalProvider {
    decisions: Mutex<HashMap<String, ApprovalDecision>>,
}

#[async_trait]
impl ApprovalProvider for FixtureApprovalProvider {
    async fn approve(&self, request: ApprovalRequest) -> Result<ApprovalDecision, HarnessError> {
        let guard = self.decisions.lock().unwrap();
        Ok(guard
            .get(&request.tool_call_id)
            .cloned()
            .unwrap_or(ApprovalDecision::Approve))
    }
}

pub struct GoldenTraceRunner;

impl GoldenTraceRunner {
    #[allow(clippy::missing_panics_doc)]
    pub async fn run_golden(
        golden: &GoldenTrace,
    ) -> Result<(Vec<EventEnvelope>, ContextPacket), HarnessError> {
        let temp_dir =
            std::env::temp_dir().join(format!("gestalt-golden-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| HarnessError::Trace(gestalt_core::error::TraceError::WriteFailed(e)))?;

        let trace_path = temp_dir.join("trace.jsonl");
        let sink = JsonlTraceSink::new(
            "session-1",
            "run-1",
            &trace_path,
            temp_dir.clone(),
            golden.input.workspace_snapshot.clone(),
        )
        .map_err(HarnessError::Trace)?;
        let sink = Arc::new(sink);

        let turns_deque: VecDeque<Vec<AgentEvent>> =
            golden.input.mock_turns.clone().into_iter().collect();
        let provider = Arc::new(FixtureProvider {
            turns: Mutex::new(turns_deque),
            capabilities: ProviderCapabilities::default(),
        });

        let mut tools_map = HashMap::new();
        for tc in &golden.input.tools {
            tools_map.insert(
                tc.name.clone(),
                Arc::new(FixtureTool { config: tc.clone() }) as Arc<dyn Tool>,
            );
        }
        let tools = Arc::new(FixtureToolCatalog { tools: tools_map });

        let captured_packet = Arc::new(Mutex::new(None));
        let pipeline = Arc::new(FixturePipeline {
            captured: captured_packet.clone(),
        });

        let policy: Arc<dyn PolicyEngine> = if let Some(ref policy_toml) = golden.input.policy_toml
        {
            let cfg = gestalt_policy::PolicyConfig::parse_toml(policy_toml)
                .map_err(HarnessError::Policy)?;
            Arc::new(MinimalPolicyEngine::new(cfg))
        } else {
            Arc::new(MinimalPolicyEngine::default())
        };

        let mut approval_decisions = HashMap::new();
        if let Some(ref decisions_map) = golden.input.approval_decisions {
            for (k, v) in decisions_map {
                let decision = match v.to_lowercase().as_str() {
                    "approve" | "approved" => ApprovalDecision::Approve,
                    "deny" | "denied" => ApprovalDecision::Deny,
                    "always_allow" | "alwaysallow" => ApprovalDecision::AlwaysAllowForSession,
                    _ => ApprovalDecision::Approve,
                };
                approval_decisions.insert(k.clone(), decision);
            }
        }
        let approval = Arc::new(FixtureApprovalProvider {
            decisions: Mutex::new(approval_decisions),
        });

        let evaluator = Arc::new(crate::evaluator::NoopTraceEvaluator);
        let sink_clone = sink.clone();
        let evaluator_hook = Arc::new(
            crate::evaluator::EvaluatorHook::new(evaluator, Some(golden.clone()))
                .with_flush_trigger(Arc::new(move || {
                    let _ = sink_clone.flush();
                })),
        );
        let mut hooks = gestalt_core::HookRegistry::new();
        hooks.register_session_hook(evaluator_hook);

        let loop_ = AgentLoop::new(
            provider,
            tools,
            pipeline,
            policy,
            approval,
            golden.input.session_config.max_turns,
        )
        .with_hooks(hooks);

        let snapshot =
            golden
                .input
                .workspace_snapshot
                .clone()
                .unwrap_or_else(|| WorkspaceSnapshot {
                    workspace_root: temp_dir.clone(),
                    git_sha: None,
                    git_dirty: None,
                    untracked_count: None,
                    content_hash: "dummy".to_string(),
                    captured_at: chrono::Utc::now(),
                });

        let mut session = Session::new(
            "session-1",
            golden.input.session_config.clone(),
            TokenBudget {
                model_limit: 4096,
                reserved_output: 1024,
                used_system: 0,
                used_history: 0,
                used_sources: 0,
                used_tools: 0,
                used_memory: 0,
                minimum_turn_budget: 100,
            },
            ToolContext {
                working_dir: temp_dir.clone(),
                workspace_root: Some(temp_dir.clone()),
                timeout: std::time::Duration::from_secs(30),
                allow_network: true,
                environment: HashMap::new(),
                max_output_bytes: 1024,
                artifact_dir: Some(temp_dir.join("artifacts")),
                current_tool_call_id: None,
                ignore_patterns: Vec::new(),
            },
            golden.input.execution_mode,
            snapshot,
        );

        session.append_message(Message::User {
            content: vec![gestalt_core::message::ContentBlock::Text {
                text: golden.input.user_prompt.clone(),
            }],
            metadata: None,
        });

        let sink_run = sink.clone();
        let cancel = gestalt_core::cancel::CancelToken::new();
        loop_
            .run(&mut session, &cancel, Some(sink.as_ref()), |event| {
                sink_run.emit(event).map_err(HarnessError::Trace)?;
                Ok(())
            })
            .await?;

        sink.flush().map_err(HarnessError::Trace)?;

        let envelopes = read_trace(&trace_path).map_err(HarnessError::Trace)?;

        let _ = std::fs::remove_dir_all(&temp_dir);

        let actual_packet =
            captured_packet
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| ContextPacket {
                    messages: vec![],
                    packet_hash: "dummy_hash".to_string(),
                    pipeline_version: "fixture-pipeline".to_string(),
                    tokenizer_id: "default".to_string(),
                    token_estimate: 0,
                    sources: vec![],
                    omissions: vec![],
                    message_hashes: vec![],
                    prompt_assembly_strategy: gestalt_core::PromptAssemblyStrategy::Dynamic,
                    snapshot_hash: None,
                    cache_prefix_hash: None,
                    segments: vec![],
                    cache_plan: None,
                    prompt_source: Some("default".to_string()),
                });

        Ok((envelopes, actual_packet))
    }

    pub fn assert_golden(
        golden: &GoldenTrace,
        actual: &[EventEnvelope],
        actual_packet: &ContextPacket,
    ) -> Result<(), String> {
        // Assert ContextPacket matches exactly
        if actual_packet != &golden.context_packet {
            return Err(format!(
                "ContextPacket mismatch:\nExpected: {:#?}\nActual: {:#?}",
                golden.context_packet, actual_packet
            ));
        }

        let actual_events: Vec<AgentEvent> = actual.iter().map(|e| e.event.clone()).collect();
        let expected_events: Vec<AgentEvent> =
            golden.expected.iter().map(|e| e.event.clone()).collect();

        policy_decisions_match(&expected_events, &actual_events)?;
        event_ordering_match(&golden.expected, actual)?;
        tool_execution_match(&expected_events, &actual_events)?;
        artifact_created_events_match(&expected_events, &actual_events)?;
        context_built_events_match(&expected_events, &actual_events)?;
        approval_decisions_match(&expected_events, &actual_events)?;
        usage_events_match(&expected_events, &actual_events)?;
        stop_events_match(&expected_events, &actual_events)?;
        verification_results_match(&expected_events, &actual_events)?;

        Ok(())
    }
}

fn get_event_type(event: &AgentEvent) -> String {
    let val = serde_json::to_value(event).unwrap_or_default();
    val.get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string()
}

fn policy_decisions_match(expected: &[AgentEvent], actual: &[AgentEvent]) -> Result<(), String> {
    let expected_decisions: Vec<&AgentEvent> = expected
        .iter()
        .filter(|e| matches!(e, AgentEvent::PolicyDecision { .. }))
        .collect();
    let actual_decisions: Vec<&AgentEvent> = actual
        .iter()
        .filter(|e| matches!(e, AgentEvent::PolicyDecision { .. }))
        .collect();

    if expected_decisions.len() != actual_decisions.len() {
        return Err(format!(
            "Policy decision count mismatch: expected {}, got {}",
            expected_decisions.len(),
            actual_decisions.len()
        ));
    }

    for (i, (exp, act)) in expected_decisions
        .iter()
        .zip(actual_decisions.iter())
        .enumerate()
    {
        match (exp, act) {
            (
                AgentEvent::PolicyDecision {
                    tool_name: exp_tn,
                    input_hash: exp_ih,
                    risk: exp_r,
                    mode: exp_m,
                    matched_rule: exp_mr,
                    decision: exp_d,
                    reason: exp_reason,
                    policy_source: exp_ps,
                    ..
                },
                AgentEvent::PolicyDecision {
                    tool_name: act_tn,
                    input_hash: act_ih,
                    risk: act_r,
                    mode: act_m,
                    matched_rule: act_mr,
                    decision: act_d,
                    reason: act_reason,
                    policy_source: act_ps,
                    ..
                },
            ) => {
                if exp_tn != act_tn {
                    return Err(format!(
                        "Decision #{i} tool_name mismatch: expected {exp_tn:?}, got {act_tn:?}"
                    ));
                }
                if exp_ih != act_ih {
                    return Err(format!(
                        "Decision #{i} input_hash mismatch: expected {exp_ih:?}, got {act_ih:?}"
                    ));
                }
                if exp_r != act_r {
                    return Err(format!(
                        "Decision #{i} risk mismatch: expected {exp_r:?}, got {act_r:?}"
                    ));
                }
                if exp_m != act_m {
                    return Err(format!(
                        "Decision #{i} mode mismatch: expected {exp_m:?}, got {act_m:?}"
                    ));
                }
                if exp_mr != act_mr {
                    return Err(format!(
                        "Decision #{i} matched_rule mismatch: expected {exp_mr:?}, got {act_mr:?}"
                    ));
                }
                if exp_d != act_d {
                    return Err(format!(
                        "Decision #{i} decision mismatch: expected {exp_d:?}, got {act_d:?}"
                    ));
                }
                if exp_reason != act_reason {
                    return Err(format!("Decision #{i} reason mismatch: expected {exp_reason:?}, got {act_reason:?}"));
                }
                if exp_ps != act_ps {
                    return Err(format!(
                        "Decision #{i} policy_source mismatch: expected {exp_ps:?}, got {act_ps:?}"
                    ));
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn event_ordering_match(
    expected: &[EventEnvelope],
    actual: &[EventEnvelope],
) -> Result<(), String> {
    if expected.len() != actual.len() {
        return Err(format!(
            "Event count mismatch: expected {}, got {}",
            expected.len(),
            actual.len()
        ));
    }

    for (i, (exp, act)) in expected.iter().zip(actual.iter()).enumerate() {
        if exp.seq != act.seq {
            return Err(format!(
                "Sequence ID mismatch at index {i}: expected {}, got {}",
                exp.seq, act.seq
            ));
        }

        let exp_type = get_event_type(&exp.event);
        let act_type = get_event_type(&act.event);
        if exp_type != act_type {
            return Err(format!(
                "Event type mismatch at index {i} (seq {}): expected {exp_type}, got {act_type}",
                exp.seq
            ));
        }
    }
    Ok(())
}

fn tool_execution_match(expected: &[AgentEvent], actual: &[AgentEvent]) -> Result<(), String> {
    let expected_results: Vec<&AgentEvent> = expected
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolResult { .. }))
        .collect();
    let actual_results: Vec<&AgentEvent> = actual
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolResult { .. }))
        .collect();

    if expected_results.len() != actual_results.len() {
        return Err(format!(
            "Tool result count mismatch: expected {}, got {}",
            expected_results.len(),
            actual_results.len()
        ));
    }

    let get_input_hash = |events: &[AgentEvent], call_id: &str| -> Option<String> {
        for ev in events {
            if let AgentEvent::PolicyDecision {
                tool_call_id,
                input_hash,
                ..
            } = ev
            {
                if tool_call_id == call_id {
                    if let Some(ref ih) = input_hash {
                        return Some(ih.clone());
                    }
                }
            }
        }
        for ev in events {
            if let AgentEvent::ToolCallProposed { id, input, .. } = ev {
                if id == call_id {
                    let mut hasher = sha2::Sha256::new();
                    let ser = serde_json::to_string(input).unwrap_or_default();
                    hasher.update(ser.as_bytes());
                    return Some(format!("{:x}", hasher.finalize()));
                }
            }
        }
        None
    };

    for (i, (exp, act)) in expected_results
        .iter()
        .zip(actual_results.iter())
        .enumerate()
    {
        match (exp, act) {
            (
                AgentEvent::ToolResult {
                    id: exp_id,
                    tool_name: exp_tn,
                    output_hash: exp_oh,
                    artifact_refs: exp_ar,
                    ..
                },
                AgentEvent::ToolResult {
                    id: act_id,
                    tool_name: act_tn,
                    output_hash: act_oh,
                    artifact_refs: act_ar,
                    ..
                },
            ) => {
                if exp_tn != act_tn {
                    return Err(format!(
                        "ToolResult #{i} tool_name mismatch: expected {exp_tn:?}, got {act_tn:?}"
                    ));
                }
                if exp_oh != act_oh {
                    return Err(format!(
                        "ToolResult #{i} output_hash mismatch: expected {exp_oh:?}, got {act_oh:?}"
                    ));
                }
                if exp_ar != act_ar {
                    return Err(format!("ToolResult #{i} artifact_refs mismatch: expected {exp_ar:?}, got {act_ar:?}"));
                }

                let exp_ih = get_input_hash(expected, exp_id);
                let act_ih = get_input_hash(actual, act_id);
                if exp_ih != act_ih {
                    return Err(format!(
                        "ToolResult #{i} input_hash mismatch: expected {exp_ih:?}, got {act_ih:?}"
                    ));
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn artifact_created_events_match(
    expected: &[AgentEvent],
    actual: &[AgentEvent],
) -> Result<(), String> {
    let expected_artifacts: Vec<&AgentEvent> = expected
        .iter()
        .filter(|e| matches!(e, AgentEvent::ArtifactCreated { .. }))
        .collect();
    let actual_artifacts: Vec<&AgentEvent> = actual
        .iter()
        .filter(|e| matches!(e, AgentEvent::ArtifactCreated { .. }))
        .collect();

    if expected_artifacts.len() != actual_artifacts.len() {
        return Err(format!(
            "Artifact count mismatch: expected {}, got {}",
            expected_artifacts.len(),
            actual_artifacts.len()
        ));
    }

    for (i, (exp, act)) in expected_artifacts
        .iter()
        .zip(actual_artifacts.iter())
        .enumerate()
    {
        match (exp, act) {
            (
                AgentEvent::ArtifactCreated {
                    path: exp_path,
                    size_bytes: exp_size,
                    mime_type: exp_mime,
                    hash: exp_hash,
                },
                AgentEvent::ArtifactCreated {
                    path: act_path,
                    size_bytes: act_size,
                    mime_type: act_mime,
                    hash: act_hash,
                },
            ) => {
                let exp_filename = Path::new(exp_path).file_name();
                let act_filename = Path::new(act_path).file_name();
                if exp_filename != act_filename {
                    return Err(format!("Artifact #{i} filename mismatch: expected {exp_filename:?}, got {act_filename:?}"));
                }
                if exp_size != act_size {
                    return Err(format!(
                        "Artifact #{i} size mismatch: expected {exp_size}, got {act_size}"
                    ));
                }
                if exp_mime != act_mime {
                    return Err(format!(
                        "Artifact #{i} mime_type mismatch: expected {exp_mime:?}, got {act_mime:?}"
                    ));
                }
                if exp_hash != act_hash {
                    return Err(format!(
                        "Artifact #{i} hash mismatch: expected {exp_hash:?}, got {act_hash:?}"
                    ));
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(())
}

fn context_built_events_match(
    expected: &[AgentEvent],
    actual: &[AgentEvent],
) -> Result<(), String> {
    let expected_built: Vec<&AgentEvent> = expected
        .iter()
        .filter(|e| matches!(e, AgentEvent::ContextBuilt { .. }))
        .collect();
    let actual_built: Vec<&AgentEvent> = actual
        .iter()
        .filter(|e| matches!(e, AgentEvent::ContextBuilt { .. }))
        .collect();

    if expected_built.len() != actual_built.len() {
        return Err(format!(
            "ContextBuilt event count mismatch: expected {}, got {}",
            expected_built.len(),
            actual_built.len()
        ));
    }

    for (i, (exp, act)) in expected_built.iter().zip(actual_built.iter()).enumerate() {
        if exp != act {
            return Err(format!(
                "ContextBuilt event #{i} mismatch:\nExpected: {exp:?}\nActual: {act:?}"
            ));
        }
    }
    Ok(())
}

fn approval_decisions_match(expected: &[AgentEvent], actual: &[AgentEvent]) -> Result<(), String> {
    let expected_decisions: Vec<&AgentEvent> = expected
        .iter()
        .filter(|e| matches!(e, AgentEvent::ApprovalDecision { .. }))
        .collect();
    let actual_decisions: Vec<&AgentEvent> = actual
        .iter()
        .filter(|e| matches!(e, AgentEvent::ApprovalDecision { .. }))
        .collect();

    if expected_decisions.len() != actual_decisions.len() {
        return Err(format!(
            "Approval decision count mismatch: expected {}, got {}",
            expected_decisions.len(),
            actual_decisions.len()
        ));
    }

    for (i, (exp, act)) in expected_decisions
        .iter()
        .zip(actual_decisions.iter())
        .enumerate()
    {
        if exp != act {
            return Err(format!(
                "ApprovalDecision #{i} mismatch:\nExpected: {exp:?}\nActual: {act:?}"
            ));
        }
    }
    Ok(())
}

fn usage_events_match(expected: &[AgentEvent], actual: &[AgentEvent]) -> Result<(), String> {
    let expected_usage: Vec<&AgentEvent> = expected
        .iter()
        .filter(|e| matches!(e, AgentEvent::Usage { .. }))
        .collect();
    let actual_usage: Vec<&AgentEvent> = actual
        .iter()
        .filter(|e| matches!(e, AgentEvent::Usage { .. }))
        .collect();

    if expected_usage.len() != actual_usage.len() {
        return Err(format!(
            "Usage event count mismatch: expected {}, got {}",
            expected_usage.len(),
            actual_usage.len()
        ));
    }

    for (i, (exp, act)) in expected_usage.iter().zip(actual_usage.iter()).enumerate() {
        if exp != act {
            return Err(format!(
                "Usage event #{i} mismatch:\nExpected: {exp:?}\nActual: {act:?}"
            ));
        }
    }
    Ok(())
}

fn stop_events_match(expected: &[AgentEvent], actual: &[AgentEvent]) -> Result<(), String> {
    let expected_stop: Vec<&AgentEvent> = expected
        .iter()
        .filter(|e| matches!(e, AgentEvent::Stop { .. }))
        .collect();
    let actual_stop: Vec<&AgentEvent> = actual
        .iter()
        .filter(|e| matches!(e, AgentEvent::Stop { .. }))
        .collect();

    if expected_stop.len() != actual_stop.len() {
        return Err(format!(
            "Stop event count mismatch: expected {}, got {}",
            expected_stop.len(),
            actual_stop.len()
        ));
    }

    for (i, (exp, act)) in expected_stop.iter().zip(actual_stop.iter()).enumerate() {
        if exp != act {
            return Err(format!(
                "Stop event #{i} mismatch:\nExpected: {exp:?}\nActual: {act:?}"
            ));
        }
    }
    Ok(())
}

fn verification_results_match(
    expected: &[AgentEvent],
    actual: &[AgentEvent],
) -> Result<(), String> {
    let expected_ver: Vec<&AgentEvent> = expected
        .iter()
        .filter(|e| matches!(e, AgentEvent::VerificationResult { .. }))
        .collect();
    let actual_ver: Vec<&AgentEvent> = actual
        .iter()
        .filter(|e| matches!(e, AgentEvent::VerificationResult { .. }))
        .collect();

    if expected_ver.len() != actual_ver.len() {
        return Err(format!(
            "VerificationResult count mismatch: expected {}, got {}",
            expected_ver.len(),
            actual_ver.len()
        ));
    }

    for (i, (exp, act)) in expected_ver.iter().zip(actual_ver.iter()).enumerate() {
        if exp != act {
            return Err(format!(
                "VerificationResult #{i} mismatch:\nExpected: {exp:?}\nActual: {act:?}"
            ));
        }
    }
    Ok(())
}
