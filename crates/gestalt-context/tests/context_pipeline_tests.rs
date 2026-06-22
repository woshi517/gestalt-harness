use gestalt_context::ContextMessageAssembler;
use gestalt_core::{
    context::{ContextAssembler, HistoryRange, PromptAssemblyStrategy},
    message::{ContentBlock, Message},
    ContextPipeline, MessageId, SessionMessage, TokenBudget,
};
use gestalt_trace::CompactionCheckpoint;

fn budget(model_limit: usize) -> TokenBudget {
    TokenBudget {
        model_limit,
        reserved_output: 16,
        used_system: 0,
        used_history: 0,
        used_sources: 0,
        used_tools: 0,
        used_memory: 0,
        minimum_turn_budget: 8,
    }
}

fn pipeline() -> ContextMessageAssembler {
    ContextMessageAssembler::new("pipeline-v1")
        .with_workspace_md("workspace rules")
        .with_memory_md("stable memory")
}

fn canonical_history(messages: Vec<Message>) -> Vec<SessionMessage> {
    messages
        .into_iter()
        .enumerate()
        .map(|(sequence, message)| SessionMessage {
            id: MessageId {
                origin_session_id: "test-session".to_string(),
                origin_message_namespace: "test-session".to_string(),
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

fn retention_snapshot(tool_names: &[&str]) -> gestalt_core::ToolRetentionRegistrySnapshot {
    let policies = tool_names
        .iter()
        .map(|name| {
            (
                gestalt_core::CanonicalToolId {
                    namespace: gestalt_core::ToolNamespace::BuiltIn,
                    name: (*name).to_string(),
                },
                gestalt_core::ToolRetention {
                    clearable: true,
                    reconstructible: true,
                    retain_errors: true,
                },
            )
        })
        .collect();
    gestalt_core::ToolRetentionRegistrySnapshot {
        policies,
        fingerprint: "test-retention".to_string(),
    }
}

#[test]
fn dynamic_strategy_keeps_cache_metadata_empty() {
    let plan = gestalt_core::context::ContextPlan {
        history: Vec::new(),
        omissions: Vec::new(),
        budget_exhausted: false,
    };
    let packet = pipeline().assemble(&plan).unwrap();

    assert_eq!(
        packet.prompt_assembly_strategy,
        PromptAssemblyStrategy::Dynamic
    );
    assert!(packet.snapshot_hash.is_none());
    assert!(packet.cache_prefix_hash.is_none());
    assert!(packet.segments.is_empty());
    assert!(packet.cache_plan.is_none());
}

#[test]
fn snapshot_strategy_records_stable_prefix_and_dynamic_tail() {
    let history = canonical_history(vec![Message::User {
        content: vec![ContentBlock::Text {
            text: "hello world".to_string(),
        }],
        metadata: None,
    }]);
    let plan = gestalt_core::context::ContextPlan {
        history,
        omissions: Vec::new(),
        budget_exhausted: false,
    };
    let packet = pipeline()
        .with_prompt_assembly_strategy(PromptAssemblyStrategy::Snapshot)
        .assemble(&plan)
        .unwrap();

    let cache_plan = packet.cache_plan.as_ref().expect("cache plan");

    assert_eq!(
        packet.prompt_assembly_strategy,
        PromptAssemblyStrategy::Snapshot
    );
    assert_eq!(cache_plan.strategy, PromptAssemblyStrategy::Snapshot);
    assert_eq!(packet.snapshot_hash, Some(cache_plan.snapshot_hash.clone()));
    assert_eq!(packet.cache_prefix_hash, Some(cache_plan.prefix_hash.clone()));
    assert_eq!(cache_plan.prefix_message_count, 3);
    assert_eq!(packet.segments, cache_plan.segments);
    assert!(packet
        .segments
        .iter()
        .any(|segment| segment.kind == gestalt_core::context::PromptSegmentKind::Snapshot));
    assert!(packet
        .segments
        .iter()
        .any(|segment| segment.kind == gestalt_core::context::PromptSegmentKind::Conversation));
}

#[test]
fn snapshot_hash_stays_stable_when_history_changes() {
    let p = pipeline().with_prompt_assembly_strategy(PromptAssemblyStrategy::Snapshot);

    let plan1 = gestalt_core::context::ContextPlan {
        history: canonical_history(vec![Message::User {
            content: vec![ContentBlock::Text {
                text: "first".to_string(),
            }],
            metadata: None,
        }]),
        omissions: Vec::new(),
        budget_exhausted: false,
    };
    let first = p.assemble(&plan1).unwrap();

    let plan2 = gestalt_core::context::ContextPlan {
        history: canonical_history(vec![Message::User {
            content: vec![ContentBlock::Text {
                text: "second".to_string(),
            }],
            metadata: None,
        }]),
        omissions: Vec::new(),
        budget_exhausted: false,
    };
    let second = p.assemble(&plan2).unwrap();

    assert_eq!(first.snapshot_hash, second.snapshot_hash);
    assert_ne!(first.packet_hash, second.packet_hash);
}

#[test]
fn budget_exhaustion_is_modelled_as_ephemeral_tail() {
    let plan = gestalt_core::context::ContextPlan {
        history: Vec::new(),
        omissions: vec![gestalt_core::context::ContextOmission {
            kind: "history".to_string(),
            path_or_label: "msg_0".to_string(),
            trust: "trusted".to_string(),
            reason: "budget_exhausted".to_string(),
            token_estimate: 50,
            authority: None,
        }],
        budget_exhausted: true,
    };
    let packet = ContextMessageAssembler::new("pipeline-v1")
        .with_prompt_assembly_strategy(PromptAssemblyStrategy::Snapshot)
        .assemble(&plan)
        .unwrap();

    assert!(packet
        .segments
        .iter()
        .any(|segment| segment.kind == gestalt_core::context::PromptSegmentKind::Ephemeral));
}

#[test]
fn test_tool_clearing_happy_path() {
    use gestalt_context::tool_clearing::clear_eligible_tool_results;
    use serde_json::json;

    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "please read the file".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "view_1".to_string(),
                name: "view_file".to_string(),
                input: json!({"path": "src/lib.rs"}),
            }],
        },
        Message::ToolResult {
            tool_use_id: "view_1".to_string(),
            content: "pub fn main() { println!(\"hello\"); }".repeat(50),
            is_error: false,
            failure: None,
            tool_name: Some("view_file".to_string()),
            output_hash: Some("some_hash".to_string()),
            artifact_refs: Some(vec![]),
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "thanks".to_string(),
            }],
            metadata: None,
        },
    ]);

    // Total tool result tokens is large. Let's set a low tool_result_budget
    let (projected, actions) = clear_eligible_tool_results(
        "test-run",
        &history,
        &retention_snapshot(&["view_file"]),
        1000,
        10,
        1,
        100,
    );

    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].tool_use_id, "view_1");
    assert_eq!(actions[0].tool_name, "view_file");

    if let Message::ToolResult {
        content, is_error, ..
    } = &projected[2].message
    {
        assert!(!is_error);
        assert!(content.contains("<tombstone"));
        assert!(content.contains("tool_name=\"view_file\""));
    } else {
        panic!("expected tool result");
    }
}

#[test]
fn test_tool_clearing_preserves_errors_and_recent_window() {
    use gestalt_context::tool_clearing::clear_eligible_tool_results;
    use serde_json::json;

    let history = canonical_history(vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "run it".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::ToolUse {
                id: "err_1".to_string(),
                name: "view_file".to_string(),
                input: json!({}),
            }],
        },
        Message::ToolResult {
            tool_use_id: "err_1".to_string(),
            content: "error details".to_string(),
            is_error: true,
            failure: None,
            tool_name: Some("view_file".to_string()),
            output_hash: Some("err_hash".to_string()),
            artifact_refs: None,
        },
        Message::User {
            content: vec![ContentBlock::Text {
                text: "try again".to_string(),
            }],
            metadata: None,
        },
    ]);

    // Even with a budget of 0, active errors are preserved
    let (projected, actions) = clear_eligible_tool_results(
        "test-run",
        &history,
        &retention_snapshot(&["view_file"]),
        1000,
        0,
        1,
        100,
    );
    assert!(actions.is_empty());
    assert_eq!(projected, history);
}

#[test]
fn checkpoint_validation_rejects_missing_protected_anchor() {
    let history = vec![
        Message::User {
            content: vec![ContentBlock::Text {
                text: "You must preserve the customer_id mapping during compaction.".to_string(),
            }],
            metadata: None,
        },
        Message::Assistant {
            content: vec![ContentBlock::Text {
                text: "Understood.".to_string(),
            }],
        },
    ];

    let range = HistoryRange::new(0, history.len());
    let range_hash = serde_json::to_string(&history).unwrap();
    let mut hasher = sha2::Sha256::new();
    use sha2::Digest as _;
    hasher.update(range_hash.as_bytes());
    let history_range_hash = format!("{:x}", hasher.finalize());

    let checkpoint: CompactionCheckpoint = serde_json::from_value(serde_json::json!({
        "checkpoint_id": "cp-1",
        "history_range": { "start": range.start, "end": range.end },
        "history_range_hash": history_range_hash,
        "policy_version": "v1",
        "compactor_model": "mock",
        "prompt_hash": "prompt",
        "created_at": "2026-06-18T00:00:00Z",
        "goal": "finish the task",
        "constraints": ["keep the system responsive"],
        "completed_work": [],
        "in_progress_work": [],
        "blocked_items": [],
        "key_decisions": [],
        "next_steps": [],
        "critical_context": "There is a migration in flight.",
        "relevant_references": []
    }))
    .unwrap();

    let err = gestalt_context::checkpoint_validation::validate_checkpoint(
        &checkpoint,
        &history,
        range,
        &checkpoint.history_range_hash,
    )
    .expect_err("protected anchor should be preserved");

    assert!(matches!(
        err,
        gestalt_context::checkpoint_validation::ValidationError::ConstraintViolation(_)
    ));
}
