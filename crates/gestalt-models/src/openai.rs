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
use serde_json::{json, Value};

use crate::{
    auth::{
        provider_auth_config, CredentialResolver, EnvironmentCredentialResolver, ProviderAuthConfig,
    },
    catalog::ModelCatalog,
    sse,
};

#[derive(Clone)]
pub struct OpenAiProvider {
    client: reqwest::Client,
    id: String,
    display_name: String,
    base_url: String,
    default_model: String,
    auth: ProviderAuthConfig,
    resolver: Arc<dyn CredentialResolver>,
    capabilities: ProviderCapabilities,
    headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Default)]
struct ToolCallState {
    id: String,
    name: String,
}

impl Default for OpenAiProvider {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            default_model: "gpt-4o-mini".to_string(),
            auth: ProviderAuthConfig {
                provider_id: "openai".to_string(),
                api_key_env: "OPENAI_API_KEY".to_string(),
                auth_ref: None,
            },
            resolver: Arc::new(EnvironmentCredentialResolver),
            capabilities: ProviderCapabilities {
                supports_parallel_tools: true,
                supports_vision: true,
                supports_thinking: false,
                supports_usage_reporting: true,
                supports_streaming: true,
                supports_strict_schema: true,
                ..ProviderCapabilities::default()
            },
            headers: HashMap::new(),
        }
    }
}

impl OpenAiProvider {
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
        let id = config
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("openai")
            .to_string();
        let display_name = config
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or("OpenAI")
            .to_string();
        let base_url = config
            .get("base_url")
            .and_then(Value::as_str)
            .unwrap_or("https://api.openai.com/v1")
            .to_string();
        let default_model = config
            .get("default_model")
            .and_then(Value::as_str)
            .unwrap_or("gpt-4o-mini")
            .to_string();
        let default_env = if id == "openai-compatible" {
            "OPENAI_COMPATIBLE_API_KEY"
        } else {
            "OPENAI_API_KEY"
        };
        let auth = provider_auth_config(config, &id, default_env)?;
        let headers = config
            .get("headers")
            .and_then(|h| serde_json::from_value::<HashMap<String, String>>(h.clone()).ok())
            .unwrap_or_default();

        let mut capabilities = ProviderCapabilities {
            supports_parallel_tools: true,
            supports_vision: true,
            supports_thinking: false,
            supports_usage_reporting: true,
            supports_streaming: true,
            supports_strict_schema: true,
            ..ProviderCapabilities::default()
        };
        if let Some(caps) = config.get("capabilities") {
            if let Ok(c) = serde_json::from_value::<ProviderCapabilities>(caps.clone()) {
                capabilities = c;
            }
        }

        Ok(Self {
            client: reqwest::Client::new(),
            id,
            display_name,
            base_url,
            default_model,
            auth,
            resolver,
            capabilities,
            headers,
        })
    }

    #[must_use]
    pub const fn auth_config(&self) -> &ProviderAuthConfig {
        &self.auth
    }

    pub fn normalize_sse(input: &str) -> Vec<Result<AgentEvent, HarnessError>> {
        let mut events = Vec::new();
        let state = Mutex::new(HashMap::new());

        for (_event, data) in sse::parse_sse(input) {
            if data == "[DONE]" {
                continue;
            }

            match normalize_payload(&data, &state) {
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
                "authorization",
                HeaderValue::from_str(&format!("Bearer {}", credential.secret()))
                    .map_err(invalid)?,
            );
        }
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        for (k, v) in &self.headers {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes()).map_err(invalid)?;
            let value = HeaderValue::from_str(v).map_err(invalid)?;
            headers.insert(name, value);
        }
        Ok(headers)
    }

    fn body(&self, request: &ProviderRequest) -> Value {
        let messages = split_openai_messages(&request.messages);
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": true,
            "stream_options": {
                "include_usage": true
            },
            "max_tokens": request.max_tokens
        });
        if !request.tools.is_empty() {
            body["tools"] = convert_tools(&request.tools);
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(top_p) = request.top_p {
            body["top_p"] = json!(top_p);
        }
        if !request.stop_sequences.is_empty() {
            body["stop"] = json!(request.stop_sequences);
        }

        let is_openai_compatible = self.id == "openai-compatible";
        let supports_reasoning = !is_openai_compatible || self.capabilities.supports_thinking;

        if supports_reasoning {
            if let Some(effort) = request.reasoning_effort {
                let effort_str = match effort {
                    gestalt_core::provider::ReasoningEffort::None => "none",
                    gestalt_core::provider::ReasoningEffort::Low => "low",
                    gestalt_core::provider::ReasoningEffort::Medium => "medium",
                    gestalt_core::provider::ReasoningEffort::High => "high",
                    gestalt_core::provider::ReasoningEffort::Xhigh => "xhigh",
                };
                body["reasoning"] = json!({ "effort": effort_str });
            }
            if let Some(verbosity) = request.text_verbosity {
                let verbosity_str = match verbosity {
                    gestalt_core::provider::TextVerbosity::None => "none",
                    gestalt_core::provider::TextVerbosity::Low => "low",
                    gestalt_core::provider::TextVerbosity::Medium => "medium",
                    gestalt_core::provider::TextVerbosity::High => "high",
                };
                body["text"] = json!({ "verbosity": verbosity_str });
            }
        } else {
            if request.reasoning_effort.is_some() {
                eprintln!("Warning: reasoning_effort ignored because the provider '{}' does not support thinking/reasoning capability", self.id);
            }
            if request.text_verbosity.is_some() {
                eprintln!("Warning: text_verbosity ignored because the provider '{}' does not support verbosity capability", self.id);
            }
        }
        body
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
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
        &self.display_name
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

    fn count_request_tokens(&self, request: &ProviderRequest) -> Result<usize, HarnessError> {
        let message_tokens = ModelCatalog::count_tokens(&request.messages);
        let body = self.body(request);
        let serialized = serde_json::to_string(&body).unwrap_or_default();
        Ok(message_tokens.max(serialized.len() / 4))
    }

    async fn stream(
        &self,
        request: ProviderRequest,
    ) -> Result<gestalt_core::EventStream, HarnessError> {
        use eventsource_stream::Eventsource;
        use futures::StreamExt;

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
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
            if let Ok(value) = serde_json::from_str::<Value>(&body) {
                return Err(HarnessError::Provider(map_error(&value, &self.id)));
            }
            return Err(HarnessError::Provider(sse::provider_error_for_status(
                status, &body,
            )));
        }

        let state = Arc::new(Mutex::new(HashMap::new()));

        let stream = response
            .bytes_stream()
            .map(|result| result.map_err(std::io::Error::other))
            .eventsource()
            .map(move |event| match event {
                Ok(event) if event.data == "[DONE]" => Ok(Vec::new()),
                Ok(event) => normalize_payload(&event.data, state.as_ref()),
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
    state: &Mutex<HashMap<u64, ToolCallState>>,
) -> Result<Vec<AgentEvent>, HarnessError> {
    let value = sse::json(data)?;
    let mut events = Vec::new();

    if let Some(usage) = value.get("usage") {
        let input_tokens = u64_to_usize(
            usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        let output_tokens = u64_to_usize(
            usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        if input_tokens > 0 || output_tokens > 0 {
            events.push(AgentEvent::Usage {
                input_tokens,
                output_tokens,
            });
        }
    }

    if let Some(choices) = value.get("choices").and_then(Value::as_array) {
        for choice in choices {
            if let Some(delta) = choice.get("delta") {
                if let Some(content) = delta.get("content").and_then(Value::as_str) {
                    if !content.is_empty() {
                        events.push(AgentEvent::Text {
                            delta: content.to_string(),
                        });
                    }
                }

                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tool_call in tool_calls {
                        let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let (id, name) = {
                            let mut map = state.lock().map_err(|_| poisoned())?;
                            let entry = map.entry(index).or_default();

                            if let Some(id) = tool_call.get("id").and_then(Value::as_str) {
                                entry.id = id.to_string();
                            }
                            if let Some(name) = tool_call
                                .get("function")
                                .and_then(|function| function.get("name"))
                                .and_then(Value::as_str)
                            {
                                entry.name = name.to_string();
                            }

                            let result = (entry.id.clone(), entry.name.clone());
                            drop(map);
                            result
                        };

                        events.push(AgentEvent::ToolCallStreamed {
                            id,
                            name,
                            input_delta: tool_call
                                .get("function")
                                .and_then(|function| function.get("arguments"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        });
                    }
                }
            }

            if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                events.push(AgentEvent::Stop {
                    reason: match finish_reason {
                        "tool_calls" | "function_call" => StopReason::ToolUse,
                        "length" => StopReason::MaxOutput,
                        "content_filter" => StopReason::ContentFiltered,
                        _ => StopReason::EndTurn,
                    },
                });
            }
        }
    }

    Ok(events)
}

fn map_error(value: &Value, provider: &str) -> ProviderError {
    let error = value.get("error").unwrap_or(value);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("provider error");
    let lowered = message.to_ascii_lowercase();
    let sanitized = sse::sanitize_detail(message);

    if matches!(
        error.get("code").and_then(Value::as_str),
        Some("invalid_api_key")
    ) {
        return ProviderError::AuthFailed {
            provider: provider.to_string(),
        };
    }

    match error.get("type").and_then(Value::as_str) {
        Some("insufficient_quota") => ProviderError::RateLimit {
            retry_after_secs: None,
        },
        Some("invalid_request_error") if lowered.contains("context") => {
            ProviderError::ContextTooLong {
                tokens: 0,
                limit: 0,
            }
        }
        Some("invalid_request_error") if lowered.contains("model") => {
            ProviderError::InvalidModel { model: sanitized }
        }
        _ if lowered.contains("timed out") => ProviderError::Timeout,
        _ => ProviderError::UnexpectedResponse { details: sanitized },
    }
}

fn split_openai_messages(messages: &[Message]) -> Vec<Value> {
    let mut output = Vec::new();
    for message in messages {
        match message {
            Message::System { content } => {
                output.push(json!({"role": "system", "content": content}));
            }
            Message::User { content, .. } => {
                output.push(convert_user_message(content));
            }
            Message::Assistant { content } => {
                output.push(convert_assistant_message(content));
            }
            Message::ToolResult {
                tool_use_id,
                content,
                is_error: _,
                ..
            } => {
                output.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content
                }));
            }
        }
    }
    output
}

fn convert_user_message(content: &[ContentBlock]) -> Value {
    let mut parts = Vec::new();
    for block in content {
        match block {
            ContentBlock::Text { text } => {
                parts.push(json!({"type": "text", "text": text}));
            }
            ContentBlock::Image { source } => {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{};base64,{}", source.media_type, source.data)
                    }
                }));
            }
            _ => {}
        }
    }
    json!({
        "role": "user",
        "content": parts
    })
}

fn convert_assistant_message(content: &[ContentBlock]) -> Value {
    let mut text_parts = Vec::new();
    let mut tool_calls = Vec::new();

    for block in content {
        match block {
            ContentBlock::Text { text } => text_parts.push(text.clone()),
            ContentBlock::Thinking { thinking } => {
                text_parts.push(format!("<thinking>\n{thinking}\n</thinking>"));
            }
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": input.to_string()
                    }
                }));
            }
            _ => {}
        }
    }

    let content_value = if text_parts.is_empty() {
        Value::Null
    } else {
        Value::String(text_parts.join("\n\n"))
    };

    if tool_calls.is_empty() {
        json!({
            "role": "assistant",
            "content": content_value
        })
    } else {
        json!({
            "role": "assistant",
            "content": content_value,
            "tool_calls": tool_calls
        })
    }
}

fn convert_tools(tools: &[gestalt_core::provider::ProviderToolSchema]) -> Value {
    let mut output = Vec::new();
    for tool in tools {
        let mut function_obj = json!({
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema
        });
        if let Some(strict) = tool.strict {
            function_obj["strict"] = json!(strict);
        }
        output.push(json!({
            "type": "function",
            "function": function_obj
        }));
    }
    Value::Array(output)
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
        let plan =
            PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot).with_segments(vec![
                PromptSegment::from_messages(
                    PromptSegmentKind::Snapshot,
                    ContextStability::SessionStatic,
                    &snapshot.messages,
                ),
            ]);

        ProviderRequest {
            model: "gpt-4o-mini".to_string(),
            messages: vec![Message::User {
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                metadata: None,
            }],
            tools: vec![],
            tool_name_map: vec![],
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            stop_sequences: vec![],
            cache_plan: Some(plan),
            metadata: serde_json::Value::Null,
            reasoning_effort: None,
            text_verbosity: None,
        }
    }

    #[test]
    fn body_ignores_cache_plan_metadata() {
        let provider = OpenAiProvider::default();
        let body = provider.body(&request_with_cache_plan());

        assert!(body.get("cache_control").is_none());
        assert!(body.get("messages").is_some());
        assert!(body.get("system").is_none());
    }

    #[test]
    fn body_preserves_assistant_tool_call_and_tombstoned_tool_result_order() {
        let provider = OpenAiProvider::default();
        let request = ProviderRequest {
            model: "gpt-4o-mini".to_string(),
            messages: vec![
                Message::Assistant {
                    content: vec![ContentBlock::ToolUse {
                        id: "tool-1".to_string(),
                        name: "view_file".to_string(),
                        input: serde_json::json!({"path": "src/lib.rs"}),
                    }],
                },
                Message::ToolResult {
                    tool_use_id: "tool-1".to_string(),
                    content:
                        "<tombstone tool_use_id=\"tool-1\" tool_name=\"view_file\" output_hash=\"abc\" />"
                            .to_string(),
                    is_error: false,
                    failure: None,
                    tool_name: Some("view_file".to_string()),
                    output_hash: Some("abc".to_string()),
                    artifact_refs: None,
                },
            ],
            tools: vec![],
            tool_name_map: vec![],
            max_tokens: 512,
            temperature: None,
            top_p: None,
            stop_sequences: vec![],
            cache_plan: None,
            metadata: serde_json::Value::Null,
            reasoning_effort: None,
            text_verbosity: None,
        };

        let body = provider.body(&request);
        let messages = body.get("messages").and_then(Value::as_array).unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0].get("role").and_then(Value::as_str),
            Some("assistant")
        );
        assert_eq!(
            messages[1].get("role").and_then(Value::as_str),
            Some("tool")
        );
        assert_eq!(
            messages[1].get("tool_call_id").and_then(Value::as_str),
            Some("tool-1")
        );
    }
}
