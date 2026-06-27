use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use gestalt_core::{
    AgentEvent, ContentBlock, HarnessError, Message, Provider, ProviderCapabilities,
    ProviderRequest,
};
use serde_json::{json, Value};

use super::common::{map_error, poisoned, u64_to_usize, CompletionsTransport};
use crate::{
    auth::{
        provider_auth_config, CredentialResolver, EnvironmentCredentialResolver, ProviderAuthConfig,
    },
    catalog::ModelCatalog,
    sse,
};

#[derive(Clone)]
pub struct OpenAiResponsesProvider {
    id: String,
    display_name: String,
    default_model: String,
    request_path: Option<String>,
    _timeout_ms: Option<u64>,
    stream_chunk_timeout_ms: Option<u64>,
    transport: CompletionsTransport,
    capabilities: ProviderCapabilities,
}

#[derive(Debug, Clone, Default)]
struct ToolCallState {
    name: String,
}

impl Default for OpenAiResponsesProvider {
    fn default() -> Self {
        let auth = ProviderAuthConfig {
            provider_id: "openai".to_string(),
            credential: crate::auth::ConfiguredCredential::Environment(
                "OPENAI_API_KEY".to_string(),
            ),
        };
        let transport = CompletionsTransport::new(
            "https://api.openai.com/v1".to_string(),
            auth,
            Arc::new(EnvironmentCredentialResolver),
            HashMap::new(),
        );
        Self {
            id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            default_model: "gpt-4o-mini".to_string(),
            request_path: None,
            _timeout_ms: None,
            stream_chunk_timeout_ms: None,
            transport,
            capabilities: ProviderCapabilities {
                supports_parallel_tools: true,
                supports_vision: true,
                supports_thinking: true,
                supports_usage_reporting: true,
                supports_streaming: true,
                supports_strict_schema: true,
                ..ProviderCapabilities::default()
            },
        }
    }
}

impl OpenAiResponsesProvider {
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
        let default_env = if id == "openai-compatible" {
            "OPENAI_COMPATIBLE_API_KEY"
        } else {
            "OPENAI_API_KEY"
        };
        let auth = provider_auth_config(config, &id, default_env)?;
        Self::new_with_auth_and_resolver(config, auth, resolver)
    }

    pub fn new_with_auth_and_resolver(
        config: &Value,
        auth: ProviderAuthConfig,
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
        let request_path = config
            .get("request_path")
            .and_then(Value::as_str)
            .map(String::from);

        let request_config = config.get("request");
        let timeout_ms = request_config
            .and_then(|req| req.get("timeout_ms"))
            .and_then(Value::as_u64);
        let stream_chunk_timeout_ms = request_config
            .and_then(|req| req.get("stream_chunk_timeout_ms"))
            .and_then(Value::as_u64);

        let headers = config
            .get("headers")
            .and_then(|h| serde_json::from_value::<HashMap<String, String>>(h.clone()).ok())
            .unwrap_or_default();

        let mut capabilities = ProviderCapabilities {
            supports_parallel_tools: true,
            supports_vision: true,
            supports_thinking: true,
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

        let mut builder = reqwest::Client::builder();
        if let Some(timeout) = timeout_ms {
            builder = builder.timeout(std::time::Duration::from_millis(timeout));
        }
        let client = builder.build().map_err(|e| {
            HarnessError::Provider(gestalt_core::ProviderError::Transport(
                std::io::Error::other(e),
            ))
        })?;

        let mut transport = CompletionsTransport::new(base_url, auth, resolver, headers);
        transport.client = client;

        Ok(Self {
            id,
            display_name,
            default_model,
            request_path,
            _timeout_ms: timeout_ms,
            stream_chunk_timeout_ms,
            transport,
            capabilities,
        })
    }

    #[must_use]
    pub const fn auth_config(&self) -> &ProviderAuthConfig {
        &self.transport.auth
    }

    fn body(&self, request: &ProviderRequest) -> Value {
        let (instructions, input) = convert_responses_messages(&request.messages);
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        let mut body = json!({
            "model": model,
            "input": input,
            "stream": true,
        });

        if let Some(inst) = instructions {
            body["instructions"] = json!(inst);
        }

        if !request.tools.is_empty() {
            body["tools"] = convert_responses_tools(&request.tools);
        }

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

        if request.max_tokens > 0 {
            body["max_output_tokens"] = json!(request.max_tokens);
        }

        body
    }

    pub fn normalize_sse(input: &str) -> Vec<Result<AgentEvent, HarnessError>> {
        let mut events = Vec::new();
        let state = Mutex::new(HashMap::new());

        for (_event, data) in crate::sse::parse_sse(input) {
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
}

#[async_trait]
impl Provider for OpenAiResponsesProvider {
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

        let path = self.request_path.as_deref().unwrap_or("/responses");
        let url = format!(
            "{}/{}",
            self.transport.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );

        let response = self
            .transport
            .client
            .post(&url)
            .headers(self.transport.build_headers()?)
            .json(&self.body(&request))
            .send()
            .await
            .map_err(|err| gestalt_core::ProviderError::Transport(std::io::Error::other(err)))?;
        let status = response.status();
        if !status.is_success() {
            let body_text = response.text().await.map_err(|err| {
                gestalt_core::ProviderError::Transport(std::io::Error::other(err))
            })?;
            if let Ok(value) = serde_json::from_str::<Value>(&body_text) {
                return Err(HarnessError::Provider(map_error(&value, &self.id)));
            }
            return Err(HarnessError::Provider(sse::provider_error_for_status(
                status, &body_text,
            )));
        }

        let state = Arc::new(Mutex::new(HashMap::new()));
        let chunk_timeout = self
            .stream_chunk_timeout_ms
            .map(std::time::Duration::from_millis);

        let sse_stream = response
            .bytes_stream()
            .map(|result| result.map_err(std::io::Error::other))
            .eventsource();

        let state_cloned = state.clone();
        let stream: gestalt_core::EventStream =
            if let Some(dur) = chunk_timeout {
                let mapped = tokio_stream::StreamExt::timeout(sse_stream, dur)
                    .map(move |item_res| match item_res {
                        Ok(Ok(event)) => {
                            if event.data == "[DONE]" {
                                Ok(Vec::new())
                            } else {
                                normalize_payload(&event.data, state_cloned.as_ref())
                            }
                        }
                        Ok(Err(err)) => Err(HarnessError::Provider(
                            gestalt_core::ProviderError::Transport(std::io::Error::other(err)),
                        )),
                        Err(_elapsed) => {
                            Err(HarnessError::Provider(gestalt_core::ProviderError::Timeout))
                        }
                    })
                    .map(
                        |result| -> futures::stream::BoxStream<
                            'static,
                            Result<AgentEvent, HarnessError>,
                        > {
                            match result {
                                Ok(events) => {
                                    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
                                }
                                Err(err) => Box::pin(futures::stream::iter(vec![Err(err)])),
                            }
                        },
                    )
                    .flatten();
                Box::pin(mapped)
            } else {
                let mapped = sse_stream
                    .map(move |event| match event {
                        Ok(event) if event.data == "[DONE]" => Ok(Vec::new()),
                        Ok(event) => normalize_payload(&event.data, state_cloned.as_ref()),
                        Err(err) => Err(HarnessError::Provider(
                            gestalt_core::ProviderError::Transport(std::io::Error::other(err)),
                        )),
                    })
                    .map(
                        |result| -> futures::stream::BoxStream<
                            'static,
                            Result<AgentEvent, HarnessError>,
                        > {
                            match result {
                                Ok(events) => {
                                    Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
                                }
                                Err(err) => Box::pin(futures::stream::iter(vec![Err(err)])),
                            }
                        },
                    )
                    .flatten();
                Box::pin(mapped)
            };

        Ok(stream)
    }
}

fn normalize_payload(
    data: &str,
    state: &Mutex<HashMap<String, ToolCallState>>,
) -> Result<Vec<AgentEvent>, HarnessError> {
    let value = sse::json(data)?;
    let mut events = Vec::new();

    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");

    match event_type {
        "response.output_text.delta" => {
            if let Some(content) = value.get("delta").and_then(Value::as_str) {
                if !content.is_empty() {
                    events.push(AgentEvent::Text {
                        delta: content.to_string(),
                    });
                }
            }
        }
        "response.output_item.added" => {
            if let Some(item) = value.get("item") {
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
                if item_type == "function_call" {
                    let id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let mut map = state.lock().map_err(|_| poisoned())?;
                    map.insert(id, ToolCallState { name });
                }
            }
        }
        "response.function_call_arguments.delta" => {
            let call_id = value
                .get("item_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let mut name = String::new();
            {
                let map = state.lock().map_err(|_| poisoned())?;
                if let Some(state) = map.get(&call_id) {
                    name = state.name.clone();
                }
            }
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                events.push(AgentEvent::ToolCallStreamed {
                    id: call_id,
                    name,
                    input_delta: delta.to_string(),
                });
            }
        }
        "response.completed" => {
            if let Some(resp) = value.get("response") {
                if let Some(usage) = resp.get("usage") {
                    let input_tokens = u64_to_usize(
                        usage
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    );
                    let output_tokens = u64_to_usize(
                        usage
                            .get("output_tokens")
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
            }
            events.push(AgentEvent::Stop {
                reason: gestalt_core::StopReason::EndTurn,
            });
        }
        "response.incomplete" => {
            let mut stop_reason = gestalt_core::StopReason::EndTurn;
            if let Some(reason) = value.get("reason").and_then(Value::as_str) {
                if reason == "max_output_tokens" {
                    stop_reason = gestalt_core::StopReason::MaxOutput;
                } else if reason == "content_filter" {
                    stop_reason = gestalt_core::StopReason::ContentFiltered;
                }
            }
            events.push(AgentEvent::Stop {
                reason: stop_reason,
            });
        }
        _ => {}
    }

    Ok(events)
}

fn convert_responses_messages(messages: &[Message]) -> (Option<String>, Vec<Value>) {
    let mut instructions = Vec::new();
    let mut input = Vec::new();

    for message in messages {
        match message {
            Message::System { content } => {
                instructions.push(content.clone());
            }
            Message::User { content, .. } => {
                let mut parts = Vec::new();
                for block in content {
                    match block {
                        ContentBlock::Text { text } => {
                            parts.push(json!({
                                "type": "input_text",
                                "text": text
                            }));
                        }
                        ContentBlock::Image { source } => {
                            parts.push(json!({
                                "type": "input_image",
                                "image": {
                                    "data": source.data,
                                    "media_type": source.media_type
                                }
                            }));
                        }
                        _ => {}
                    }
                }
                input.push(json!({
                    "role": "user",
                    "content": parts
                }));
            }
            Message::Assistant { content } => {
                let mut text_parts = Vec::new();
                for block in content {
                    match block {
                        ContentBlock::Text { text } => {
                            text_parts.push(json!({
                                "type": "text",
                                "text": text
                            }));
                        }
                        ContentBlock::Thinking { thinking } => {
                            text_parts.push(json!({
                                "type": "text",
                                "text": format!("<thinking>\n{thinking}\n</thinking>")
                            }));
                        }
                        ContentBlock::ToolUse {
                            id,
                            name,
                            input: tool_input,
                        } => {
                            input.push(json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": tool_input.to_string()
                            }));
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": text_parts
                    }));
                }
            }
            Message::ToolResult {
                tool_use_id,
                content,
                ..
            } => {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": tool_use_id,
                    "output": content
                }));
            }
        }
    }

    let inst_str = if instructions.is_empty() {
        None
    } else {
        Some(instructions.join("\n\n"))
    };

    (inst_str, input)
}

fn convert_responses_tools(tools: &[gestalt_core::provider::ProviderToolSchema]) -> Value {
    let mut output = Vec::new();
    for tool in tools {
        let mut tool_obj = json!({
            "type": "function",
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema
        });
        if let Some(strict) = tool.strict {
            tool_obj["strict"] = json!(strict);
        }
        output.push(tool_obj);
    }
    Value::Array(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_core::Message;
    use serde_json::json;

    #[test]
    fn openai_responses_request_body_shape() {
        let provider = OpenAiResponsesProvider::default();
        let request = ProviderRequest {
            model: "gpt-5.1".to_string(),
            messages: vec![
                Message::System {
                    content: "system text".to_string(),
                },
                Message::User {
                    content: vec![ContentBlock::Text {
                        text: "question".to_string(),
                    }],
                    metadata: None,
                },
                Message::Assistant {
                    content: vec![ContentBlock::ToolUse {
                        id: "call_1".to_string(),
                        name: "read".to_string(),
                        input: json!({"path": "README.md"}),
                    }],
                },
                Message::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "contents".to_string(),
                    is_error: false,
                    failure: None,
                    tool_name: Some("read".to_string()),
                    output_hash: Some("abc".to_string()),
                    artifact_refs: None,
                },
            ],
            tools: vec![gestalt_core::provider::ProviderToolSchema {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({"type": "object"}),
                strict: Some(true),
            }],
            tool_name_map: vec![],
            max_tokens: 1024,
            temperature: None,
            top_p: None,
            stop_sequences: vec![],
            cache_plan: None,
            metadata: Value::Null,
            reasoning_effort: Some(gestalt_core::provider::ReasoningEffort::High),
            text_verbosity: Some(gestalt_core::provider::TextVerbosity::Medium),
        };

        let body = provider.body(&request);

        assert_eq!(body.get("model").and_then(Value::as_str), Some("gpt-5.1"));
        assert_eq!(
            body.get("instructions").and_then(Value::as_str),
            Some("system text")
        );

        let input = body.get("input").and_then(Value::as_array).unwrap();
        assert_eq!(input.len(), 3);

        assert_eq!(input[0].get("role").and_then(Value::as_str), Some("user"));
        let user_content = input[0].get("content").and_then(Value::as_array).unwrap();
        assert_eq!(user_content.len(), 1);
        assert_eq!(
            user_content[0].get("type").and_then(Value::as_str),
            Some("input_text")
        );
        assert_eq!(
            user_content[0].get("text").and_then(Value::as_str),
            Some("question")
        );

        assert_eq!(
            input[1].get("type").and_then(Value::as_str),
            Some("function_call")
        );
        assert_eq!(
            input[1].get("call_id").and_then(Value::as_str),
            Some("call_1")
        );
        assert_eq!(input[1].get("name").and_then(Value::as_str), Some("read"));
        assert_eq!(
            input[1].get("arguments").and_then(Value::as_str),
            Some("{\"path\":\"README.md\"}")
        );

        assert_eq!(
            input[2].get("type").and_then(Value::as_str),
            Some("function_call_output")
        );
        assert_eq!(
            input[2].get("call_id").and_then(Value::as_str),
            Some("call_1")
        );
        assert_eq!(
            input[2].get("output").and_then(Value::as_str),
            Some("contents")
        );

        let tools = body.get("tools").and_then(Value::as_array).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].get("type").and_then(Value::as_str),
            Some("function")
        );
        assert_eq!(tools[0].get("name").and_then(Value::as_str), Some("read"));
        assert_eq!(
            tools[0].get("description").and_then(Value::as_str),
            Some("Read a file")
        );
        assert_eq!(tools[0].get("strict").and_then(Value::as_bool), Some(true));

        assert_eq!(
            body.get("reasoning")
                .and_then(|r| r.get("effort"))
                .and_then(Value::as_str),
            Some("high")
        );
        assert_eq!(
            body.get("text")
                .and_then(|t| t.get("verbosity"))
                .and_then(Value::as_str),
            Some("medium")
        );
    }
}
