use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use gestalt_core::{
    AgentEvent, ContentBlock, HarnessError, Message, Provider, ProviderCapabilities, ProviderError,
    ProviderRequest, StopReason,
};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{json, Map, Value};

use crate::{
    auth::{
        provider_auth_config, CredentialResolver, EnvironmentCredentialResolver, ProviderAuthConfig,
    },
    catalog::ModelCatalog,
    sse,
};

#[derive(Clone)]
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    default_model: String,
    auth: ProviderAuthConfig,
    resolver: Arc<dyn CredentialResolver>,
    capabilities: ProviderCapabilities,
    headers: HashMap<String, String>,
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: "https://api.anthropic.com".to_string(),
            default_model: "claude-3-5-sonnet-20241022".to_string(),
            auth: ProviderAuthConfig {
                provider_id: "anthropic".to_string(),
                api_key_env: "ANTHROPIC_API_KEY".to_string(),
                auth_ref: None,
            },
            resolver: Arc::new(EnvironmentCredentialResolver),
            capabilities: ProviderCapabilities {
                supports_parallel_tools: true,
                supports_vision: true,
                supports_prompt_caching: true,
                supports_usage_reporting: true,
                supports_streaming: true,
                ..ProviderCapabilities::default()
            },
            headers: HashMap::new(),
        }
    }
}

impl AnthropicProvider {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "registry factories pass serde_json::Value by value"
    )]
    pub fn new(config: Value) -> Result<Self, HarnessError> {
        Self::new_with_resolver(&config, Arc::new(EnvironmentCredentialResolver))
    }

    pub fn new_with_resolver(
        config: &Value,
        resolver: Arc<dyn CredentialResolver>,
    ) -> Result<Self, HarnessError> {
        let base_url = config
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or("https://api.anthropic.com")
            .to_string();
        let default_model = config
            .get("default_model")
            .and_then(Value::as_str)
            .unwrap_or("claude-3-5-sonnet-20241022")
            .to_string();
        let auth = provider_auth_config(config, "anthropic", "ANTHROPIC_API_KEY")?;
        let headers = config
            .get("headers")
            .and_then(|h| serde_json::from_value::<HashMap<String, String>>(h.clone()).ok())
            .unwrap_or_default();

        Ok(Self {
            client: reqwest::Client::new(),
            base_url,
            default_model,
            auth,
            resolver,
            capabilities: ProviderCapabilities {
                supports_parallel_tools: true,
                supports_vision: true,
                supports_prompt_caching: true,
                supports_usage_reporting: true,
                supports_streaming: true,
                ..ProviderCapabilities::default()
            },
            headers,
        })
    }

    #[must_use]
    pub const fn auth_config(&self) -> &ProviderAuthConfig {
        &self.auth
    }

    pub fn normalize_sse(input: &str) -> Vec<Result<AgentEvent, HarnessError>> {
        let mut events = Vec::new();
        let index_to_id = Mutex::new(HashMap::new());

        for (_event, data) in sse::parse_sse(input) {
            if data == "[DONE]" {
                continue;
            }

            match normalize_payload(&data, &index_to_id) {
                Ok(normalized) => events.extend(normalized.into_iter().map(Ok)),
                Err(err) => events.push(Err(err)),
            }
        }

        events
    }

    fn headers(&self) -> Result<HeaderMap, HarnessError> {
        let mut headers = HeaderMap::new();
        let has_auth = self.auth.auth_ref.is_some()
            || (!self.auth.api_key_env.is_empty() && self.auth.api_key_env != "none");
        if has_auth {
            let credential = self.resolver.resolve(&self.auth)?;
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(credential.secret()).map_err(invalid)?,
            );
        }
        headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        for (k, v) in &self.headers {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes()).map_err(invalid)?;
            let value = HeaderValue::from_str(v).map_err(invalid)?;
            headers.insert(name, value);
        }
        Ok(headers)
    }

    fn body(&self, request: &ProviderRequest) -> Value {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        let mut body = Map::new();
        body.insert("model".to_string(), json!(model));
        body.insert("max_tokens".to_string(), json!(request.max_tokens));
        body.insert("stream".to_string(), Value::Bool(true));
        if let Some(cache_plan) = request.cache_plan.as_ref() {
            let prefix_count = cache_plan.prefix_message_count.min(request.messages.len());
            let (system, messages) = split_anthropic_messages_with_cache(&request.messages, prefix_count);

            if !system.is_empty() {
                body.insert("system".to_string(), Value::Array(system));
            }
            body.insert("messages".to_string(), Value::Array(messages));
        } else {
            let (system, messages) = split_anthropic_messages(&request.messages);
            body.insert("messages".to_string(), Value::Array(messages));

            if !system.is_empty() {
                body.insert("system".to_string(), Value::String(system));
            }
        }
        if !request.tools.is_empty() {
            let tools_val = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema
                    })
                })
                .collect::<Vec<_>>();
            body.insert("tools".to_string(), json!(tools_val));
        }
        if let Some(temperature) = request.temperature {
            body.insert("temperature".to_string(), json!(temperature));
        }
        if !request.stop_sequences.is_empty() {
            body.insert("stop_sequences".to_string(), json!(request.stop_sequences));
        }

        Value::Object(body)
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn adapt_tools(
        &self,
        tools: &[gestalt_core::tool_descriptor::ToolDescriptor],
    ) -> (
        Vec<gestalt_core::provider::ProviderToolSchema>,
        Vec<gestalt_core::tool_name_mapping::ToolNameMapping>,
    ) {
        crate::tool_schema_adapter::ToolSchemaAdapter::adapt_batch(tools, self.capabilities())
    }

    fn display_name(&self) -> &str {
        "Anthropic"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    fn model_info(&self, model: &str) -> Option<gestalt_core::ModelInfo> {
        let catalog = ModelCatalog::built_in();
        catalog
            .get_provider_model(self.id(), model)
            .or_else(|| catalog.get(model))
    }

    fn count_tokens(&self, _model: &str, messages: &[Message]) -> Result<usize, HarnessError> {
        Ok(ModelCatalog::count_tokens(messages))
    }

    async fn stream(
        &self,
        request: ProviderRequest,
    ) -> Result<gestalt_core::EventStream, HarnessError> {
        use eventsource_stream::Eventsource;
        use futures::StreamExt;

        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .headers(self.headers()?)
            .json(&self.body(&request))
            .send()
            .await
            .map_err(|err| ProviderError::Transport(std::io::Error::other(err)))?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .map_err(|err| ProviderError::Transport(std::io::Error::other(err)))?;
            return Err(HarnessError::Provider(sse::provider_error_for_status(
                status, &body,
            )));
        }

        let index_to_id = Arc::new(Mutex::new(HashMap::new()));

        let stream = response
            .bytes_stream()
            .map(|result| result.map_err(std::io::Error::other))
            .eventsource()
            .map(move |event| match event {
                Ok(event) if event.data == "[DONE]" => Ok(Vec::new()),
                Ok(event) => normalize_payload(&event.data, index_to_id.as_ref()),
                Err(err) => Err(HarnessError::Provider(ProviderError::Transport(
                    std::io::Error::other(err),
                ))),
            })
            .map(
                |result| -> futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>> {
                    match result {
                        Ok(events) => Box::pin(futures::stream::iter(events.into_iter().map(Ok))),
                        Err(err) => Box::pin(futures::stream::iter(vec![Err(err)])),
                    }
                },
            )
            .flatten();

        Ok(Box::pin(stream))
    }
}

fn normalize_payload(
    data: &str,
    index_to_id: &Mutex<HashMap<u64, String>>,
) -> Result<Vec<AgentEvent>, HarnessError> {
    let value = sse::json(data)?;
    let mut events = Vec::new();

    match value.get("type").and_then(Value::as_str) {
        Some("content_block_start") => {
            let index = value.get("index").and_then(Value::as_u64);
            let block = value.get("content_block").unwrap_or(&Value::Null);

            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let (Some(idx), false) = (index, id.is_empty()) {
                    let mut map = index_to_id.lock().map_err(|_| poisoned())?;
                    map.insert(idx, id.clone());
                }

                events.push(AgentEvent::ToolCallStreamed {
                    id,
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    input_delta: String::new(),
                });
            }
        }
        Some("content_block_delta") => {
            let delta = value.get("delta").unwrap_or(&Value::Null);
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => events.push(AgentEvent::Text {
                    delta: delta
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }),
                Some("thinking_delta") => events.push(AgentEvent::Thinking {
                    delta: delta
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }),
                Some("input_json_delta") => {
                    let index = value.get("index").and_then(Value::as_u64);
                    let id = if let Some(index) = index {
                        let map = index_to_id.lock().map_err(|_| poisoned())?;
                        map.get(&index).cloned().unwrap_or_default()
                    } else {
                        String::new()
                    };

                    events.push(AgentEvent::ToolCallStreamed {
                        id,
                        name: String::new(),
                        input_delta: delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
                _ => {}
            }
        }
        Some("message_start" | "message_delta") => {
            if let Some(usage) = value.get("usage") {
                events.push(AgentEvent::Usage {
                    input_tokens: u64_to_usize(
                        usage
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    ),
                    output_tokens: u64_to_usize(
                        usage
                            .get("output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    ),
                });
            }

            if let Some(delta) = value.get("delta") {
                let reason = sse::stop_reason(delta.get("stop_reason").and_then(Value::as_str));
                if !matches!(reason, StopReason::EndTurn) {
                    events.push(AgentEvent::Stop { reason });
                }
            }
        }
        Some("message_stop") => events.push(AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }),
        Some("error") => return Err(HarnessError::Provider(map_error(&value))),
        _ => {}
    }

    Ok(events)
}

fn map_error(value: &Value) -> ProviderError {
    let error = value.get("error").unwrap_or(value);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("provider error");
    let lowered = message.to_ascii_lowercase();
    let sanitized = sse::sanitize_detail(message);

    match error.get("type").and_then(Value::as_str) {
        Some("authentication_error") => ProviderError::AuthFailed {
            provider: "anthropic".to_string(),
        },
        Some("rate_limit_error") => ProviderError::RateLimit {
            retry_after_secs: Some(1),
        },
        Some("invalid_request_error") if lowered.contains("model") => {
            ProviderError::InvalidModel { model: sanitized }
        }
        Some("invalid_request_error") if lowered.contains("context") => {
            ProviderError::ContextTooLong {
                tokens: 0,
                limit: 0,
            }
        }
        _ => ProviderError::UnexpectedResponse { details: sanitized },
    }
}

fn split_anthropic_messages(messages: &[Message]) -> (String, Vec<Value>) {
    let mut system = Vec::new();
    let mut output = Vec::new();

    for message in messages {
        match message {
            Message::System { content } => system.push(content.clone()),
            other => output.push(message_to_anthropic_message(other, false)),
        }
    }

    (system.join("\n\n"), output)
}

fn split_anthropic_messages_with_cache(
    messages: &[Message],
    prefix_message_count: usize,
) -> (Vec<Value>, Vec<Value>) {
    let mut system = Vec::new();
    let mut output = Vec::new();

    for (index, message) in messages.iter().enumerate() {
        if index < prefix_message_count {
            let cache_last = index + 1 == prefix_message_count;
            system.push(message_to_system_block(message, cache_last));
        } else {
            output.push(message_to_anthropic_message(message, false));
        }
    }

    (system, output)
}

fn message_to_anthropic_message(message: &Message, allow_system_role: bool) -> Value {
    match message {
        Message::System { content } if allow_system_role => json!({
            "role": "system",
            "content": blocks_from_text(content)
        }),
        Message::System { content } => json!({
            "role": "user",
            "content": blocks_from_text(content)
        }),
        Message::User { content } => json!({
            "role": "user",
            "content": blocks(content)
        }),
        Message::Assistant { content } => json!({
            "role": "assistant",
            "content": blocks(content)
        }),
        Message::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => json!({
            "role": "user",
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error
            }]
        }),
    }
}

fn message_to_system_block(message: &Message, cache_last: bool) -> Value {
    let mut block = match message {
        Message::System { content } => json!({
            "type": "text",
            "text": content
        }),
        other => json!({
            "type": "text",
            "text": serde_json::to_string(other).unwrap_or_default()
        }),
    };

    if cache_last {
        if let Some(map) = block.as_object_mut() {
            map.insert("cache_control".to_string(), json!({"type": "ephemeral"}));
        }
    }

    block
}

fn blocks_from_text(text: &str) -> Vec<Value> {
    vec![json!({"type": "text", "text": text})]
}

fn blocks(blocks: &[ContentBlock]) -> Vec<Value> {
    blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => json!({"type": "text", "text": text}),
            ContentBlock::Thinking { thinking } => {
                json!({"type": "thinking", "thinking": thinking})
            }
            ContentBlock::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            other => json!({
                "type": "text",
                "text": serde_json::to_string(other).unwrap_or_default()
            }),
        })
        .collect()
}

fn invalid(err: impl std::fmt::Display) -> HarnessError {
    HarnessError::Provider(ProviderError::UnexpectedResponse {
        details: sse::sanitize_detail(&err.to_string()),
    })
}

fn poisoned() -> HarnessError {
    HarnessError::Provider(ProviderError::UnexpectedResponse {
        details: "provider stream state poisoned".to_string(),
    })
}

fn u64_to_usize(value: u64) -> usize {
    usize::try_from(value).map_or(usize::MAX, |value| value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_core::context::{
        ContextStability, PromptAssemblyStrategy, PromptCachePlan, PromptSegment,
        PromptSegmentKind, PromptSnapshot,
    };

    fn request_with_cache_plan() -> ProviderRequest {
        let snapshot = PromptSnapshot::new(
            vec![Message::System {
                content: "stable prefix".to_string(),
            }],
            0,
        );

        let plan = PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot)
            .with_segments(vec![PromptSegment::from_messages(
                PromptSegmentKind::Snapshot,
                ContextStability::SessionStatic,
                &snapshot.messages,
            )]);

        ProviderRequest {
            model: "claude-3-5-sonnet-20241022".to_string(),
            messages: vec![
                Message::System {
                    content: "stable prefix".to_string(),
                },
                Message::User {
                    content: vec![ContentBlock::Text {
                        text: "hello".to_string(),
                    }],
                },
            ],
            tools: vec![],
            tool_name_map: vec![],
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            stop_sequences: vec![],
            cache_plan: Some(plan),
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn body_uses_cached_system_prefix_when_cache_plan_is_present() {
        let provider = AnthropicProvider::default();
        let body = provider.body(&request_with_cache_plan());

        assert!(body.get("system").is_some());
        assert!(body.get("cache_control").is_none());

        let system = body.get("system").and_then(Value::as_array).unwrap();
        assert_eq!(system.len(), 1);
        assert!(system[0].get("cache_control").and_then(Value::as_object).is_some());
        assert_eq!(system[0].get("text").and_then(Value::as_str), Some("stable prefix"));
    }

    #[test]
    fn body_preserves_current_shape_without_cache_plan() {
        let provider = AnthropicProvider::default();
        let request = ProviderRequest {
            cache_plan: None,
            ..request_with_cache_plan()
        };

        let body = provider.body(&request);
        assert!(body.get("system").map(Value::is_string).unwrap_or(false));
    }

    #[test]
    fn body_serializes_tail_system_messages_as_user_messages_when_cached() {
        let provider = AnthropicProvider::default();
        let mut request = request_with_cache_plan();
        request.messages.push(Message::System {
            content: "context budget exhausted or truncated; keep working with the available context"
                .to_string(),
        });

        let body = provider.body(&request);
        let messages = body.get("messages").and_then(Value::as_array).unwrap();
        let last_role = messages
            .last()
            .and_then(|message| message.get("role"))
            .and_then(Value::as_str);

        assert_eq!(last_role, Some("user"));
    }
}
