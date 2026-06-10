use async_trait::async_trait;
use gestalt_core::session_queue::{QueueAck, QueueLifecycle, QueuedSessionMessage, SteeringQueue};
use std::collections::HashSet;
use std::sync::Mutex;

pub struct InMemorySteeringQueue {
    state: Mutex<QueueState>,
}

struct QueueState {
    lifecycle: QueueLifecycle,
    messages: Vec<QueuedSessionMessage>,
    seen_idempotency_keys: HashSet<String>,
}

impl InMemorySteeringQueue {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                lifecycle: QueueLifecycle::Completed,
                messages: Vec::new(),
                seen_idempotency_keys: HashSet::new(),
            }),
        }
    }
}

impl Default for InMemorySteeringQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SteeringQueue for InMemorySteeringQueue {
    async fn enqueue(
        &self,
        message: QueuedSessionMessage,
    ) -> Result<QueueAck, gestalt_core::error::HarnessError> {
        let mut guard = self.state.lock().unwrap();
        match guard.lifecycle {
            QueueLifecycle::Completed => Ok(QueueAck::SessionNotActive),
            QueueLifecycle::Closing => Ok(QueueAck::SessionClosing),
            QueueLifecycle::Active => {
                if let Some(ref key) = message.idempotency_key {
                    if guard.seen_idempotency_keys.contains(key) {
                        return Ok(QueueAck::Duplicate);
                    }
                    guard.seen_idempotency_keys.insert(key.clone());
                }
                guard.messages.push(message);
                Ok(QueueAck::Queued)
            }
        }
    }

    async fn drain(&self) -> Result<Vec<QueuedSessionMessage>, gestalt_core::error::HarnessError> {
        let mut guard = self.state.lock().unwrap();
        let messages = std::mem::take(&mut guard.messages);
        Ok(messages)
    }

    async fn update_lifecycle(
        &self,
        state: QueueLifecycle,
    ) -> Result<(), gestalt_core::error::HarnessError> {
        let mut guard = self.state.lock().unwrap();
        guard.lifecycle = state;
        if state == QueueLifecycle::Completed {
            guard.messages.clear();
        }
        Ok(())
    }

    async fn len(&self) -> Result<usize, gestalt_core::error::HarnessError> {
        let guard = self.state.lock().unwrap();
        Ok(guard.messages.len())
    }
}
