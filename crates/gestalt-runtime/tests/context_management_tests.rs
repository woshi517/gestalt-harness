use std::sync::{Arc, Mutex};

#[cfg(feature = "trace")]
use gestalt_core::context::StateUpdate;
use gestalt_core::{
    context::{HistoryRange, TokenBudget},
    event::{AgentEvent, StopReason},
    message::{ContentBlock, Message},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    ContextAssembler, ContextPipeline, MessageId, SessionMessage,
};
#[cfg(feature = "trace")]
use gestalt_runtime::CompactionCheckpoint;
use gestalt_runtime::ContextMessageAssembler;
use gestalt_runtime::RuntimeContextPipeline;

fn runtime_pipeline() -> RuntimeContextPipeline {
    RuntimeContextPipeline {
        base: Arc::new(ContextMessageAssembler::new("pipeline-v1").with_prompt_override("prompt")),
        patch_store: Arc::new(Mutex::new(Vec::new())),
    }
}

fn compute_checkpoint_artifact_hash(checkpoint: &gestalt_runtime::CompactionCheckpoint) -> String {
    use sha2::Digest as _;
    let content = serde_json::to_string_pretty(checkpoint).unwrap();
    let mut hasher = sha2::Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn budget(model_limit: usize, reserved_output: usize, minimum_turn_budget: usize) -> TokenBudget {
    TokenBudget {
        model_limit,
        reserved_output,
        used_system: 0,
        used_history: 0,
        used_sources: 0,
        used_tools: 0,
        used_memory: 0,
        minimum_turn_budget,
    }
}

fn policy() -> gestalt_core::ContextManagementPolicy {
    gestalt_core::ContextManagementPolicy {
        enabled: true,
        buffer_tokens: 0,
        keep_recent_tokens: 40,
        keep_recent_turns: 1,
        tool_result_budget_ratio: 0.1,
        compaction_target_ratio: 0.8,
        durability: gestalt_core::DurabilityMode::Required,
        profile: "test".to_string(),
    }
}

fn retention_snapshot() -> gestalt_core::ToolRetentionRegistrySnapshot {
    let mut policies = std::collections::BTreeMap::new();
    policies.insert(
        gestalt_core::CanonicalToolId {
            namespace: gestalt_core::ToolNamespace::BuiltIn,
            name: "view_file".to_string(),
        },
        gestalt_core::ToolRetention {
            clearable: true,
            reconstructible: true,
            retain_errors: true,
        },
    );
    gestalt_core::ToolRetentionRegistrySnapshot {
        policies,
        fingerprint: "test-retention".to_string(),
    }
}

fn provider_capabilities() -> &'static ProviderCapabilities {
    static CAP: ProviderCapabilities = ProviderCapabilities {
        supports_tools: true,
        supports_parallel_tools: false,
        supports_vision: false,
        supports_documents: false,
        supports_thinking: false,
        supports_json_schema_tools: true,
        supports_prompt_caching: false,
        supports_usage_reporting: false,
        supports_streaming: true,
        supports_strict_schema: false,
    };

    &CAP
}

#[derive(Clone)]
struct ThresholdProvider;

#[async_trait::async_trait]
impl Provider for ThresholdProvider {
    fn id(&self) -> &str {
        "threshold"
    }

    fn display_name(&self) -> &str {
        "Threshold"
    }

    fn default_model(&self) -> &str {
        "threshold-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        provider_capabilities()
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(0)
    }

    fn count_request_tokens(
        &self,
        request: &ProviderRequest,
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        let has_tombstone = request.messages.iter().any(|message| {
            matches!(message, Message::ToolResult { content, .. } if content.contains("<tombstone"))
        });

        Ok(if has_tombstone { 90 } else { 150 })
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

#[derive(Clone)]
struct CompactionProvider {
    trace: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Provider for CompactionProvider {
    fn id(&self) -> &str {
        "compaction"
    }

    fn display_name(&self) -> &str {
        "Compaction"
    }

    fn default_model(&self) -> &str {
        "compaction-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        provider_capabilities()
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(0)
    }

    fn count_request_tokens(
        &self,
        request: &ProviderRequest,
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        let has_checkpoint = request.messages.iter().any(|message| {
            matches!(message, Message::System { content } if content.starts_with("### Session Checkpoint Summary"))
        });
        let has_tombstone = request.messages.iter().any(|message| {
            matches!(message, Message::ToolResult { content, .. } if content.contains("<tombstone"))
        });

        Ok(if has_checkpoint {
            120
        } else if has_tombstone {
            220
        } else {
            240
        })
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        self.trace
            .lock()
            .unwrap()
            .push("provider_stream".to_string());

        let checkpoint = serde_json::json!({
            "goal": "Finish migration safely",
            "constraints": [
                "Preserve the customer_id mapping",
                "Do not drop audit events"
            ],
            "completed_work": [],
            "in_progress_work": [],
            "blocked_items": [],
            "key_decisions": [],
            "next_steps": [],
            "critical_context": "Preserve the customer_id mapping. Do not drop audit events.",
            "relevant_references": ["docs/spec.md"]
        })
        .to_string();

        let events = vec![
            Ok(AgentEvent::Text { delta: checkpoint }),
            Ok(AgentEvent::Stop {
                reason: StopReason::EndTurn,
            }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

#[cfg(feature = "trace")]
#[derive(Clone)]
struct SecondCompactionProvider {
    trace: Arc<Mutex<Vec<String>>>,
}

#[cfg(feature = "trace")]
#[async_trait::async_trait]
impl Provider for SecondCompactionProvider {
    fn id(&self) -> &str {
        "second-compaction"
    }

    fn display_name(&self) -> &str {
        "Second Compaction"
    }

    fn default_model(&self) -> &str {
        "second-compaction-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        provider_capabilities()
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(0)
    }

    fn count_request_tokens(
        &self,
        request: &ProviderRequest,
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        let total = request
            .messages
            .iter()
            .map(gestalt_runtime::estimate_message_tokens)
            .sum::<usize>();
        Ok(total)
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        self.trace
            .lock()
            .unwrap()
            .push("provider_stream".to_string());

        let checkpoint = serde_json::json!({
            "goal": "Finish migration safely",
            "constraints": [
                "Preserve the customer_id mapping",
                "Do not drop audit events"
            ],
            "completed_work": [],
            "in_progress_work": [],
            "blocked_items": [],
            "key_decisions": [],
            "next_steps": [],
            "critical_context": "Preserve the customer_id mapping. Do not drop audit events.",
            "relevant_references": ["docs/spec.md"]
        })
        .to_string();

        let events = vec![
            Ok(AgentEvent::Text { delta: checkpoint }),
            Ok(AgentEvent::Stop {
                reason: StopReason::EndTurn,
            }),
        ];
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

fn request_template() -> ProviderRequest {
    ProviderRequest {
        model: "test-model".to_string(),
        max_tokens: 256,
        ..Default::default()
    }
}

fn temp_artifact_dir(label: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "gestalt-context-tests-{label}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn canonical_history(messages: Vec<Message>) -> Vec<SessionMessage> {
    messages
        .into_iter()
        .enumerate()
        .map(|(sequence, message)| SessionMessage {
            id: MessageId {
                origin_session_id: "session-1".to_string(),
                origin_message_namespace: "session-1".to_string(),
                sequence: sequence as u64,
            },
            metadata: match &message {
                Message::User { metadata, .. } => metadata.clone(),
                _ => None,
            },
            message,
        })
        .collect()
}

fn compaction_history() -> Vec<SessionMessage> {
    canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "You must preserve the customer_id mapping during the migration. ".repeat(3),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "view_file".to_string(),
                input: serde_json::json!({"path": "docs/spec.md"}),
            }],
        },
        Message::ToolResult {
            tool_use_id: "tool-1".to_string(),
            content: "spec details about preserving mappings\n".repeat(20),
            is_error: false,
            failure: None,
            tool_name: Some("view_file".to_string()),
            output_hash: Some("hash-1".to_string()),
            artifact_refs: Some(vec!["docs/spec.md".to_string()]),
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "Also do not drop audit events while compacting this history. ".repeat(3),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "I inspected the spec and the audit requirements. ".repeat(4),
            }],
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "Recent turn should stay verbatim.".to_string(),
            }],
            metadata: None,
        },
    ])
}

#[tokio::test]
async fn prepare_context_requires_artifact_dir_when_durability_is_required() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![Message::User {
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
        }],
        metadata: None,
    }]);
    let err = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget(240, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: None,
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .expect_err("required durability should fail without an artifact dir");

    assert!(format!("{err}").contains("artifact directory"));
}

#[tokio::test]
async fn prepare_context_uses_projected_growth_to_trigger_tombstoning_early() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "Earlier turn that can be compacted if needed.".to_string(),
            }],
            metadata: None,
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "Please inspect src/lib.rs".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "view_file".to_string(),
                input: serde_json::json!({"path": "src/lib.rs"}),
            }],
        },
        Message::ToolResult {
            tool_use_id: "tool-1".to_string(),
            content: "pub fn example() {}\n".repeat(120),
            is_error: false,
            failure: None,
            tool_name: Some("view_file".to_string()),
            output_hash: Some("hash-1".to_string()),
            artifact_refs: None,
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "Latest turn must remain recent.".to_string(),
            }],
            metadata: None,
        },
    ]);
    let artifacts = temp_artifact_dir("threshold");

    let packet = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget(200, 60, 24),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .expect("projected growth should trigger clearing before hard overflow");

    assert!(packet.packet.messages.iter().any(|message| {
        matches!(message, Message::ToolResult { content, .. } if content.contains("<tombstone"))
    }));
}

#[tokio::test]
async fn projection_manifest_ids_are_stable_for_identical_packets() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![Message::User {
        content: vec![ContentBlock::Text {
            text: "short request".to_string(),
        }],
        metadata: None,
    }]);
    let artifacts = temp_artifact_dir("manifest");
    let policy = policy();

    for turn_id in [0, 0] {
        let _ = pipeline
            .prepare_context(gestalt_core::ContextPreparationRequest {
                history: &history,
                context_state: &gestalt_core::ContextProjectionState::default(),
                token_budget: &budget(300, 32, 16),
                provider: &ThresholdProvider,
                request_template: &request_template(),
                model: "test-model",
                session_id: "session-1",
                run_id: "run-1",
                turn_id,
                policy: &policy,
                artifacts_dir: Some(artifacts.as_path()),
                tool_retention: &retention_snapshot(),
                emit: &mut |_| Ok(()),
            })
            .await
            .unwrap();
    }

    let manifests: Vec<_> = std::fs::read_dir(&artifacts)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("projection_manifest_"))
        .collect();

    assert_eq!(manifests.len(), 1);
}

#[tokio::test]
async fn prepare_context_compacts_history_and_persists_artifacts() {
    let pipeline = runtime_pipeline();
    let sequence = Arc::new(Mutex::new(Vec::new()));
    let provider = CompactionProvider {
        trace: sequence.clone(),
    };
    let history = compaction_history();
    let artifacts = temp_artifact_dir("compaction");
    let events = sequence.clone();

    let packet = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget(220, 40, 16),
            provider: &provider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |event| {
                match &event {
                    AgentEvent::ContextPressure { .. } => {
                        events.lock().unwrap().push("pressure".to_string());
                    }
                    AgentEvent::ContextClearing { .. } => {
                        events.lock().unwrap().push("clearing".to_string());
                    }
                    AgentEvent::ContextCompactionStarted { .. } => events
                        .lock()
                        .unwrap()
                        .push("compaction_started".to_string()),
                    AgentEvent::ContextCompacted { .. } => {
                        events.lock().unwrap().push("compacted".to_string());
                    }
                    _ => {}
                }
                Ok(())
            },
        })
        .await
        .expect("compaction path should succeed");

    assert!(packet.packet.messages.iter().any(|message| {
        matches!(message, Message::System { content } if content.starts_with("### Session Checkpoint Summary"))
    }));
    assert!(matches!(
        packet.packet.messages.last(),
        Some(Message::User { content, .. })
            if matches!(content.first(), Some(ContentBlock::Text { text }) if text == "Recent turn should stay verbatim.")
    ));

    let sequence = sequence.lock().unwrap().clone();
    let stream_idx = sequence
        .iter()
        .position(|item| item == "provider_stream")
        .unwrap();
    let started_idx = sequence
        .iter()
        .position(|item| item == "compaction_started")
        .unwrap();
    let compacted_idx = sequence
        .iter()
        .position(|item| item == "compacted")
        .unwrap();

    assert!(started_idx < compacted_idx);
    assert!(started_idx < stream_idx);

    let artifact_names: Vec<_> = std::fs::read_dir(&artifacts)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(artifact_names
        .iter()
        .any(|name| name.starts_with("checkpoint_")));
    assert!(artifact_names
        .iter()
        .any(|name| name.starts_with("projection_manifest_")));

    let checkpoint_ref = packet
        .manifest
        .checkpoint_ref
        .as_ref()
        .expect("compaction should record a checkpoint reference");
    let checkpoint_file = artifacts.join(
        checkpoint_ref
            .artifact
            .as_ref()
            .expect("checkpoint ref should include artifact metadata")
            .relative_path
            .clone(),
    );
    let checkpoint: gestalt_runtime::CompactionCheckpoint = serde_json::from_str(
        &std::fs::read_to_string(checkpoint_file).expect("checkpoint file should exist"),
    )
    .expect("checkpoint file should parse");
    assert_eq!(
        checkpoint_ref
            .artifact
            .as_ref()
            .expect("checkpoint ref should include artifact metadata")
            .content_hash,
        compute_checkpoint_artifact_hash(&checkpoint),
    );
}

#[cfg(not(feature = "trace"))]
#[tokio::test]
async fn trace_disabled_compaction_keeps_summary_without_artifact_reference() {
    let pipeline = runtime_pipeline();
    let provider = CompactionProvider {
        trace: Arc::new(Mutex::new(Vec::new())),
    };
    let history = compaction_history();
    let mut compaction_policy = policy();
    compaction_policy.durability = gestalt_core::DurabilityMode::Disabled;

    let prepared = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget(220, 40, 16),
            provider: &provider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &compaction_policy,
            artifacts_dir: None,
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .expect("trace-disabled compaction should succeed");

    assert!(prepared.packet.messages.iter().any(|message| {
        matches!(message, Message::System { content } if content.starts_with("### Session Checkpoint Summary"))
    }));
    assert!(prepared
        .manifest
        .checkpoint_ref
        .as_ref()
        .is_some_and(|checkpoint| checkpoint.artifact.is_none()));
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn active_checkpoint_survives_noop_preparation() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2".to_string(),
            }],
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 3".to_string(),
            }],
            metadata: None,
        },
    ]);
    let checkpoint = CompactionCheckpoint {
        checkpoint_id: "cp-1".to_string(),
        history_range: HistoryRange::new(0, 2),
        history_range_hash: {
            let serialized = serde_json::to_string(&history[0..2]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        },
        policy_version: "v1".to_string(),
        compactor_model: "test".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "Finish migration safely".to_string(),
        constraints: vec![],
        completed_work: vec![],
        in_progress_work: vec![],
        blocked_items: vec![],
        key_decisions: vec![],
        next_steps: vec![],
        critical_context: "Some critical context".to_string(),
        relevant_references: vec![],
    };

    let artifacts = temp_artifact_dir("noop_survive");
    gestalt_runtime::persist_checkpoint(
        &checkpoint,
        &artifacts,
        gestalt_core::DurabilityMode::Required,
    )
    .unwrap();

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: "cp-1".to_string(),
        source_range: HistoryRange::new(0, 2),
        source_hash: checkpoint.history_range_hash.clone(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: "run-1".to_string(),
            relative_path: "checkpoint_cp-1.json".to_string(),
            content_hash: compute_checkpoint_artifact_hash(&checkpoint),
        }),
    });

    let prepared = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    assert_eq!(
        prepared.state_delta.active_checkpoint,
        gestalt_core::context::StateUpdate::Unchanged
    );
    assert!(prepared.packet.messages.iter().any(|message| {
        matches!(message, Message::System { content } if content.contains("Session Checkpoint Summary"))
    }));
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn resume_resolves_checkpoint_from_parent_run() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2".to_string(),
            }],
        },
    ]);
    let checkpoint = CompactionCheckpoint {
        checkpoint_id: "cp-1".to_string(),
        history_range: HistoryRange::new(0, 2),
        history_range_hash: {
            let serialized = serde_json::to_string(&history[0..2]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        },
        policy_version: "v1".to_string(),
        compactor_model: "test".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "Finish migration safely".to_string(),
        constraints: vec![],
        completed_work: vec![],
        in_progress_work: vec![],
        blocked_items: vec![],
        key_decisions: vec![],
        next_steps: vec![],
        critical_context: "Some critical context".to_string(),
        relevant_references: vec![],
    };

    let temp_base = temp_artifact_dir("resume_resolve");
    let parent_artifacts = temp_base.join("runs").join("parent-run").join("artifacts");
    let child_artifacts = temp_base.join("runs").join("child-run").join("artifacts");
    std::fs::create_dir_all(&parent_artifacts).unwrap();
    std::fs::create_dir_all(&child_artifacts).unwrap();

    gestalt_runtime::persist_checkpoint(
        &checkpoint,
        &parent_artifacts,
        gestalt_core::DurabilityMode::Required,
    )
    .unwrap();

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: "cp-1".to_string(),
        source_range: HistoryRange::new(0, 2),
        source_hash: checkpoint.history_range_hash.clone(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: "parent-run".to_string(),
            relative_path: "checkpoint_cp-1.json".to_string(),
            content_hash: compute_checkpoint_artifact_hash(&checkpoint),
        }),
    });

    let prepared = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "child-run",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(child_artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    assert_eq!(
        prepared.state_delta.active_checkpoint,
        gestalt_core::context::StateUpdate::Unchanged
    );
    assert!(prepared.packet.messages.iter().any(|message| {
        matches!(message, Message::System { content } if content.contains("Session Checkpoint Summary"))
    }));
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn continue_after_compaction_reuses_checkpoint() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2".to_string(),
            }],
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 3".to_string(),
            }],
            metadata: None,
        },
    ]);
    let checkpoint = CompactionCheckpoint {
        checkpoint_id: "cp-1".to_string(),
        history_range: HistoryRange::new(0, 2),
        history_range_hash: {
            let serialized = serde_json::to_string(&history[0..2]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        },
        policy_version: "v1".to_string(),
        compactor_model: "test".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "Finish migration safely".to_string(),
        constraints: vec![],
        completed_work: vec![],
        in_progress_work: vec![],
        blocked_items: vec![],
        key_decisions: vec![],
        next_steps: vec![],
        critical_context: "Some critical context".to_string(),
        relevant_references: vec![],
    };

    let artifacts = temp_artifact_dir("continue_reuse");
    gestalt_runtime::persist_checkpoint(
        &checkpoint,
        &artifacts,
        gestalt_core::DurabilityMode::Required,
    )
    .unwrap();

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: "cp-1".to_string(),
        source_range: HistoryRange::new(0, 2),
        source_hash: checkpoint.history_range_hash.clone(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: "run-1".to_string(),
            relative_path: "checkpoint_cp-1.json".to_string(),
            content_hash: compute_checkpoint_artifact_hash(&checkpoint),
        }),
    });

    let prepared = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 1,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    assert_eq!(
        prepared.state_delta.active_checkpoint,
        gestalt_core::context::StateUpdate::Unchanged
    );
    assert!(prepared.packet.messages.iter().any(|message| {
        matches!(message, Message::System { content } if content.contains("Session Checkpoint Summary"))
    }));
}

#[tokio::test]
async fn missing_referenced_checkpoint_artifact() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2".to_string(),
            }],
        },
    ]);

    let artifacts = temp_artifact_dir("missing_artifact");

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: "cp-missing".to_string(),
        source_range: HistoryRange::new(0, 2),
        source_hash: "some-hash".to_string(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: "run-1".to_string(),
            relative_path: "checkpoint_cp-missing.json".to_string(),
            content_hash: "some-hash".to_string(),
        }),
    });

    let res = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await;

    assert!(res.is_err());
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn legacy_checkpoint_artifact_hash_is_migrated() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2".to_string(),
            }],
        },
    ]);

    let checkpoint = CompactionCheckpoint {
        checkpoint_id: "cp-legacy".to_string(),
        history_range: HistoryRange::new(0, 2),
        history_range_hash: {
            let serialized = serde_json::to_string(&history[0..2]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        },
        policy_version: "v1".to_string(),
        compactor_model: "test".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "goal".to_string(),
        constraints: vec![],
        completed_work: vec![],
        in_progress_work: vec![],
        blocked_items: vec![],
        key_decisions: vec![],
        next_steps: vec![],
        critical_context: "critical".to_string(),
        relevant_references: vec![],
    };

    let artifacts = temp_artifact_dir("legacy_hash_migration");
    gestalt_runtime::persist_checkpoint(
        &checkpoint,
        &artifacts,
        gestalt_core::DurabilityMode::Required,
    )
    .unwrap();

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        source_range: checkpoint.history_range,
        source_hash: checkpoint.history_range_hash.clone(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: String::new(),
            relative_path: format!("checkpoint_{}.json", checkpoint.checkpoint_id),
            content_hash: checkpoint.history_range_hash.clone(),
        }),
    });

    let prepared = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    let migrated = match prepared.state_delta.active_checkpoint {
        StateUpdate::Set(ref checkpoint_ref) => checkpoint_ref,
        _ => panic!("expected migrated checkpoint ref to be written back"),
    };
    assert_eq!(
        migrated
            .artifact
            .as_ref()
            .expect("migrated checkpoint should preserve artifact")
            .content_hash,
        compute_checkpoint_artifact_hash(&checkpoint)
    );
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn checkpoint_artifact_path_rejects_parent_dir_escape() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2".to_string(),
            }],
        },
    ]);

    let checkpoint = CompactionCheckpoint {
        checkpoint_id: "cp-path-escape".to_string(),
        history_range: HistoryRange::new(0, 2),
        history_range_hash: {
            let serialized = serde_json::to_string(&history[0..2]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        },
        policy_version: "v1".to_string(),
        compactor_model: "test".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "goal".to_string(),
        constraints: vec![],
        completed_work: vec![],
        in_progress_work: vec![],
        blocked_items: vec![],
        key_decisions: vec![],
        next_steps: vec![],
        critical_context: "critical".to_string(),
        relevant_references: vec![],
    };

    let artifacts = temp_artifact_dir("path_escape");
    gestalt_runtime::persist_checkpoint(
        &checkpoint,
        &artifacts,
        gestalt_core::DurabilityMode::Required,
    )
    .unwrap();

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        source_range: checkpoint.history_range,
        source_hash: checkpoint.history_range_hash.clone(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: String::new(),
            relative_path: "../checkpoint_cp-path-escape.json".to_string(),
            content_hash: compute_checkpoint_artifact_hash(&checkpoint),
        }),
    });

    let res = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await;

    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(err.contains("escapes artifact directory") || err.contains("must be relative"));
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn missing_checkpoint_run_directory_is_an_error() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2".to_string(),
            }],
        },
    ]);

    let checkpoint = CompactionCheckpoint {
        checkpoint_id: "cp-missing-run".to_string(),
        history_range: HistoryRange::new(0, 2),
        history_range_hash: {
            let serialized = serde_json::to_string(&history[0..2]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        },
        policy_version: "v1".to_string(),
        compactor_model: "test".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "goal".to_string(),
        constraints: vec![],
        completed_work: vec![],
        in_progress_work: vec![],
        blocked_items: vec![],
        key_decisions: vec![],
        next_steps: vec![],
        critical_context: "critical".to_string(),
        relevant_references: vec![],
    };

    let artifacts = temp_artifact_dir("missing_run_dir");
    gestalt_runtime::persist_checkpoint(
        &checkpoint,
        &artifacts,
        gestalt_core::DurabilityMode::Required,
    )
    .unwrap();

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        source_range: checkpoint.history_range,
        source_hash: checkpoint.history_range_hash.clone(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: "nonexistent-run".to_string(),
            relative_path: format!("checkpoint_{}.json", checkpoint.checkpoint_id),
            content_hash: compute_checkpoint_artifact_hash(&checkpoint),
        }),
    });

    let res = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await;

    assert!(res.is_err());
    assert!(res
        .unwrap_err()
        .to_string()
        .contains("artifact run directory not found"));
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn second_compaction_maps_projected_range_to_canonical_range() {
    let pipeline = RuntimeContextPipeline {
        base: Arc::new(
            ContextMessageAssembler::new("pipeline-v1")
                .with_prompt_override("prompt long system instruction override. ".repeat(2)),
        ),
        patch_store: Arc::new(Mutex::new(Vec::new())),
    };
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1 long payload. ".repeat(20),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2 long response. ".repeat(20),
            }],
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 3 long query. ".repeat(30),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 4 long tail. ".repeat(15),
            }],
        },
    ]);
    let checkpoint = CompactionCheckpoint {
        checkpoint_id: "cp-1".to_string(),
        history_range: HistoryRange::new(0, 2),
        history_range_hash: {
            let serialized = serde_json::to_string(&history[0..2]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        },
        policy_version: "v1".to_string(),
        compactor_model: "test".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "Finish migration safely".to_string(),
        constraints: vec![],
        completed_work: vec![],
        in_progress_work: vec![],
        blocked_items: vec![],
        key_decisions: vec![],
        next_steps: vec![],
        critical_context: "Some critical context".to_string(),
        relevant_references: vec![],
    };

    let artifacts = temp_artifact_dir("second_compaction");
    gestalt_runtime::persist_checkpoint(
        &checkpoint,
        &artifacts,
        gestalt_core::DurabilityMode::Required,
    )
    .unwrap();

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: "cp-1".to_string(),
        source_range: HistoryRange::new(0, 2),
        source_hash: checkpoint.history_range_hash.clone(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: "run-1".to_string(),
            relative_path: "checkpoint_cp-1.json".to_string(),
            content_hash: compute_checkpoint_artifact_hash(&checkpoint),
        }),
    });

    let provider = SecondCompactionProvider {
        trace: Arc::new(Mutex::new(Vec::new())),
    };

    let mut custom_policy = policy();
    custom_policy.keep_recent_turns = 0;
    custom_policy.keep_recent_tokens = 10;

    let prepared = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(280, 32, 16),
            provider: &provider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 1,
            policy: &custom_policy,
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    if let StateUpdate::Set(cp_ref) = &prepared.state_delta.active_checkpoint {
        assert_eq!(cp_ref.source_range, HistoryRange::new(0, 3));
        let expected_hash = {
            let serialized = serde_json::to_string(&history[0..3]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        assert_eq!(cp_ref.source_hash, expected_hash);
    } else {
        panic!("expected new checkpoint to be set");
    }
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn second_checkpoint_hash_matches_actual_canonical_source() {
    let pipeline = RuntimeContextPipeline {
        base: Arc::new(
            ContextMessageAssembler::new("pipeline-v1")
                .with_prompt_override("prompt long system instruction override. ".repeat(2)),
        ),
        patch_store: Arc::new(Mutex::new(Vec::new())),
    };
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1 long payload. ".repeat(20),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2 long response. ".repeat(20),
            }],
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 3 long query. ".repeat(30),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 4 long tail. ".repeat(15),
            }],
        },
    ]);
    let checkpoint = CompactionCheckpoint {
        checkpoint_id: "cp-1".to_string(),
        history_range: HistoryRange::new(0, 2),
        history_range_hash: {
            let serialized = serde_json::to_string(&history[0..2]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        },
        policy_version: "v1".to_string(),
        compactor_model: "test".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "Finish migration safely".to_string(),
        constraints: vec![],
        completed_work: vec![],
        in_progress_work: vec![],
        blocked_items: vec![],
        key_decisions: vec![],
        next_steps: vec![],
        critical_context: "Some critical context".to_string(),
        relevant_references: vec![],
    };

    let artifacts = temp_artifact_dir("second_checkpoint_hash");
    gestalt_runtime::persist_checkpoint(
        &checkpoint,
        &artifacts,
        gestalt_core::DurabilityMode::Required,
    )
    .unwrap();

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: "cp-1".to_string(),
        source_range: HistoryRange::new(0, 2),
        source_hash: checkpoint.history_range_hash.clone(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: "run-1".to_string(),
            relative_path: "checkpoint_cp-1.json".to_string(),
            content_hash: compute_checkpoint_artifact_hash(&checkpoint),
        }),
    });

    let provider = SecondCompactionProvider {
        trace: Arc::new(Mutex::new(Vec::new())),
    };

    let mut custom_policy = policy();
    custom_policy.keep_recent_turns = 0;
    custom_policy.keep_recent_tokens = 10;

    let prepared = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(280, 32, 16),
            provider: &provider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 1,
            policy: &custom_policy,
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    if let StateUpdate::Set(cp_ref) = &prepared.state_delta.active_checkpoint {
        let expected_hash = {
            let serialized = serde_json::to_string(&history[0..3]).unwrap();
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest as _;
            hasher.update(serialized.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        assert_eq!(cp_ref.source_hash, expected_hash);
    } else {
        panic!("expected new checkpoint to be set");
    }
}

#[tokio::test]
async fn persisted_cleared_result_remains_tombstoned_next_turn() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "please inspect src/lib.rs".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "view_file".to_string(),
                input: serde_json::json!({"path": "src/lib.rs"}),
            }],
        },
        Message::ToolResult {
            tool_use_id: "tool-1".to_string(),
            content: "some file contents".to_string(),
            is_error: false,
            failure: None,
            tool_name: Some("view_file".to_string()),
            output_hash: Some("hash-1".to_string()),
            artifact_refs: None,
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "next turn".to_string(),
            }],
            metadata: None,
        },
    ]);

    let artifacts = temp_artifact_dir("persisted_cleared");

    let mut state = gestalt_core::ContextProjectionState::default();
    state.cleared_tool_results.insert(
        "tool-1".to_string(),
        gestalt_core::context::ClearedToolResultRef {
            tool_use_id: "tool-1".to_string(),
            message_id: history[2].id.clone(),
            output_hash: "hash-1".to_string(),
            artifact: None,
        },
    );

    let prepared = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 1,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    let msg = prepared
        .packet
        .messages
        .iter()
        .find(|m| matches!(m, Message::ToolResult { .. }))
        .expect("expected tool result message");
    if let Message::ToolResult { content, .. } = msg {
        assert!(content.contains("<tombstone"));
    } else {
        panic!("expected tool result message");
    }
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn cleared_result_reference_is_removed_when_source_disappears() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::System {
            content: "### Session Checkpoint Summary".to_string(),
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 3".to_string(),
            }],
            metadata: None,
        },
    ]);
    let checkpoint = CompactionCheckpoint {
        checkpoint_id: "cp-1".to_string(),
        history_range: HistoryRange::new(0, 3),
        history_range_hash: "hash".to_string(),
        policy_version: "v1".to_string(),
        compactor_model: "test".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "Finish migration safely".to_string(),
        constraints: vec![],
        completed_work: vec![],
        in_progress_work: vec![],
        blocked_items: vec![],
        key_decisions: vec![],
        next_steps: vec![],
        critical_context: "Some critical context".to_string(),
        relevant_references: vec![],
    };

    let artifacts = temp_artifact_dir("disappearing_reference");
    gestalt_runtime::persist_checkpoint(
        &checkpoint,
        &artifacts,
        gestalt_core::DurabilityMode::Required,
    )
    .unwrap();

    let mut state = gestalt_core::ContextProjectionState::default();
    state.active_checkpoint = Some(gestalt_core::context::CompactionCheckpointRef {
        checkpoint_id: "cp-1".to_string(),
        source_range: HistoryRange::new(0, 3),
        source_hash: "hash".to_string(),
        artifact: Some(gestalt_core::ArtifactRef {
            run_id: "run-1".to_string(),
            relative_path: "checkpoint_cp-1.json".to_string(),
            content_hash: compute_checkpoint_artifact_hash(&checkpoint),
        }),
    });
    state.cleared_tool_results.insert(
        "tool-1".to_string(),
        gestalt_core::context::ClearedToolResultRef {
            tool_use_id: "tool-1".to_string(),
            message_id: MessageId {
                origin_session_id: "session-1".to_string(),
                origin_message_namespace: "ns-1".to_string(),
                sequence: 2,
            },
            output_hash: "hash-1".to_string(),
            artifact: None,
        },
    );

    let prepared = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &state,
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    assert_eq!(
        prepared.state_delta.cleared_tool_results_remove,
        vec!["tool-1".to_string()]
    );
}

#[tokio::test]
async fn assembler_never_drops_planned_messages() {
    let pipeline = ContextMessageAssembler::new("pipeline-v1");
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".to_string(),
            }],
            metadata: None,
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 2".to_string(),
            }],
            metadata: None,
        },
    ]);
    let plan = gestalt_core::context::ContextPlan {
        history,
        omissions: Vec::new(),
        budget_exhausted: false,
    };

    let packet = pipeline.assemble(&plan).unwrap();
    assert_eq!(packet.messages.len(), 3);
}

#[test]
fn only_runtime_pipeline_implements_context_policy() {
    let pipeline = runtime_pipeline();
    let _: &dyn gestalt_core::ContextPipeline = &pipeline;

    let assembler = ContextMessageAssembler::new("pipeline-v1");
    let _: &dyn gestalt_core::context::ContextAssembler = &assembler;
}

#[tokio::test]
async fn manifest_id_changes_when_omissions_change() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".repeat(100),
            }],
            metadata: None,
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 2".to_string(),
            }],
            metadata: None,
        },
    ]);
    let artifacts = temp_artifact_dir("manifest_omissions");

    let mut disabled_policy = policy();
    disabled_policy.enabled = false;

    let prepared1 = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &disabled_policy,
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    let prepared2 = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget(100, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &disabled_policy,
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    assert_ne!(
        prepared1.manifest.manifest_id,
        prepared2.manifest.manifest_id
    );
}

#[tokio::test]
async fn manifest_id_changes_when_retention_fingerprint_changes() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![Message::User {
        content: vec![ContentBlock::Text {
            text: "msg 1".to_string(),
        }],
        metadata: None,
    }]);
    let artifacts = temp_artifact_dir("manifest_fingerprint");

    let ret1 = retention_snapshot();
    let mut ret2 = ret1.clone();
    ret2.fingerprint = "different-fingerprint".to_string();

    let prepared1 = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &ret1,
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    let prepared2 = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget(1000, 32, 16),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &ret2,
            emit: &mut |_| Ok(()),
        })
        .await
        .unwrap();

    assert_ne!(
        prepared1.manifest.manifest_id,
        prepared2.manifest.manifest_id
    );
}

#[tokio::test]
async fn failed_final_size_validation_publishes_no_active_artifacts() {
    let pipeline = runtime_pipeline();
    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "msg 1".repeat(50),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "msg 2".repeat(50),
            }],
        },
    ]);
    let artifacts = temp_artifact_dir("failed_validation");

    let res = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget(50, 8, 4),
            provider: &ThresholdProvider,
            request_template: &request_template(),
            model: "test-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy(),
            artifacts_dir: Some(artifacts.as_path()),
            tool_retention: &retention_snapshot(),
            emit: &mut |_| Ok(()),
        })
        .await;

    assert!(res.is_err());

    let entries: Vec<_> = std::fs::read_dir(&artifacts)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        entries.is_empty(),
        "Expected no files to be written, found: {:?}",
        entries
    );
}
