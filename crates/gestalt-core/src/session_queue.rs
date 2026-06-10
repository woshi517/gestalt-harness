use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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
    /// Runtime opens the queue when a run becomes able to accept steering.
    Active,
    /// AgentLoop closes the queue at the terminal stop boundary, after the
    /// final safe pre-request drain point and before session-end hooks.
    Closing,
    /// Runtime completes the queue after `AgentLoop::run` returns and outer
    /// cleanup finishes. Pending messages may be discarded at this point.
    Completed,
}

#[async_trait]
pub trait SteeringQueue: Send + Sync {
    /// Enqueue a message.
    async fn enqueue(
        &self,
        message: QueuedSessionMessage,
    ) -> Result<QueueAck, crate::error::HarnessError>;

    /// Drain all messages currently in the queue, returning them in FIFO order.
    async fn drain(&self) -> Result<Vec<QueuedSessionMessage>, crate::error::HarnessError>;

    /// Update the lifecycle state of the queue.
    async fn update_lifecycle(
        &self,
        state: QueueLifecycle,
    ) -> Result<(), crate::error::HarnessError>;

    /// Get current queue length.
    async fn len(&self) -> Result<usize, crate::error::HarnessError>;

    /// Check if empty.
    async fn is_empty(&self) -> Result<bool, crate::error::HarnessError> {
        self.len().await.map(|l| l == 0)
    }
}
