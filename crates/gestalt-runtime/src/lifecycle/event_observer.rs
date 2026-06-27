use async_trait::async_trait;
use gestalt_core::event::AgentEvent;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EventObserverRequest {
    pub session_id: String,
    pub event: AgentEvent,
}

#[async_trait]
pub trait EventObserver: Send + Sync {
    async fn observe_event(&self, request: EventObserverRequest) -> crate::Result<()>;
}
