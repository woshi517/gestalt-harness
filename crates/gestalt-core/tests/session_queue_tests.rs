use gestalt_core::session_queue::{MessageSource, QueuedSessionMessage};

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
