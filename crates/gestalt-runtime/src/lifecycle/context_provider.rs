use async_trait::async_trait;
use gestalt_core::message::Message;

#[derive(Debug, Clone, PartialEq)]
pub struct ContextProviderRequest {
    pub session_id: String,
    pub current_turn: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextProviderResponse {
    pub messages: Vec<Message>,
}

#[async_trait]
pub trait ContextProvider: Send + Sync {
    async fn provide_context(
        &self,
        request: ContextProviderRequest,
    ) -> crate::Result<ContextProviderResponse>;
}
