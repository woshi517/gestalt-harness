use gestalt_core::session_queue::{
    MessageSource, QueueAck, QueueLifecycle, QueuedSessionMessage, SteeringQueue,
};
use gestalt_runtime::InMemorySteeringQueue;

#[tokio::test]
async fn test_in_memory_steering_queue_fifo_and_len() {
    let queue = InMemorySteeringQueue::new();
    queue
        .update_lifecycle(QueueLifecycle::Active)
        .await
        .unwrap();

    assert!(queue.is_empty().await.unwrap());
    assert_eq!(queue.len().await.unwrap(), 0);

    let msg1 = QueuedSessionMessage {
        id: "1".to_string(),
        content: "First".to_string(),
        source: MessageSource::User,
        idempotency_key: None,
        injected_at_turn: None,
    };
    let msg2 = QueuedSessionMessage {
        id: "2".to_string(),
        content: "Second".to_string(),
        source: MessageSource::Operator,
        idempotency_key: None,
        injected_at_turn: None,
    };

    let ack1 = queue.enqueue(msg1.clone()).await.unwrap();
    assert_eq!(ack1, QueueAck::Queued);
    assert_eq!(queue.len().await.unwrap(), 1);
    assert!(!queue.is_empty().await.unwrap());

    let ack2 = queue.enqueue(msg2.clone()).await.unwrap();
    assert_eq!(ack2, QueueAck::Queued);
    assert_eq!(queue.len().await.unwrap(), 2);

    let drained = queue.drain().await.unwrap();
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].id, "1");
    assert_eq!(drained[1].id, "2");

    assert!(queue.is_empty().await.unwrap());
    assert_eq!(queue.len().await.unwrap(), 0);
}

#[tokio::test]
async fn test_in_memory_steering_queue_idempotency() {
    let queue = InMemorySteeringQueue::new();
    queue
        .update_lifecycle(QueueLifecycle::Active)
        .await
        .unwrap();

    let msg1 = QueuedSessionMessage {
        id: "1".to_string(),
        content: "First".to_string(),
        source: MessageSource::User,
        idempotency_key: Some("key-1".to_string()),
        injected_at_turn: None,
    };
    let msg2 = QueuedSessionMessage {
        id: "2".to_string(),
        content: "First".to_string(),
        source: MessageSource::User,
        idempotency_key: Some("key-1".to_string()),
        injected_at_turn: None,
    };

    let ack1 = queue.enqueue(msg1.clone()).await.unwrap();
    assert_eq!(ack1, QueueAck::Queued);

    let ack2 = queue.enqueue(msg2.clone()).await.unwrap();
    assert_eq!(ack2, QueueAck::Duplicate);

    assert_eq!(queue.len().await.unwrap(), 1);
    let drained = queue.drain().await.unwrap();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].id, "1");
}

#[tokio::test]
async fn queue_rejects_idempotency_key_reused_with_different_payload() {
    let queue = InMemorySteeringQueue::with_capacity(1);
    queue
        .update_lifecycle(QueueLifecycle::Active)
        .await
        .unwrap();
    let first = QueuedSessionMessage {
        id: "1".to_string(),
        content: "first".to_string(),
        source: MessageSource::User,
        idempotency_key: Some("key".to_string()),
        injected_at_turn: None,
    };
    let conflicting = QueuedSessionMessage {
        id: "2".to_string(),
        content: "different".to_string(),
        source: MessageSource::User,
        idempotency_key: Some("key".to_string()),
        injected_at_turn: None,
    };

    queue.enqueue(first).await.unwrap();
    assert_eq!(
        queue.enqueue(conflicting).await.unwrap(),
        QueueAck::Conflict
    );
}

#[tokio::test]
async fn test_in_memory_steering_queue_lifecycle() {
    let queue = InMemorySteeringQueue::new();

    queue
        .update_lifecycle(QueueLifecycle::Closing)
        .await
        .unwrap();
    let msg = QueuedSessionMessage {
        id: "1".to_string(),
        content: "Test".to_string(),
        source: MessageSource::User,
        idempotency_key: None,
        injected_at_turn: None,
    };
    let ack = queue.enqueue(msg.clone()).await.unwrap();
    assert_eq!(ack, QueueAck::SessionClosing);

    queue
        .update_lifecycle(QueueLifecycle::Completed)
        .await
        .unwrap();
    let ack2 = queue.enqueue(msg).await.unwrap();
    assert_eq!(ack2, QueueAck::SessionNotActive);
}

#[tokio::test]
async fn queue_returns_full_at_capacity() {
    let queue = InMemorySteeringQueue::with_capacity(1);
    queue
        .update_lifecycle(QueueLifecycle::Active)
        .await
        .unwrap();
    let message = QueuedSessionMessage {
        id: "1".to_string(),
        content: "first".to_string(),
        source: MessageSource::User,
        idempotency_key: None,
        injected_at_turn: None,
    };

    assert_eq!(
        queue.enqueue(message.clone()).await.unwrap(),
        QueueAck::Queued
    );
    assert_eq!(queue.enqueue(message).await.unwrap(), QueueAck::Full);
}
