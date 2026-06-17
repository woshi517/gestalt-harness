use gestalt_context::MinimalContextPipeline;
use gestalt_core::{
    context::PromptAssemblyStrategy,
    message::{ContentBlock, Message},
    ContextPipeline, TokenBudget,
};

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

fn pipeline() -> MinimalContextPipeline {
    MinimalContextPipeline::new("pipeline-v1")
        .with_workspace_md("workspace rules")
        .with_memory_md("stable memory")
}

#[test]
fn dynamic_strategy_keeps_cache_metadata_empty() {
    let packet = pipeline().build_packet(&[], &budget(400));

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
    let packet = pipeline()
        .with_prompt_assembly_strategy(PromptAssemblyStrategy::Snapshot)
        .build_packet(
            &[Message::User {
                content: vec![ContentBlock::Text {
                    text: "hello world".to_string(),
                }],
                metadata: None,
            }],
            &budget(400),
        );

    let plan = packet.cache_plan.as_ref().expect("cache plan");

    assert_eq!(
        packet.prompt_assembly_strategy,
        PromptAssemblyStrategy::Snapshot
    );
    assert_eq!(plan.strategy, PromptAssemblyStrategy::Snapshot);
    assert_eq!(packet.snapshot_hash, Some(plan.snapshot_hash.clone()));
    assert_eq!(packet.cache_prefix_hash, Some(plan.prefix_hash.clone()));
    assert_eq!(plan.prefix_message_count, 3);
    assert_eq!(packet.segments, plan.segments);
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
    let pipeline = pipeline().with_prompt_assembly_strategy(PromptAssemblyStrategy::Snapshot);

    let first = pipeline.build_packet(
        &[Message::User {
            content: vec![ContentBlock::Text {
                text: "first".to_string(),
            }],
            metadata: None,
        }],
        &budget(400),
    );

    let second = pipeline.build_packet(
        &[Message::User {
            content: vec![ContentBlock::Text {
                text: "second".to_string(),
            }],
            metadata: None,
        }],
        &budget(400),
    );

    assert_eq!(first.snapshot_hash, second.snapshot_hash);
    assert_ne!(first.packet_hash, second.packet_hash);
}

#[test]
fn budget_exhaustion_is_modelled_as_ephemeral_tail() {
    let packet = MinimalContextPipeline::new("pipeline-v1")
        .with_prompt_assembly_strategy(PromptAssemblyStrategy::Snapshot)
        .build_packet(
            &[],
            &TokenBudget {
                model_limit: 32,
                reserved_output: 24,
                used_system: 0,
                used_history: 0,
                used_sources: 0,
                used_tools: 0,
                used_memory: 0,
                minimum_turn_budget: 16,
            },
        );

    assert!(packet
        .segments
        .iter()
        .any(|segment| segment.kind == gestalt_core::context::PromptSegmentKind::Ephemeral));
}

#[test]
fn test_tool_clearing_happy_path() {
    use gestalt_context::tool_clearing::clear_eligible_tool_results;
    use serde_json::json;

    let history = vec![
        Message::User {
            content: vec![ContentBlock::Text { text: "please read the file".to_string() }],
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
            content: vec![ContentBlock::Text { text: "thanks".to_string() }],
            metadata: None,
        },
    ];

    // Total tool result tokens is large. Let's set a low tool_result_budget
    let (projected, actions) = clear_eligible_tool_results(&history, 1000, 10, 1, 100);

    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].tool_use_id, "view_1");
    assert_eq!(actions[0].tool_name, "view_file");

    if let Message::ToolResult { content, is_error, .. } = &projected[2] {
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

    let history = vec![
        Message::User {
            content: vec![ContentBlock::Text { text: "run it".to_string() }],
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
            content: vec![ContentBlock::Text { text: "try again".to_string() }],
            metadata: None,
        },
    ];

    // Even with a budget of 0, active errors are preserved
    let (projected, actions) = clear_eligible_tool_results(&history, 1000, 0, 1, 100);
    assert!(actions.is_empty());
    assert_eq!(projected, history);
}
