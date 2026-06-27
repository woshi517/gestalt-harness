use gestalt_core::session_queue::{MessageSource, QueuedSessionMessage};
use gestalt_core::{
    context::{CompactionCheckpointRef, ContextStateDelta, StateUpdate, TokenBudget},
    message::{ContentBlock, Message},
    session::{ExecutionMode, Session, SessionConfig},
    snapshot::WorkspaceSnapshot,
    tool::ToolContext,
};

#[test]
fn test_queued_session_message_serialization() {
    let msg = QueuedSessionMessage {
        id: "msg-123".to_string(),
        content: "Hello from operator".to_string(),
        source: MessageSource::Operator,
        idempotency_key: Some("idem-key-1".to_string()),
        injected_at_turn: Some(3),
    };

    let serialized = serde_json::to_string(&msg).unwrap();
    let deserialized: QueuedSessionMessage = serde_json::from_str(&serialized).unwrap();

    assert_eq!(msg, deserialized);
    assert_eq!(deserialized.source, MessageSource::Operator);
    assert_eq!(deserialized.injected_at_turn, Some(3));
}

#[test]
fn test_session_appends_messages_with_stable_ids_and_default_projection_state() {
    let mut session = Session::new(
        "session-test",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 1,
            top_p: None,
            reasoning_effort: None,
            text_verbosity: None,
            metadata: serde_json::Value::Null,
            resolved_model: None,
        },
        TokenBudget::default(),
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            workspace_root: None,
            timeout: std::time::Duration::from_secs(1),
            allow_network: false,
            environment: std::collections::HashMap::new(),
            max_output_bytes: 1024,
            artifact_dir: None,
            current_tool_call_id: None,
            ignore_patterns: Vec::new(),
        },
        ExecutionMode::Yolo,
        WorkspaceSnapshot {
            workspace_root: std::path::PathBuf::from("/tmp"),
            git_sha: None,
            git_dirty: Some(false),
            untracked_count: None,
            content_hash: "hash".to_string(),
            captured_at: chrono::Utc::now(),
        },
    );

    let user_id = session.append_message(Message::User {
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
        }],
        metadata: None,
    });
    let assistant_id = session.append_message(Message::Assistant {
        content: vec![ContentBlock::Text {
            text: "world".to_string(),
        }],
    });

    assert_eq!(session.history.len(), 2);
    assert_eq!(user_id.sequence, 0);
    assert_eq!(assistant_id.sequence, 1);
    assert_eq!(user_id.origin_session_id, session.id);
    assert_eq!(assistant_id.origin_session_id, session.id);
    assert_eq!(session.history[0].id, user_id);
    assert_eq!(session.history[1].id, assistant_id);
    assert!(session.context_state.active_checkpoint.is_none());
    assert!(session.context_state.cleared_tool_results.is_empty());
    assert!(session.context_state.prompt_snapshot.is_none());
    assert_eq!(session.context_state.context_epoch, 0);
}

#[test]
fn test_context_state_delta_distinguishes_unchanged_set_and_clear() {
    let mut session = Session::new(
        "session-test",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 1,
            top_p: None,
            reasoning_effort: None,
            text_verbosity: None,
            metadata: serde_json::Value::Null,
            resolved_model: None,
        },
        TokenBudget::default(),
        ToolContext {
            working_dir: std::path::PathBuf::from("/tmp"),
            workspace_root: None,
            timeout: std::time::Duration::from_secs(1),
            allow_network: false,
            environment: std::collections::HashMap::new(),
            max_output_bytes: 1024,
            artifact_dir: None,
            current_tool_call_id: None,
            ignore_patterns: Vec::new(),
        },
        ExecutionMode::Yolo,
        WorkspaceSnapshot {
            workspace_root: std::path::PathBuf::from("/tmp"),
            git_sha: None,
            git_dirty: Some(false),
            untracked_count: None,
            content_hash: "hash".to_string(),
            captured_at: chrono::Utc::now(),
        },
    );

    let checkpoint = CompactionCheckpointRef {
        checkpoint_id: "cp-1".to_string(),
        source_range: gestalt_core::HistoryRange::new(0, 1),
        source_hash: "hash-1".to_string(),
        artifact: None,
    };

    session.apply_context_state_delta(ContextStateDelta {
        active_checkpoint: StateUpdate::Set(checkpoint.clone()),
        ..ContextStateDelta::default()
    });
    assert_eq!(
        session.context_state.active_checkpoint,
        Some(checkpoint.clone())
    );

    session.apply_context_state_delta(ContextStateDelta::default());
    assert_eq!(
        session.context_state.active_checkpoint,
        Some(checkpoint.clone())
    );

    session.apply_context_state_delta(ContextStateDelta {
        active_checkpoint: StateUpdate::Clear,
        ..ContextStateDelta::default()
    });
    assert!(session.context_state.active_checkpoint.is_none());
}
