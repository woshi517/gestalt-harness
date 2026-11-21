#![forbid(unsafe_code)]
//! OpenAI Chat Completions & Responses API (`ModelKind::Llm`) provider adapter (`"openai"`).

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use gestalt_kernel::{ModelCapabilities, ModelKind, Usage};
use gestalt_runtime::{
    ModelProvider, ModelRequest, ModelResponse, ProviderContext, ProviderErrorKind, RuntimeError,
    StreamDeltaFn, compute_cost_micro_usd, estimate_tokens_fallback,
};
use serde_json::{Value, json};

use crate::extensions::providers::{encode_base64, resolve_auth_headers};
use crate::extensions::{
    ChatRole, ContentBlock, LlmInput, LlmOutput, ThoughtBlock, ToolCallRequest,
};

const DEFAULT_OPENAI_ENDPOINT: &str = "https://api.openai.com/v1/chat/completions";

/// [`ModelProvider`] adapter for OpenAI Chat Completions and Responses APIs (`"openai"`).
#[derive(Debug, Clone, Default)]
pub struct OpenAiProvider;

impl ModelProvider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai"
    }

    fn capabilities(&self, model: &str) -> ModelCapabilities {
        let lower = model.to_ascii_lowercase();
        let supports_reasoning = lower.starts_with("o1")
            || lower.starts_with("o3")
            || lower.starts_with("o4")
            || lower.contains("gpt-5")
            || lower.contains("reason");
        ModelCapabilities {
            max_context_tokens: 128_000,
            max_output_tokens: 16_384,
            supports_prompt_caching: true,
            supports_reasoning,
            supports_vision: true,
        }
    }

    fn invoke<'a>(
        &'a self,
        req: &'a ModelRequest,
        ctx: &'a ProviderContext<'a>,
        on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            if req.kind != ModelKind::Llm {
                return Err(RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Fatal,
                    message: format!(
                        "openai provider only supports ModelKind::Llm, got {:?}",
                        req.kind
                    ),
                });
            }

            let endpoint = req
                .config
                .as_ref()
                .and_then(|c| c.endpoint.as_deref())
                .unwrap_or(DEFAULT_OPENAI_ENDPOINT);
            let use_responses_api = req
                .config
                .as_ref()
                .and_then(|c| c.extra.get("api_mode"))
                .and_then(Value::as_str)
                == Some("responses")
                || endpoint.trim_end_matches('/').ends_with("/responses");

            let mut headers = resolve_auth_headers(req, ctx, "Authorization", true).await?;
            headers.insert("content-type".to_string(), "application/json".to_string());

            let llm_input: LlmInput = serde_json::from_value(req.input.clone()).map_err(|e| {
                RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Fatal,
                    message: format!("invalid LlmInput for openai: {e}"),
                }
            })?;

            let body = if use_responses_api {
                build_responses_request_body(&req.model, &llm_input, ctx)?
            } else {
                build_chat_completions_body(&req.model, &llm_input, ctx)?
            };

            let mut state = OpenAiStreamState::default();
            ctx.http
                .post_sse(
                    endpoint,
                    &headers,
                    &body,
                    ctx.redactor.clone(),
                    ctx.cancel.clone(),
                    |ev| {
                        if use_responses_api {
                            state.handle_responses_event(&ev.data, on_delta)
                        } else {
                            state.handle_chat_event(&ev.data, on_delta)
                        }
                    },
                )
                .await?;

            let raw_in_tok = state.input_tokens;
            let raw_out_tok = state.output_tokens;
            let out = state.finish_output();
            let input_units = if raw_in_tok > 0 {
                raw_in_tok
            } else {
                estimate_tokens_fallback(&body.to_string())
            };
            let output_units = if raw_out_tok > 0 {
                raw_out_tok
            } else {
                estimate_tokens_fallback(&out.text)
            };
            let cost_micro_usd =
                compute_cost_micro_usd(input_units, output_units, req.config.as_ref());

            Ok(ModelResponse {
                kind: ModelKind::Llm,
                output: serde_json::to_value(out)?,
                usage: Usage {
                    input_units,
                    output_units,
                    cost_micro_usd,
                },
            })
        })
    }
}

fn build_chat_completions_body(
    model: &str,
    input: &LlmInput,
    ctx: &ProviderContext<'_>,
) -> Result<Value, RuntimeError> {
    let mut messages = Vec::new();
    let system_text: String = input
        .system_tiers
        .iter()
        .filter(|t| !t.content.is_empty())
        .map(|t| t.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if !system_text.is_empty() {
        messages.push(json!({ "role": "system", "content": system_text }));
    }

    for msg in &input.messages {
        match msg.role {
            ChatRole::Assistant => {
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => text_parts.push(text.as_str()),
                        ContentBlock::ToolUse { id, name, input } => {
                            tool_calls.push(json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
                                }
                            }));
                        }
                        ContentBlock::Thought { .. }
                        | ContentBlock::ToolResult { .. }
                        | ContentBlock::Media { .. } => {}
                    }
                }
                let mut obj = serde_json::Map::new();
                obj.insert("role".to_string(), Value::String("assistant".to_string()));
                if !text_parts.is_empty() {
                    obj.insert("content".to_string(), Value::String(text_parts.join("")));
                }
                if !tool_calls.is_empty() {
                    obj.insert("tool_calls".to_string(), Value::Array(tool_calls));
                }
                messages.push(Value::Object(obj));
            }
            ChatRole::User | ChatRole::Tool => {
                let mut user_parts = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            user_parts.push(json!({ "type": "text", "text": text }));
                        }
                        ContentBlock::Media {
                            mime_type,
                            blob_digest,
                        } => {
                            let bytes = ctx.blobs.get_bytes(blob_digest)?;
                            let b64 = encode_base64(&bytes);
                            user_parts.push(json!({
                                "type": "image_url",
                                "image_url": { "url": format!("data:{mime_type};base64,{b64}") }
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            let content_str = match content {
                                Value::String(s) => s.clone(),
                                other => serde_json::to_string(other).unwrap_or_default(),
                            };
                            messages.push(json!({
                                "role": "tool",
                                "tool_call_id": tool_use_id,
                                "content": content_str,
                            }));
                        }
                        ContentBlock::Thought { .. } | ContentBlock::ToolUse { .. } => {}
                    }
                }
                if !user_parts.is_empty() {
                    messages.push(json!({ "role": "user", "content": user_parts }));
                }
            }
        }
    }

    let tools_json: Vec<Value> = input
        .tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters_schema,
                }
            })
        })
        .collect();

    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(model.to_string()));
    body.insert("messages".to_string(), Value::Array(messages));
    body.insert("stream".to_string(), Value::Bool(true));
    body.insert(
        "stream_options".to_string(),
        json!({ "include_usage": true }),
    );
    if !tools_json.is_empty() {
        body.insert("tools".to_string(), Value::Array(tools_json));
    }
    if let Some(max_tok) = input.max_output_tokens {
        body.insert("max_tokens".to_string(), json!(max_tok));
    }
    Ok(Value::Object(body))
}

fn build_responses_request_body(
    model: &str,
    input: &LlmInput,
    ctx: &ProviderContext<'_>,
) -> Result<Value, RuntimeError> {
    let instructions: String = input
        .system_tiers
        .iter()
        .filter(|t| !t.content.is_empty())
        .map(|t| t.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");

    let mut items = Vec::new();
    for msg in &input.messages {
        let role = match msg.role {
            ChatRole::Assistant => "assistant",
            ChatRole::User | ChatRole::Tool => "user",
        };
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    items.push(json!({
                        "type": "message",
                        "role": role,
                        "content": [{ "type": "input_text", "text": text }],
                    }));
                }
                ContentBlock::Media {
                    mime_type,
                    blob_digest,
                } => {
                    let bytes = ctx.blobs.get_bytes(blob_digest)?;
                    let b64 = encode_base64(&bytes);
                    items.push(json!({
                        "type": "message",
                        "role": "user",
                        "content": [{
                            "type": "input_image",
                            "image_url": format!("data:{mime_type};base64,{b64}"),
                        }],
                    }));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    items.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()),
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } => {
                    let output_str = match content {
                        Value::String(s) => s.clone(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    items.push(json!({
                        "type": "function_call_output",
                        "call_id": tool_use_id,
                        "output": output_str,
                    }));
                }
                ContentBlock::Thought { .. } => {}
            }
        }
    }

    let tools_json: Vec<Value> = input
        .tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters_schema,
            })
        })
        .collect();

    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(model.to_string()));
    body.insert("input".to_string(), Value::Array(items));
    body.insert("stream".to_string(), Value::Bool(true));
    if !instructions.is_empty() {
        body.insert("instructions".to_string(), Value::String(instructions));
    }
    if !tools_json.is_empty() {
        body.insert("tools".to_string(), Value::Array(tools_json));
    }
    if let Some(max_tok) = input.max_output_tokens {
        body.insert("max_output_tokens".to_string(), json!(max_tok));
    }
    Ok(Value::Object(body))
}

#[derive(Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
struct OpenAiStreamState {
    text: String,
    reasoning_buf: String,
    tool_calls_by_idx: BTreeMap<usize, PartialToolCall>,
    tool_calls_by_item: BTreeMap<String, PartialToolCall>,
    stop_reason: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
}

impl OpenAiStreamState {
    fn handle_chat_event(
        &mut self,
        raw_data: &str,
        on_delta: StreamDeltaFn<'_>,
    ) -> Result<(), RuntimeError> {
        let trimmed = raw_data.trim();
        if trimmed.is_empty() || trimmed == "[DONE]" {
            return Ok(());
        }
        let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
            return Ok(());
        };
        if let Some(u) = val.get("usage") {
            if let Some(p) = u.get("prompt_tokens").and_then(Value::as_u64) {
                self.input_tokens = self.input_tokens.max(p);
            }
            if let Some(c) = u.get("completion_tokens").and_then(Value::as_u64) {
                self.output_tokens = self.output_tokens.max(c);
            }
        }
        let Some(choice) = val.pointer("/choices/0") else {
            return Ok(());
        };
        if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = Some(fr.to_string());
        }
        let Some(delta) = choice.get("delta") else {
            return Ok(());
        };
        if let Some(content) = delta.get("content").and_then(Value::as_str) {
            self.text.push_str(content);
            on_delta("text", Value::String(content.to_string()));
        }
        if let Some(reasoning) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
        {
            self.reasoning_buf.push_str(reasoning);
            on_delta("thought", Value::String(reasoning.to_string()));
        }
        if let Some(tc_arr) = delta.get("tool_calls").and_then(Value::as_array) {
            for tc in tc_arr {
                let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let entry = self.tool_calls_by_idx.entry(idx).or_default();
                if let Some(id) = tc.get("id").and_then(Value::as_str) {
                    entry.id = id.to_string();
                }
                if let Some(name) = tc.pointer("/function/name").and_then(Value::as_str) {
                    entry.name.push_str(name);
                }
                if let Some(args) = tc.pointer("/function/arguments").and_then(Value::as_str) {
                    entry.arguments.push_str(args);
                }
            }
        }
        Ok(())
    }

    fn handle_responses_event(
        &mut self,
        raw_data: &str,
        on_delta: StreamDeltaFn<'_>,
    ) -> Result<(), RuntimeError> {
        let trimmed = raw_data.trim();
        if trimmed.is_empty() || trimmed == "[DONE]" {
            return Ok(());
        }
        let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
            return Ok(());
        };
        let ev_type = val.get("type").and_then(Value::as_str).unwrap_or("");
        match ev_type {
            "response.output_text.delta" => {
                if let Some(chunk) = val.get("delta").and_then(Value::as_str) {
                    self.text.push_str(chunk);
                    on_delta("text", Value::String(chunk.to_string()));
                }
            }
            "response.reasoning_summary_text.delta" => {
                if let Some(chunk) = val.get("delta").and_then(Value::as_str) {
                    self.reasoning_buf.push_str(chunk);
                    on_delta("thought", Value::String(chunk.to_string()));
                }
            }
            "response.output_item.added" => {
                let item = val.get("item").unwrap_or(&Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    let item_id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let call_id = item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or(&item_id)
                        .to_string();
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    self.tool_calls_by_item.insert(
                        item_id,
                        PartialToolCall {
                            id: call_id,
                            name,
                            arguments: String::new(),
                        },
                    );
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(item_id) = val.get("item_id").and_then(Value::as_str)
                    && let Some(delta) = val.get("delta").and_then(Value::as_str)
                {
                    let entry = self
                        .tool_calls_by_item
                        .entry(item_id.to_string())
                        .or_default();
                    entry.arguments.push_str(delta);
                }
            }
            "response.completed" => {
                self.stop_reason = Some("completed".to_string());
                if let Some(u) = val.pointer("/response/usage") {
                    self.input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    self.output_tokens =
                        u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn finish_output(self) -> LlmOutput {
        let mut tool_calls = Vec::new();
        for (_, tc) in self.tool_calls_by_idx {
            tool_calls.push(parse_partial_tool(tc));
        }
        for (_, tc) in self.tool_calls_by_item {
            tool_calls.push(parse_partial_tool(tc));
        }
        let reasoning = if self.reasoning_buf.is_empty() {
            Vec::new()
        } else {
            vec![ThoughtBlock {
                text: self.reasoning_buf,
                signature: None,
            }]
        };
        LlmOutput {
            text: self.text,
            reasoning,
            tool_calls,
            stop_reason: self.stop_reason,
        }
    }
}

fn parse_partial_tool(tc: PartialToolCall) -> ToolCallRequest {
    let args = if tc.arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str::<Value>(&tc.arguments)
            .unwrap_or_else(|_| json!({ "raw_invalid_json": tc.arguments }))
    };
    ToolCallRequest {
        id: tc.id,
        name: tc.name,
        args,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_runtime::{
        BlobStore, CancellationToken, CredentialStore, ProviderConfig, SharedHttpClient,
        StreamRedactor,
    };
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn openai_provider_streams_chat_completions_and_tool_calls() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");

        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let sse_body = concat!(
                    "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Plan.\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"Running \"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"tool.\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\"}}]}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"ls\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":200,\"completion_tokens\":20}}\n\n",
                    "data: [DONE]\n\n",
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    sse_body.len(),
                    sse_body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        let mut creds = CredentialStore::default();
        creds.insert_runtime_secret("openai", "sk-openai-test-12345");
        let http = SharedHttpClient::new(10).expect("http");
        let blobs = BlobStore::new(&std::env::temp_dir());
        let redactor = StreamRedactor::default();
        let pctx = ProviderContext {
            credentials: &creds,
            http: &http,
            blobs: &blobs,
            redactor: &redactor,
            cancel: CancellationToken::new(),
        };

        let req = ModelRequest {
            effect_id: "e1".into(),
            turn_id: "t1".into(),
            phase_id: "p1".into(),
            provider_id: "openai".into(),
            model: "gpt-5".into(),
            kind: ModelKind::Llm,
            input: json!({
                "system_tiers": [{ "name": "stable", "cacheable": true, "content": "Sys" }],
                "messages": [{ "role": "user", "content": [{ "kind": "text", "text": "List files" }] }],
                "tools": [{ "name": "bash", "description": "Run shell", "parameters_schema": { "type": "object" } }]
            }),
            schema: None,
            config: Some(ProviderConfig {
                endpoint: Some(format!("http://{addr}/v1/chat/completions")),
                model: "gpt-5".into(),
                input_cost_per_mtok_usd: Some(2.0),
                output_cost_per_mtok_usd: Some(8.0),
                ..ProviderConfig::default()
            }),
        };

        let deltas = Arc::new(Mutex::new(Vec::new()));
        let deltas_cb = deltas.clone();
        let resp = OpenAiProvider
            .invoke(&req, &pctx, &move |ch, payload| {
                if let Ok(mut g) = deltas_cb.lock() {
                    g.push((ch.to_string(), payload));
                }
            })
            .await
            .expect("openai invoke");

        let parsed: LlmOutput = serde_json::from_value(resp.output).expect("parse LlmOutput");
        assert_eq!(parsed.text, "Running tool.");
        assert_eq!(parsed.reasoning[0].text, "Plan.");
        assert_eq!(parsed.tool_calls[0].id, "call_1");
        assert_eq!(parsed.tool_calls[0].args["command"], "ls");
        assert_eq!(resp.usage.cost_micro_usd, 200 * 2 + 20 * 8);
    }
}
