use serde::{Deserialize, Serialize};
use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    User,
    Operator,
    Automation,
    FollowUp,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueuedSessionMessage {
    pub id: String,
    pub content: String,
    pub source: MessageSource,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub injected_at_turn: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueAck {
    Queued,
    Duplicate,
    SessionNotActive,
    SessionClosing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueLifecycle {
    Active,
    Closing,
    Completed,
}

#[async_trait]
pub trait SteeringQueue: Send + Sync {
    /// Enqueue a message.
    async fn enqueue(&self, message: QueuedSessionMessage) -> Result<QueueAck, crate::error::HarnessError>;

    /// Drain all messages currently in the queue, returning them in FIFO order.
    async fn drain(&self) -> Result<Vec<QueuedSessionMessage>, crate::error::HarnessError>;

    /// Update the lifecycle state of the queue.
    async fn update_lifecycle(&self, state: QueueLifecycle) -> Result<(), crate::error::HarnessError>;

    /// Get current queue length.
    async fn len(&self) -> Result<usize, crate::error::HarnessError>;

    /// Check if empty.
    async fn is_empty(&self) -> Result<bool, crate::error::HarnessError> {
        self.len().await.map(|l| l == 0)
    }
}
