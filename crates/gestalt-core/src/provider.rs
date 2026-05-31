use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{error::HarnessError, event::AgentEvent, message::Message, tool::ToolSchema};

pub type EventStream = Pin<Box<dyn Stream<Item = Result<AgentEvent, HarnessError>> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn default_model(&self) -> &str;
    fn capabilities(&self) -> &ProviderCapabilities;
    fn count_tokens(&self, messages: &[Message]) -> usize;

    async fn stream(&self, request: ProviderRequest) -> Result<EventStream, HarnessError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub supports_tools: bool,
    pub supports_parallel_tools: bool,
    pub supports_vision: bool,
    pub supports_documents: bool,
    pub supports_thinking: bool,
    pub supports_json_schema_tools: bool,
    pub supports_prompt_caching: bool,
    pub max_context_tokens: usize,
    pub max_output_tokens: usize,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            supports_tools: true,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: true,
            supports_thinking: true,
            supports_json_schema_tools: true,
            supports_prompt_caching: false,
            max_context_tokens: 0,
            max_output_tokens: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSchema>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stop_sequences: Vec<String>,
    pub metadata: Value,
}

impl Default for ProviderRequest {
    fn default() -> Self {
        Self {
            model: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            max_tokens: 4096,
            temperature: None,
            top_p: None,
            stop_sequences: Vec::new(),
            metadata: Value::Null,
        }
    }
}
