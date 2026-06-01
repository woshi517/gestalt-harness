use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::HarnessError, event::AgentEvent, message::Message, model::ModelInfo, tool::ToolSchema,
};

pub type EventStream = Pin<Box<dyn Stream<Item = Result<AgentEvent, HarnessError>> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    /// Stable provider identifier, e.g. "anthropic", "openai", "ollama".
    fn id(&self) -> &str;

    /// Human-readable provider name, e.g. "Anthropic".
    fn display_name(&self) -> &str;

    /// Default model used when no model is specified by config or CLI.
    fn default_model(&self) -> &str;

    /// Provider-level capabilities.
    fn capabilities(&self) -> &ProviderCapabilities;

    /// Return known metadata for a model.
    fn model_info(&self, model: &str) -> Option<ModelInfo>;

    /// Count tokens for a fully assembled message list.
    fn count_tokens(&self, model: &str, messages: &[Message]) -> Result<usize, HarnessError>;

    /// Stream a normalized event sequence for one model request.
    async fn stream(&self, request: ProviderRequest) -> Result<EventStream, HarnessError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub supports_tools: bool,
    pub supports_parallel_tools: bool,
    pub supports_vision: bool,
    pub supports_documents: bool,
    pub supports_thinking: bool,
    pub supports_json_schema_tools: bool,
    pub supports_prompt_caching: bool,
    pub supports_usage_reporting: bool,
    pub supports_streaming: bool,
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
            supports_usage_reporting: true,
            supports_streaming: true,
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
