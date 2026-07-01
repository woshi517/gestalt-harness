use async_trait::async_trait;
use gestalt_core::session_queue::{QueueAck, QueueLifecycle, QueuedSessionMessage, SteeringQueue};
use std::collections::HashMap;
use std::sync::Mutex;

pub struct InMemorySteeringQueue {
    state: Mutex<QueueState>,
    capacity: usize,
}

pub const DEFAULT_STEERING_QUEUE_CAPACITY: usize = 64;

struct QueueState {
    lifecycle: QueueLifecycle,
    messages: Vec<QueuedSessionMessage>,
    idempotent_messages: HashMap<String, QueuedSessionMessage>,
}

impl InMemorySteeringQueue {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_STEERING_QUEUE_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                lifecycle: QueueLifecycle::Completed,
                messages: Vec::new(),
                idempotent_messages: HashMap::new(),
            }),
            capacity,
        }
    }

    pub fn active_with_capacity(capacity: usize) -> Self {
        let queue = Self::with_capacity(capacity);
        queue
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .lifecycle = QueueLifecycle::Active;
        queue
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
                    if let Some(previous) = guard.idempotent_messages.get(key) {
                        let same_payload = previous.content == message.content
                            && previous.source == message.source
                            && previous.injected_at_turn == message.injected_at_turn;
                        return Ok(if same_payload {
                            QueueAck::Duplicate
                        } else {
                            QueueAck::Conflict
                        });
                    }
                }
                if guard.messages.len() >= self.capacity {
                    return Ok(QueueAck::Full);
                }
                if let Some(ref key) = message.idempotency_key {
                    guard
                        .idempotent_messages
                        .insert(key.clone(), message.clone());
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
            guard.idempotent_messages.clear();
        }
        Ok(())
    }

    async fn len(&self) -> Result<usize, gestalt_core::error::HarnessError> {
        let guard = self.state.lock().unwrap();
        Ok(guard.messages.len())
    }
}
