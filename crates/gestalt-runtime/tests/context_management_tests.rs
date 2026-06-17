use std::sync::{Arc, Mutex};

use gestalt_context::MinimalContextPipeline;
use gestalt_core::{
    context::TokenBudget,
    event::{AgentEvent, StopReason},
    message::{ContentBlock, Message},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    ContextPipeline,
};
use gestalt_runtime::RuntimeContextPipeline;

fn runtime_pipeline() -> RuntimeContextPipeline {
    RuntimeContextPipeline {
        base: Arc::new(MinimalContextPipeline::new("pipeline-v1").with_prompt_override("prompt")),
        patch_store: Arc::new(Mutex::new(Vec::new())),
        current_checkpoint: Arc::new(Mutex::new(None)),
    }
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

#[tokio::test]
async fn prepare_context_requires_artifact_dir_when_durability_is_required() {
    let pipeline = runtime_pipeline();
    let err = pipeline
        .prepare_context(
            &[Message::User {
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                metadata: None,
            }],
            &budget(240, 32, 16),
            &ThresholdProvider,
            &request_template(),
            "test-model",
            "session-1",
            "run-1",
            0,
            &policy(),
            None,
            &mut |_| Ok(()),
        )
        .await
        .expect_err("required durability should fail without an artifact dir");

    assert!(format!("{err}").contains("artifact directory"));
}

#[tokio::test]
async fn prepare_context_uses_projected_growth_to_trigger_tombstoning_early() {
    let pipeline = runtime_pipeline();
    let history = vec![
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
    ];
    let artifacts = temp_artifact_dir("threshold");

    let packet = pipeline
        .prepare_context(
            &history,
            &budget(200, 60, 24),
            &ThresholdProvider,
            &request_template(),
            "test-model",
            "session-1",
            "run-1",
            0,
            &policy(),
            Some(artifacts.as_path()),
            &mut |_| Ok(()),
        )
        .await
        .expect("projected growth should trigger clearing before hard overflow");

    assert!(packet.messages.iter().any(|message| {
        matches!(message, Message::ToolResult { content, .. } if content.contains("<tombstone"))
    }));
}

#[tokio::test]
async fn projection_manifest_ids_are_stable_for_identical_packets() {
    let pipeline = runtime_pipeline();
    let history = vec![Message::User {
        content: vec![ContentBlock::Text {
            text: "short request".to_string(),
        }],
        metadata: None,
    }];
    let artifacts = temp_artifact_dir("manifest");
    let policy = policy();

    for turn_id in [0, 0] {
        let _ = pipeline
            .prepare_context(
                &history,
                &budget(300, 32, 16),
                &ThresholdProvider,
                &request_template(),
                "test-model",
                "session-1",
                "run-1",
                turn_id,
                &policy,
                Some(artifacts.as_path()),
                &mut |_| Ok(()),
            )
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
    let history = vec![
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
    ];
    let artifacts = temp_artifact_dir("compaction");
    let events = sequence.clone();

    let packet = pipeline
        .prepare_context(
            &history,
            &budget(220, 40, 16),
            &provider,
            &request_template(),
            "test-model",
            "session-1",
            "run-1",
            0,
            &policy(),
            Some(artifacts.as_path()),
            &mut |event| {
                match &event {
                    AgentEvent::ContextPressure { .. } => {
                        events.lock().unwrap().push("pressure".to_string())
                    }
                    AgentEvent::ContextClearing { .. } => {
                        events.lock().unwrap().push("clearing".to_string())
                    }
                    AgentEvent::ContextCompactionStarted { .. } => events
                        .lock()
                        .unwrap()
                        .push("compaction_started".to_string()),
                    AgentEvent::ContextCompacted { .. } => {
                        events.lock().unwrap().push("compacted".to_string())
                    }
                    _ => {}
                }
                Ok(())
            },
        )
        .await
        .expect("compaction path should succeed");

    assert!(packet.messages.iter().any(|message| {
        matches!(message, Message::System { content } if content.starts_with("### Session Checkpoint Summary"))
    }));
    assert!(matches!(
        packet.messages.last(),
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
}
