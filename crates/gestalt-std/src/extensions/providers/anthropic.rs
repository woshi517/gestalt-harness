#![forbid(unsafe_code)]
//! Anthropic Messages API (`ModelKind::Llm`) provider adapter (`"anthropic"`).

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

const DEFAULT_ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u64 = 8_192;

/// [`ModelProvider`] adapter for Anthropic Messages API (`"anthropic"`).
#[derive(Debug, Clone, Default)]
pub struct AnthropicProvider;

impl ModelProvider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn capabilities(&self, model: &str) -> ModelCapabilities {
        let lower = model.to_ascii_lowercase();
        let max_output =
            if lower.contains("sonnet-4") || lower.contains("opus-4") || lower.contains("3-7") {
                16_384
            } else {
                8_192
            };
        ModelCapabilities {
            max_context_tokens: 200_000,
            max_output_tokens: max_output,
            supports_prompt_caching: true,
            supports_reasoning: true,
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
                        "anthropic provider only supports ModelKind::Llm, got {:?}",
                        req.kind
                    ),
                });
            }

            let endpoint = req
                .config
                .as_ref()
                .and_then(|c| c.endpoint.as_deref())
                .unwrap_or(DEFAULT_ANTHROPIC_ENDPOINT);

            let mut headers = resolve_auth_headers(req, ctx, "x-api-key", false).await?;
            let api_ver = req
                .config
                .as_ref()
                .and_then(|c| c.extra.get("anthropic_version"))
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_ANTHROPIC_VERSION);
            headers.insert("anthropic-version".to_string(), api_ver.to_string());
            headers.insert("content-type".to_string(), "application/json".to_string());
            if let Some(beta) = req
                .config
                .as_ref()
                .and_then(|c| c.extra.get("anthropic_beta"))
                .and_then(Value::as_str)
            {
                headers.insert("anthropic-beta".to_string(), beta.to_string());
            }

            let llm_input: LlmInput = serde_json::from_value(req.input.clone()).map_err(|e| {
                RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Fatal,
                    message: format!("invalid LlmInput for anthropic: {e}"),
                }
            })?;

            let body = build_anthropic_request_body(&req.model, &llm_input, ctx)?;

            let mut state = AnthropicStreamState::default();
            ctx.http
                .post_sse(
                    endpoint,
                    &headers,
                    &body,
                    ctx.redactor.clone(),
                    ctx.cancel.clone(),
                    |ev| state.handle_sse_event(&ev.data, on_delta),
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

fn build_anthropic_request_body(
    model: &str,
    input: &LlmInput,
    ctx: &ProviderContext<'_>,
) -> Result<Value, RuntimeError> {
    let mut system_blocks = Vec::new();
    for tier in &input.system_tiers {
        if tier.content.is_empty() {
            continue;
        }
        let mut obj = serde_json::Map::new();
        obj.insert("type".to_string(), Value::String("text".to_string()));
        obj.insert("text".to_string(), Value::String(tier.content.clone()));
        if tier.cacheable {
            obj.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
        }
        system_blocks.push(Value::Object(obj));
    }

    let mut messages = Vec::new();
    for msg in &input.messages {
        let role = match msg.role {
            ChatRole::Assistant => "assistant",
            ChatRole::User | ChatRole::Tool => "user",
        };
        let mut content_blocks = Vec::new();
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    content_blocks.push(json!({ "type": "text", "text": text }));
                }
                ContentBlock::Thought { text, signature } => {
                    let mut obj = serde_json::Map::new();
                    obj.insert("type".to_string(), Value::String("thinking".to_string()));
                    obj.insert("thinking".to_string(), Value::String(text.clone()));
                    if let Some(sig) = signature {
                        obj.insert("signature".to_string(), Value::String(sig.clone()));
                    }
                    content_blocks.push(Value::Object(obj));
                }
                ContentBlock::ToolUse { id, name, input } => {
                    content_blocks.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": name,
                        "input": input,
                    }));
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                    ..
                } => {
                    let content_str = match content {
                        Value::String(s) => s.clone(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    content_blocks.push(json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": content_str,
                        "is_error": is_error,
                    }));
                }
                ContentBlock::Media {
                    mime_type,
                    blob_digest,
                } => {
                    let bytes = ctx.blobs.get_bytes(blob_digest)?;
                    let b64 = encode_base64(&bytes);
                    content_blocks.push(json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": mime_type,
                            "data": b64,
                        }
                    }));
                }
            }
        }
        if !content_blocks.is_empty() {
            messages.push(json!({
                "role": role,
                "content": content_blocks,
            }));
        }
    }

    let mut tools_json = Vec::with_capacity(input.tools.len());
    for (idx, tool) in input.tools.iter().enumerate() {
        let mut obj = serde_json::Map::new();
        obj.insert("name".to_string(), Value::String(tool.name.clone()));
        obj.insert(
            "description".to_string(),
            Value::String(tool.description.clone()),
        );
        obj.insert("input_schema".to_string(), tool.parameters_schema.clone());
        if idx + 1 == input.tools.len() {
            obj.insert("cache_control".to_string(), json!({ "type": "ephemeral" }));
        }
        tools_json.push(Value::Object(obj));
    }

    let max_tokens = input.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(model.to_string()));
    body.insert("max_tokens".to_string(), json!(max_tokens));
    body.insert("stream".to_string(), Value::Bool(true));
    if !system_blocks.is_empty() {
        body.insert("system".to_string(), Value::Array(system_blocks));
    }
    body.insert("messages".to_string(), Value::Array(messages));
    if !tools_json.is_empty() {
        body.insert("tools".to_string(), Value::Array(tools_json));
    }
    if let Some(budget) = input.reasoning_budget_tokens
        && budget > 0
    {
        body.insert(
            "thinking".to_string(),
            json!({ "type": "enabled", "budget_tokens": budget }),
        );
    }
    Ok(Value::Object(body))
}

#[derive(Default)]
struct AnthropicStreamState {
    text: String,
    reasoning: Vec<ThoughtBlock>,
    tool_calls: Vec<ToolCallRequest>,
    active_blocks: BTreeMap<usize, ActiveAnthropicBlock>,
    stop_reason: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
}

enum ActiveAnthropicBlock {
    Text,
    Thinking {
        text: String,
        signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        partial_json: String,
    },
}

impl AnthropicStreamState {
    fn handle_sse_event(
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
            "message_start" => {
                if let Some(u) = val.pointer("/message/usage") {
                    let base = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    let cache_read = u
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let cache_create = u
                        .get("cache_creation_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    self.input_tokens = self
                        .input_tokens
                        .saturating_add(base)
                        .saturating_add(cache_read)
                        .saturating_add(cache_create);
                }
            }
            "content_block_start" => {
                let idx = val.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let block = val.get("content_block").unwrap_or(&Value::Null);
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        self.active_blocks.insert(idx, ActiveAnthropicBlock::Text);
                    }
                    Some("thinking") => {
                        self.active_blocks.insert(
                            idx,
                            ActiveAnthropicBlock::Thinking {
                                text: String::new(),
                                signature: None,
                            },
                        );
                    }
                    Some("tool_use") => {
                        let id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        self.active_blocks.insert(
                            idx,
                            ActiveAnthropicBlock::ToolUse {
                                id,
                                name,
                                partial_json: String::new(),
                            },
                        );
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let idx = val.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let delta = val.get("delta").unwrap_or(&Value::Null);
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(chunk) = delta.get("text").and_then(Value::as_str) {
                            self.text.push_str(chunk);
                            on_delta("text", Value::String(chunk.to_string()));
                        }
                    }
                    Some("thinking_delta") => {
                        if let Some(chunk) = delta.get("thinking").and_then(Value::as_str) {
                            if let Some(ActiveAnthropicBlock::Thinking { text, .. }) =
                                self.active_blocks.get_mut(&idx)
                            {
                                text.push_str(chunk);
                            }
                            on_delta("thought", Value::String(chunk.to_string()));
                        }
                    }
                    Some("signature_delta") => {
                        if let Some(sig) = delta.get("signature").and_then(Value::as_str)
                            && let Some(ActiveAnthropicBlock::Thinking { signature, .. }) =
                                self.active_blocks.get_mut(&idx)
                        {
                            *signature = Some(sig.to_string());
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(part) = delta.get("partial_json").and_then(Value::as_str)
                            && let Some(ActiveAnthropicBlock::ToolUse { partial_json, .. }) =
                                self.active_blocks.get_mut(&idx)
                        {
                            partial_json.push_str(part);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let idx = val.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(block) = self.active_blocks.remove(&idx) {
                    self.finalize_block(block);
                }
            }
            "message_delta" => {
                if let Some(sr) = val.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(sr.to_string());
                }
                if let Some(out_tok) = val.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                    self.output_tokens = self.output_tokens.max(out_tok);
                }
            }
            "error" => {
                let msg = val
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("anthropic stream error");
                return Err(RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Fatal,
                    message: msg.to_string(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn finalize_block(&mut self, block: ActiveAnthropicBlock) {
        match block {
            ActiveAnthropicBlock::Text => {}
            ActiveAnthropicBlock::Thinking { text, signature } => {
                self.reasoning.push(ThoughtBlock { text, signature });
            }
            ActiveAnthropicBlock::ToolUse {
                id,
                name,
                partial_json,
            } => {
                let args = if partial_json.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str::<Value>(&partial_json)
                        .unwrap_or_else(|_| json!({ "raw_invalid_json": partial_json }))
                };
                self.tool_calls.push(ToolCallRequest { id, name, args });
            }
        }
    }

    fn finish_output(mut self) -> LlmOutput {
        let remaining: Vec<ActiveAnthropicBlock> = std::mem::take(&mut self.active_blocks)
            .into_values()
            .collect();
        for block in remaining {
            self.finalize_block(block);
        }
        LlmOutput {
            text: self.text,
            reasoning: self.reasoning,
            tool_calls: self.tool_calls,
            stop_reason: self.stop_reason,
        }
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
    async fn anthropic_provider_streams_thinking_text_tool_use_and_caching() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let captured_req = Arc::new(Mutex::new(String::new()));
        let captured_clone = captured_req.clone();

        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if let Ok(mut g) = captured_clone.lock() {
                    *g = String::from_utf8_lossy(&buf[..n]).to_string();
                }
                let sse_body = concat!(
                    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":500}}}\n\n",
                    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Need to read.\"}}\n\n",
                    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Checking \"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"now.\"}}\n\n",
                    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
                    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"read\"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
                    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.rs\\\"}\"}}\n\n",
                    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
                    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":40}}\n\n",
                    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
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
        creds.insert_runtime_secret("anthropic", "sk-ant-test-key-12345");
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
            provider_id: "anthropic".into(),
            model: "claude-sonnet-4-5".into(),
            kind: ModelKind::Llm,
            input: json!({
                "system_tiers": [
                    { "name": "stable", "cacheable": true, "content": "Be concise." }
                ],
                "messages": [
                    { "role": "user", "content": [{ "kind": "text", "text": "Read a.rs" }] }
                ],
                "tools": [
                    { "name": "read", "description": "Read file", "parameters_schema": { "type": "object" } }
                ]
            }),
            schema: None,
            config: Some(ProviderConfig {
                endpoint: Some(format!("http://{addr}/v1/messages")),
                model: "claude-sonnet-4-5".into(),
                input_cost_per_mtok_usd: Some(3.0),
                output_cost_per_mtok_usd: Some(15.0),
                ..ProviderConfig::default()
            }),
        };

        let deltas = Arc::new(Mutex::new(Vec::new()));
        let deltas_cb = deltas.clone();
        let resp = AnthropicProvider
            .invoke(&req, &pctx, &move |ch, payload| {
                if let Ok(mut g) = deltas_cb.lock() {
                    g.push((ch.to_string(), payload));
                }
            })
            .await
            .expect("anthropic invoke");

        let parsed: LlmOutput = serde_json::from_value(resp.output).expect("parse LlmOutput");
        assert_eq!(parsed.text, "Checking now.");
        assert_eq!(parsed.reasoning.len(), 1);
        assert_eq!(parsed.reasoning[0].text, "Need to read.");
        assert_eq!(parsed.tool_calls.len(), 1);
        assert_eq!(parsed.tool_calls[0].args["path"], "a.rs");
        assert_eq!(parsed.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(resp.usage.cost_micro_usd, 500 * 3 + 40 * 15);

        let raw_http = captured_req.lock().expect("lock").clone();
        assert!(raw_http.contains("x-api-key: sk-ant-test-key-12345"));
        assert!(raw_http.contains("ephemeral"));

        let captured_deltas = deltas.lock().expect("lock");
        assert_eq!(captured_deltas.len(), 3);
        assert_eq!(captured_deltas[0].0, "thought");
        assert_eq!(captured_deltas[1].0, "text");
    }
}
