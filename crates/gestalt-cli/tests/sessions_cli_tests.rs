use gestalt_cli::config::{load_effective_config, CliOverrides};
use gestalt_cli::sessions::{history_session, inspect_session, list_sessions, run_session_action};
use gestalt_trace::run_manifest::{CompatibilityFingerprint, LifecycleState, RunKind, RunManifest};
use std::fs;
use std::path::PathBuf;

fn create_temp_workspace() -> PathBuf {
    let temp = std::env::temp_dir().join(format!("gestalt-test-sessions-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[tokio::test]
async fn test_sessions_list_inspect_history() {
    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let session_id = "session-test-12345".to_string();

    // 1. Create a root run manifest
    let run1_id = "run-root".to_string();
    let run1_dir = runs_dir.join(format!("20260602T100000Z-{}", run1_id));
    fs::create_dir_all(&run1_dir).unwrap();

    let fingerprint = CompatibilityFingerprint {
        context_pipeline_version: "pipeline-v1".to_string(),
        tool_schema_hash: "hash1".to_string(),
        policy_fingerprint: "policy1".to_string(),
        hook_contract_hash: "hook1".to_string(),
        execution_mode: "Yolo".to_string(),
    };

    let manifest1 = RunManifest {
        v: 1,
        session_id: session_id.clone(),
        run_id: run1_id.clone(),
        parent_run_id: None,
        base_checkpoint: None,
        run_kind: RunKind::New,
        created_at: chrono::Utc::now() - chrono::Duration::hours(2),
        lifecycle_state: LifecycleState::Completed,
        finalized_at: Some(chrono::Utc::now() - chrono::Duration::hours(2)),
        failure_kind: None,
        interrupted_phase: None,
        compatibility_fingerprint: fingerprint.clone(),
    };
    manifest1.save_to(&run1_dir.join("run.json")).unwrap();

    // Create basic trace with a Checkpoint event
    let trace1 = r#"{"v":1,"session_id":"session-test-12345","run_id":"run-root","turn_id":1,"seq":1,"ts":"2026-06-02T10:00:00Z","event":{"type":"checkpoint","history":[],"token_budget":{"model_limit":100,"reserved_output":10,"used_system":0,"used_history":0,"used_sources":0,"used_tools":0,"used_memory":0,"minimum_turn_budget":16}},"redacted":false}
{"v":1,"session_id":"session-test-12345","run_id":"run-root","turn_id":1,"seq":2,"ts":"2026-06-02T10:01:00Z","event":{"type":"user_message","content":"hello"},"redacted":false}
{"v":1,"session_id":"session-test-12345","run_id":"run-root","turn_id":1,"seq":3,"ts":"2026-06-02T10:02:00Z","event":{"type":"stop","reason":"end_turn"},"redacted":false}"#;
    fs::write(run1_dir.join("trace.jsonl"), trace1).unwrap();

    // 2. Create a child run manifest (continued)
    let run2_id = "run-child".to_string();
    let run2_dir = runs_dir.join(format!("20260602T110000Z-{}", run2_id));
    fs::create_dir_all(&run2_dir).unwrap();

    let manifest2 = RunManifest {
        v: 1,
        session_id: session_id.clone(),
        run_id: run2_id.clone(),
        parent_run_id: Some(run1_id.clone()),
        base_checkpoint: Some(1),
        run_kind: RunKind::Continue,
        created_at: chrono::Utc::now() - chrono::Duration::hours(1),
        lifecycle_state: LifecycleState::Interrupted,
        finalized_at: Some(chrono::Utc::now() - chrono::Duration::hours(1)),
        failure_kind: None,
        interrupted_phase: Some("agent_loop".to_string()),
        compatibility_fingerprint: fingerprint.clone(),
    };
    manifest2.save_to(&run2_dir.join("run.json")).unwrap();

    let trace2 = r#"{"v":1,"session_id":"session-test-12345","run_id":"run-child","turn_id":2,"seq":1,"ts":"2026-06-02T11:00:00Z","event":{"type":"checkpoint","history":[],"token_budget":{"model_limit":100,"reserved_output":10,"used_system":0,"used_history":0,"used_sources":0,"used_tools":0,"used_memory":0,"minimum_turn_budget":16}},"redacted":false}
{"v":1,"session_id":"session-test-12345","run_id":"run-child","turn_id":2,"seq":2,"ts":"2026-06-02T11:01:00Z","event":{"type":"interrupted","reason":"signal"},"redacted":false}"#;
    fs::write(run2_dir.join("trace.jsonl"), trace2).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    // 3. Test list_sessions
    let list_rep = list_sessions(&config).unwrap();
    assert_eq!(list_rep.sessions.len(), 1);
    let session_sum = &list_rep.sessions[0];
    assert_eq!(session_sum.session_id, session_id);
    assert_eq!(session_sum.runs_count, 2);
    assert_eq!(session_sum.latest_run_id, run2_id);
    assert_eq!(session_sum.latest_run_status, "interrupted");

    // 4. Test inspect_session
    let inspect_rep = inspect_session(&config, &session_id).unwrap();
    assert_eq!(inspect_rep.session_id, session_id);
    assert_eq!(inspect_rep.runs.len(), 2);
    assert_eq!(inspect_rep.runs[0].run_id, run1_id);
    assert_eq!(inspect_rep.runs[0].run_kind, "new");
    assert_eq!(inspect_rep.runs[1].run_id, run2_id);
    assert_eq!(inspect_rep.runs[1].run_kind, "continue");

    // 5. Test history_session
    let history_rep = history_session(&config, &session_id).unwrap();
    assert_eq!(history_rep.session_id, session_id);
    // There should be checkpoints, user messages, and interrupts in timeline
    assert!(!history_rep.timeline.is_empty());

    // 6. Test preflight validation failure (drift or state mismatch)
    // Resume on run-child should be safe because it is InterruptedSafe (no in-flight ambiguous tools/hooks)
    // But continue on run-child should be rejected because it is interrupted, not completed.
    let cancel = gestalt_core::CancelToken::new();
    let continue_err = run_session_action(
        &config,
        "continue",
        &session_id,
        Some("next prompt".to_string()),
        None,
        None,
        cancel.clone(),
        None,
        None,
    )
    .await;

    assert!(continue_err.is_err());
    let err_msg = format!("{:?}", continue_err.err().unwrap());
    assert!(err_msg.contains("Continue rejected") || err_msg.contains("Only completed head runs"));

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_sessions_successful_resume_and_branch() {
    use gestalt_core::event::AgentEvent;
    use gestalt_core::message::{ContentBlock, Message};
    use gestalt_core::provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest};
    use gestalt_core::ToolCatalog;
    use std::sync::Arc;

    struct MockProvider {
        capabilities: ProviderCapabilities,
    }
    impl MockProvider {
        fn new() -> Self {
            Self {
                capabilities: ProviderCapabilities {
                    supports_tools: true,
                    supports_parallel_tools: true,
                    supports_vision: false,
                    supports_documents: false,
                    supports_thinking: false,
                    supports_json_schema_tools: true,
                    supports_prompt_caching: false,
                    supports_usage_reporting: false,
                    supports_streaming: true,
                    supports_strict_schema: false,
                },
            }
        }
    }
    #[async_trait::async_trait]
    impl Provider for MockProvider {
        fn id(&self) -> &str {
            "mock-provider"
        }
        fn display_name(&self) -> &str {
            "Mock Provider"
        }
        fn default_model(&self) -> &str {
            "mock-model"
        }
        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }
        fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
            None
        }
        fn count_tokens(
            &self,
            _model: &str,
            _messages: &[Message],
        ) -> Result<usize, gestalt_core::HarnessError> {
            Ok(0)
        }
        async fn stream(
            &self,
            _request: ProviderRequest,
        ) -> Result<EventStream, gestalt_core::HarnessError> {
            let events = vec![AgentEvent::Stop {
                reason: gestalt_core::event::StopReason::EndTurn,
            }];
            let stream =
                futures::stream::iter(events.into_iter().map(Ok::<_, gestalt_core::HarnessError>));
            Ok(Box::pin(stream))
        }
    }

    let _ = gestalt_models::registry::register(
        "mock-provider",
        Box::new(|_| Ok(Arc::new(MockProvider::new()) as Arc<dyn Provider>)),
    );

    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let policies_toml = r#"
[policy]
mode = "yolo"
"#;
    fs::write(temp_root.join(".gestalt/policies.toml"), policies_toml).unwrap();

    let config_toml = r#"
[defaults]
profile = "mock-profile"
provider = "mock-provider"
model = "mock-model"
mode = "yolo"
max_turns = 1

[profiles.mock-profile]
provider = "mock-provider"
model = "mock-model"
"#;
    fs::write(temp_root.join(".gestalt/config.toml"), config_toml).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    let session_id = "session-resume-test".to_string();

    // Create a completed parent run
    let run_root_id = "run-root".to_string();
    let run_root_dir = runs_dir.join(format!("20260602T100000Z-{}", run_root_id));
    fs::create_dir_all(&run_root_dir).unwrap();

    // Fingerprint matching what run_session_action generates
    let fingerprint = CompatibilityFingerprint {
        context_pipeline_version: "pipeline-v1".to_string(),
        tool_schema_hash: gestalt_trace::run_manifest::compute_tool_schema_hash(
            &gestalt_tools::default_registry().unwrap().schemas(),
        ),
        policy_fingerprint: gestalt_trace::run_manifest::compute_policy_fingerprint(policies_toml),
        hook_contract_hash: {
            let hook_names = vec![
                "VerificationToolHook".to_string(),
                "EvaluatorHook".to_string(),
            ];
            gestalt_trace::run_manifest::compute_hook_contract_hash(&hook_names)
        },
        execution_mode: "Yolo".to_string(),
    };

    let manifest_root = RunManifest {
        v: 1,
        session_id: session_id.clone(),
        run_id: run_root_id.clone(),
        parent_run_id: None,
        base_checkpoint: None,
        run_kind: RunKind::New,
        created_at: chrono::Utc::now() - chrono::Duration::hours(2),
        lifecycle_state: LifecycleState::Completed,
        finalized_at: Some(chrono::Utc::now() - chrono::Duration::hours(2)),
        failure_kind: None,
        interrupted_phase: None,
        compatibility_fingerprint: fingerprint.clone(),
    };
    manifest_root
        .save_to(&run_root_dir.join("run.json"))
        .unwrap();

    // Write trace with a Checkpoint event containing some history
    let history_msg = Message::Assistant {
        content: vec![ContentBlock::Text {
            text: "Final assistant message response".to_string(),
        }],
    };
    let trace_data = format!(
        "{}\n{}\n",
        r#"{"v":1,"session_id":"session-resume-test","run_id":"run-root","turn_id":1,"seq":1,"ts":"2026-06-02T10:00:00Z","event":{"type":"checkpoint","history":[],"token_budget":{"model_limit":100,"reserved_output":10,"used_system":0,"used_history":0,"used_sources":0,"used_tools":0,"used_memory":0,"minimum_turn_budget":16}},"redacted":false}"#,
        serde_json::to_string(&gestalt_trace::EventEnvelope {
            v: 1,
            session_id: session_id.clone(),
            run_id: run_root_id.clone(),
            turn_id: 1,
            seq: 2,
            ts: chrono::Utc::now(),
            event: AgentEvent::Checkpoint {
                history: vec![history_msg.clone()],
                token_budget: gestalt_core::context::TokenBudget::default(),
                packet_hash: None,
                prompt_source: None,
            },
            redacted: false,
            workspace_snapshot: None,
            snapshot_id: None,
        })
        .unwrap()
    );
    fs::write(run_root_dir.join("trace.jsonl"), trace_data).unwrap();

    // 1. Test CONTINUE - Should succeed since parent is Completed
    let cancel = gestalt_core::CancelToken::new();
    let new_run_path = run_session_action(
        &config,
        "continue",
        &session_id,
        Some("next prompt".to_string()),
        None,
        None,
        cancel.clone(),
        None,
        None,
    )
    .await
    .unwrap();

    // Verify it created a new run directory with CONTINUE kind and correct lineage
    let manifest_new = RunManifest::load_from(&new_run_path.join("run.json")).unwrap();
    assert_eq!(manifest_new.run_kind, RunKind::Continue);
    assert_eq!(manifest_new.parent_run_id.as_ref(), Some(&run_root_id));

    // Verify that history is preserved including the final assistant turn from the checkpoint!
    let new_trace_content = fs::read_to_string(new_run_path.join("trace.jsonl")).unwrap();
    let mut reconstructed_has_history = false;
    for line in new_trace_content.lines() {
        if let Ok(env) = serde_json::from_str::<gestalt_trace::EventEnvelope>(line) {
            if let AgentEvent::Checkpoint { history, .. } = env.event {
                for msg in history {
                    if let Message::Assistant { content } = msg {
                        for block in content {
                            if let ContentBlock::Text { text } = block {
                                if text == "Final assistant message response" {
                                    reconstructed_has_history = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        reconstructed_has_history,
        "Reconstructed run did not preserve final assistant turn history!"
    );

    // 2. Test BRANCHing from a specific checkpoint sequence
    let cancel_branch = gestalt_core::CancelToken::new();
    let branch_run_path = run_session_action(
        &config,
        "branch",
        &run_root_id,
        Some("branched prompt".to_string()),
        Some(1),
        None,
        cancel_branch,
        None,
        None,
    )
    .await
    .unwrap();

    let manifest_branch = RunManifest::load_from(&branch_run_path.join("run.json")).unwrap();
    assert_eq!(manifest_branch.run_kind, RunKind::Branch);
    assert_eq!(manifest_branch.parent_run_id.as_ref(), Some(&run_root_id));
    assert_eq!(manifest_branch.base_checkpoint, Some(1));

    // Branch from sequence 1 should have only the branch prompt in history
    let branch_trace_content = fs::read_to_string(branch_run_path.join("trace.jsonl")).unwrap();
    let mut found_checkpoint = false;
    for line in branch_trace_content.lines() {
        if let Ok(env) = serde_json::from_str::<gestalt_trace::EventEnvelope>(line) {
            if let AgentEvent::Checkpoint { history, .. } = env.event {
                found_checkpoint = true;
                assert_eq!(
                    history.len(),
                    1,
                    "Branched run should only contain the branch prompt"
                );
                if let Message::User { content } = &history[0] {
                    if let gestalt_core::message::ContentBlock::Text { text } = &content[0] {
                        assert_eq!(text, "branched prompt");
                    } else {
                        panic!("Expected text block");
                    }
                } else {
                    panic!("Expected user message");
                }
                break;
            }
        }
    }
    assert!(found_checkpoint);

    let _ = fs::remove_dir_all(&temp_root);
}
