use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    context::PromptCachePlan, error::HarnessError, event::AgentEvent, message::Message,
    model::ModelInfo, tool::ToolSchema,
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

    /// Adapt gestalt descriptors to provider-specific schema and mappings.
    fn adapt_tools(
        &self,
        tools: &[crate::tool_descriptor::ToolDescriptor],
    ) -> (
        Vec<ProviderToolSchema>,
        Vec<crate::tool_name_mapping::ToolNameMapping>,
    ) {
        use crate::tool_descriptor::CanonicalToolId;
        use crate::tool_name_mapping::ToolNameMapping;
        use sha2::{Digest, Sha256};

        // Sort descriptors by canonical internal ID so collision
        // resolution is deterministic regardless of the order the
        // catalog returned them in.
        let mut sorted: Vec<&crate::tool_descriptor::ToolDescriptor> = tools.iter().collect();
        sorted.sort_by(|a, b| a.id.cmp(&b.id));

        let ids: Vec<CanonicalToolId> = sorted.iter().map(|t| t.id.clone()).collect();
        let resolved = ToolNameMapping::resolve_provider_names(&ids);

        let mut schemas = Vec::with_capacity(sorted.len());
        let mut mappings = Vec::with_capacity(sorted.len());

        for (tool, (canonical_id, provider_name)) in sorted.iter().zip(resolved.iter()) {
            // Sanity: the resolver should preserve the canonical IDs
            // we fed it, but guard against any future drift so we
            // never produce a mapping that doesn't line up with the
            // descriptor it claims to represent.
            debug_assert_eq!(&tool.id, canonical_id);

            let desc_json = serde_json::to_string(tool).unwrap_or_default();
            let mut hasher = Sha256::new();
            hasher.update(desc_json.as_bytes());
            let descriptor_hash = format!("{:x}", hasher.finalize());

            let input_schema = tool
                .schema
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| tool.schema.clone());

            let provider_schema = ProviderToolSchema {
                name: provider_name.clone(),
                description: tool.description.clone(),
                input_schema,
                strict: None,
            };

            let mapping = ToolNameMapping {
                internal_id: tool.id.clone(),
                provider_name: provider_name.clone(),
                display_name: tool.id.name.clone(),
                descriptor_hash,
                input_schema: None,
                strict: None,
            };

            schemas.push(provider_schema);
            mappings.push(mapping);
        }
        (schemas, mappings)
    }
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
    pub supports_strict_schema: bool,
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
            supports_strict_schema: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: ToolSchema,
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ProviderToolSchema>,
    pub tool_name_map: Vec<crate::tool_name_mapping::ToolNameMapping>,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stop_sequences: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_plan: Option<PromptCachePlan>,
    pub metadata: Value,
}

impl Default for ProviderRequest {
    fn default() -> Self {
        Self {
            model: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            tool_name_map: Vec::new(),
            max_tokens: 4096,
            temperature: None,
            top_p: None,
            stop_sequences: Vec::new(),
            cache_plan: None,
            metadata: Value::Null,
        }
    }
}
