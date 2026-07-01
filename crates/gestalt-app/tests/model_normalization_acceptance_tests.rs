use gestalt_core::{
    AgentEvent, ApiFormat, ModelCapabilities, PromptCacheMode, ResolvedModelSnapshot,
};
use gestalt_runtime::unstable::run_manifest::{
    CompatibilityFingerprint, LifecycleState, RunKind, RunManifest,
};

#[test]
fn test_responses_sse_parsing() {
    use gestalt_runtime::unstable::openai::responses::OpenAiResponsesProvider;

    // Simulate raw event streams from OpenAI Responses API
    let raw_events = vec![
        r#"event: response.output_item.added
data: {"type":"response.output_item.added","item":{"id":"fc_123","call_id":"call_123","type":"function_call","name":"execute_tool"}}"#,
        r#"event: response.function_call_arguments.delta
data: {"type":"response.function_call_arguments.delta","item_id":"fc_123","delta":"{\"query\":"}"#,
        r#"event: response.function_call_arguments.delta
data: {"type":"response.function_call_arguments.delta","item_id":"fc_123","delta":"\"hello\"}"}"#,
        r#"event: response.completed
data: {"type":"response.completed","response":{"usage":{"input_tokens":140,"output_tokens":85}}}"#,
    ];

    let joined = raw_events.join("\n\n");
    let parsed = OpenAiResponsesProvider::normalize_sse(&joined);

    // Verify parser output
    let mut tool_stream_count = 0;
    let mut usage_count = 0;

    for event_res in parsed {
        let event = event_res.expect("parse event");
        match event {
            AgentEvent::ToolCallStreamed {
                id,
                name,
                input_delta,
            } => {
                assert_eq!(id, "call_123");
                assert_eq!(name, "execute_tool");
                if tool_stream_count == 0 {
                    assert_eq!(input_delta, "{\"query\":");
                } else {
                    assert_eq!(input_delta, "\"hello\"}");
                }
                tool_stream_count += 1;
            }
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                assert_eq!(input_tokens, 140);
                assert_eq!(output_tokens, 85);
                usage_count += 1;
            }
            _ => {}
        }
    }

    assert_eq!(tool_stream_count, 2);
    assert_eq!(usage_count, 1);
}

#[test]
fn test_manifest_snapshot_serialization() {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let temp_dir = std::env::temp_dir().join(format!(
        "gestalt-manifest-serialization-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let resolved_model = ResolvedModelSnapshot {
        selection: gestalt_core::ModelSelection {
            provider_id: "openai".to_string(),
            model_id: "gpt-4o-mini".to_string(),
            variant: None,
        },
        api_format: ApiFormat::OpenAiResponses,
        display_name: Some("GPT-4o Mini (Responses)".to_string()),
        max_context_tokens: 128_000,
        max_output_tokens: 16_384,
        capabilities: ModelCapabilities {
            streaming: true,
            tools: true,
            vision: true,
            json_mode: true,
            reasoning: false,
            prompt_cache: PromptCacheMode::None,
        },
    };

    let fp = CompatibilityFingerprint {
        context_pipeline_version: "pipeline-v1".to_string(),
        tool_schema_hash: "schema-hash".to_string(),
        policy_fingerprint: "policy-hash".to_string(),
        hook_contract_hash: "hook-hash".to_string(),
        execution_mode: "Yolo".to_string(),
        skill_fingerprint: None,
        workspace_context_snapshot_hash: None,
    };

    let manifest = RunManifest {
        v: 1,
        session_id: "session-abc".to_string(),
        run_id: "run-xyz".to_string(),
        parent_run_id: None,
        base_checkpoint: None,
        run_kind: RunKind::New,
        created_at: chrono::Utc::now(),
        lifecycle_state: LifecycleState::Completed,
        finalized_at: Some(chrono::Utc::now()),
        failure_kind: None,
        interrupted_phase: None,
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        resolved_model: Some(resolved_model.clone()),
        compatibility_fingerprint: fp,
    };

    let manifest_path = temp_dir.join("run.json");
    manifest.save_to(&manifest_path).unwrap();

    // Reload manifest and verify resolved_model
    let loaded = RunManifest::load_from(&manifest_path).unwrap();
    assert!(loaded.resolved_model.is_some());
    let loaded_model = loaded.resolved_model.unwrap();
    assert_eq!(loaded_model.selection.provider_id, "openai");
    assert_eq!(loaded_model.selection.model_id, "gpt-4o-mini");
    assert_eq!(loaded_model.api_format, ApiFormat::OpenAiResponses);
    assert_eq!(loaded_model.max_context_tokens, 128_000);
    assert_eq!(loaded_model.max_output_tokens, 16_384);

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_provider_switch_and_projection_reset() {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-provider-switch-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    // Parent run model (Anthropic)
    let parent_model = ResolvedModelSnapshot {
        selection: gestalt_core::ModelSelection {
            provider_id: "anthropic".to_string(),
            model_id: "claude-3-5-sonnet".to_string(),
            variant: None,
        },
        api_format: ApiFormat::AnthropicMessages,
        display_name: Some("Claude 3.5 Sonnet".to_string()),
        max_context_tokens: 200_000,
        max_output_tokens: 8192,
        capabilities: ModelCapabilities {
            streaming: true,
            tools: true,
            vision: true,
            json_mode: true,
            reasoning: false,
            prompt_cache: PromptCacheMode::Automatic,
        },
    };

    // Current run model (OpenAI)
    let current_model = ResolvedModelSnapshot {
        selection: gestalt_core::ModelSelection {
            provider_id: "openai".to_string(),
            model_id: "gpt-4o-mini".to_string(),
            variant: None,
        },
        api_format: ApiFormat::OpenAiResponses,
        display_name: Some("GPT-4o Mini".to_string()),
        max_context_tokens: 128_000,
        max_output_tokens: 16_384,
        capabilities: ModelCapabilities {
            streaming: true,
            tools: true,
            vision: true,
            json_mode: true,
            reasoning: false,
            prompt_cache: PromptCacheMode::None,
        },
    };

    use gestalt_app::config::{load_effective_config, CliOverrides};

    let overrides = CliOverrides::default();
    let config = load_effective_config(&overrides).unwrap();

    // Reconstruct projection state reset on model change
    let mut parent_context_state = gestalt_core::ContextProjectionState::default();
    parent_context_state.active_checkpoint = Some(gestalt_core::CompactionCheckpointRef {
        checkpoint_id: "chk-10".to_string(),
        source_range: gestalt_core::HistoryRange::default(),
        source_hash: "hash".to_string(),
        artifact: None,
    });

    // Token budget rebudgeting on model change
    let parent_budget = gestalt_core::TokenBudget {
        model_limit: parent_model.max_context_tokens,
        reserved_output: parent_model.max_output_tokens,
        used_system: 0,
        used_history: 0,
        used_sources: 0,
        used_tools: 0,
        used_memory: 0,
        minimum_turn_budget: 16,
    };

    let (context_state, token_budget, model_changed) =
        gestalt_app::sessions::calculate_continuation_state(
            Some(&parent_model),
            &current_model,
            parent_context_state,
            parent_budget,
            &config,
        );

    assert!(model_changed, "model must be detected as changed");
    assert!(context_state.active_checkpoint.is_none());
    assert_eq!(token_budget.model_limit, 128_000);
    assert_eq!(token_budget.reserved_output, 8192);

    let _ = std::fs::remove_dir_all(&temp_dir);
}
