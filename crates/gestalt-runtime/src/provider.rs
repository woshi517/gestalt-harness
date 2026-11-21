#![forbid(unsafe_code)]
//! Composable `ModelProvider` trait, `ProviderContext`, `ProviderRegistry`,
//! target/alias & rate resolution, HTTP error classification, and shared
//! `reqwest` JSON + SSE streaming client.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use gestalt_kernel::{ModelCapabilities, ModelKind, Usage};
use serde_json::Value;

use crate::blobs::BlobStore;
use crate::config::ProviderConfig;
use crate::credentials::{CredentialStore, StreamRedactor};
use crate::{CancellationToken, ProviderErrorKind, RuntimeError};

/// Normalized request passed to a registered [`ModelProvider`].
///
/// `input` and `schema` remain opaque [`serde_json::Value`] so any provider
/// adapter (chat LLMs, decision classifiers, embeddings, world models, or
/// custom protocols) can define its own typed message schemas without modifying
/// `gestalt-runtime`.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// Correlated kernel `effect_id`.
    pub effect_id: String,
    /// Active Turn ID.
    pub turn_id: String,
    /// Active Phase ID.
    pub phase_id: String,
    /// Resolved provider adapter ID.
    pub provider_id: String,
    /// Resolved model identifier.
    pub model: String,
    /// Model paradigm requested by `Effect::Model`.
    pub kind: ModelKind,
    /// Paradigm-specific JSON payload built by a `ContextBuilder`.
    pub input: Value,
    /// Optional structured output JSON schema.
    pub schema: Option<Value>,
    /// Optional provider configuration from `gestalt.toml`.
    pub config: Option<ProviderConfig>,
}

/// Normalized response returned by a [`ModelProvider`].
#[derive(Debug, Clone, PartialEq)]
pub struct ModelResponse {
    /// Model paradigm that executed.
    pub kind: ModelKind,
    /// Normalized model output JSON.
    pub output: Value,
    /// Token and micro-USD usage.
    pub usage: Usage,
}

/// Shared host services passed to [`ModelProvider::invoke`] so custom providers
/// can resolve credentials (API key/OAuth/custom), load multimodal blobs, reuse
/// the shared TLS/SSE HTTP client, and honor cooperative cancellation without
/// changing the [`ModelProvider`] trait signature.
pub struct ProviderContext<'a> {
    /// Host credential store (supporting API keys, OAuth, headers, and custom resolvers).
    pub credentials: &'a CredentialStore,
    /// Shared `reqwest` HTTP/SSE client (`rustls-tls`).
    pub http: &'a SharedHttpClient,
    /// Content-addressed blob store for resolving `{ kind: "media", blob_digest }` blocks.
    pub blobs: &'a BlobStore,
    /// Secret redactor for scrubbing streaming payloads.
    pub redactor: &'a StreamRedactor,
    /// Cooperative cancellation token for `<= 250 ms` abort.
    pub cancel: CancellationToken,
}

/// Callback invoked for transient streaming deltas (`channel`, `payload`).
pub type StreamDeltaFn<'a> = &'a (dyn Fn(&str, Value) + Send + Sync);

/// Vendor-neutral async model provider contract implemented in user space or
/// external extensions.
pub trait ModelProvider: Send + Sync {
    /// Unique provider adapter identifier (e.g., `"mock_llm"`).
    fn id(&self) -> &str;

    /// Capabilities reported to `ContextBuilder`s for `model`.
    fn capabilities(&self, model: &str) -> ModelCapabilities;

    /// Execute a model request using host services in `ctx`, emitting transient
    /// deltas to `on_delta`.
    fn invoke<'a>(
        &'a self,
        req: &'a ModelRequest,
        ctx: &'a ProviderContext<'a>,
        on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>>;
}

/// Resolved target returned by [`ProviderRegistry::resolve_target`].
#[derive(Clone)]
pub struct ResolvedProviderTarget {
    /// Registered provider adapter implementation.
    pub provider: Arc<dyn ModelProvider>,
    /// Target model identifier.
    pub model: String,
    /// Optional provider configuration from `gestalt.toml`.
    pub config: Option<ProviderConfig>,
}

/// Registry mapping provider IDs and `gestalt.toml` aliases to [`ModelProvider`]s.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: BTreeMap<String, Arc<dyn ModelProvider>>,
    configs: BTreeMap<String, ProviderConfig>,
}

impl ProviderRegistry {
    /// Create an empty [`ProviderRegistry`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a [`ModelProvider`] implementation under its `id()`.
    pub fn register_provider(&mut self, provider: Arc<dyn ModelProvider>) {
        self.providers.insert(provider.id().to_string(), provider);
    }

    /// Merge `[providers.<id>]` configurations loaded from `gestalt.toml`.
    pub fn merge_configs(&mut self, configs: BTreeMap<String, ProviderConfig>) {
        self.configs.extend(configs);
    }

    /// Borrow a registered provider by ID.
    #[must_use]
    pub fn get_provider(&self, id: &str) -> Option<&Arc<dyn ModelProvider>> {
        self.providers.get(id)
    }

    /// Borrow a provider config by ID or alias.
    #[must_use]
    pub fn get_config(&self, id: &str) -> Option<&ProviderConfig> {
        self.configs.get(id)
    }

    /// List all registered provider adapter IDs in deterministic order.
    #[must_use]
    pub fn registered_provider_ids(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Borrow all configured provider entries/aliases from `gestalt.toml`.
    #[must_use]
    pub fn configured_aliases(&self) -> &BTreeMap<String, ProviderConfig> {
        &self.configs
    }

    /// Resolve a target string (`"<provider_or_alias>/<model>"` or configured alias)
    /// into a [`ResolvedProviderTarget`].
    ///
    /// Supports custom aliases whose `[providers.<alias>]` sets `adapter = "<provider_id>"`,
    /// allowing multiple endpoints/models to share one provider implementation.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ProviderError`] if no matching provider adapter is registered.
    pub fn resolve_target(&self, target: &str) -> Result<ResolvedProviderTarget, RuntimeError> {
        if let Some((prefix, model)) = target.split_once('/') {
            let config = self.configs.get(prefix).cloned();
            let adapter_id = config
                .as_ref()
                .and_then(|c| c.adapter.as_deref())
                .unwrap_or(prefix);
            let provider = self.providers.get(adapter_id).cloned().ok_or_else(|| {
                RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Fatal,
                    message: format!(
                        "unknown model provider adapter '{adapter_id}' for target '{target}'"
                    ),
                }
            })?;
            return Ok(ResolvedProviderTarget {
                provider,
                model: model.to_string(),
                config,
            });
        }

        if let Some(cfg) = self.configs.get(target) {
            let adapter_id = cfg.adapter.as_deref().unwrap_or(target);
            if let Some(provider) = self.providers.get(adapter_id) {
                return Ok(ResolvedProviderTarget {
                    provider: provider.clone(),
                    model: cfg.model.clone(),
                    config: Some(cfg.clone()),
                });
            }
        }

        if let Some(provider) = self.providers.get(target) {
            return Ok(ResolvedProviderTarget {
                provider: provider.clone(),
                model: target.to_string(),
                config: None,
            });
        }

        Err(RuntimeError::ProviderError {
            kind: ProviderErrorKind::Fatal,
            message: format!("unable to resolve model target '{target}'"),
        })
    }
}

/// Compute integer `cost_micro_usd` (`1_000_000 = $1.00`) from token counts and
/// `[providers.<id>]` rate tables.
///
/// Because `$1.00 / 1,000,000 tokens == 1 micro_usd / token`, multiplying token
/// counts by `cost_per_mtok_usd` yields `micro_usd` directly.
#[must_use]
pub fn compute_cost_micro_usd(
    input_tokens: u64,
    output_tokens: u64,
    cfg: Option<&ProviderConfig>,
) -> u64 {
    let Some(c) = cfg else {
        return 0;
    };
    let in_cost = c
        .input_cost_per_mtok_usd
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map_or(0, |rate| ((input_tokens as f64) * rate).round() as u64);
    let out_cost = c
        .output_cost_per_mtok_usd
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map_or(0, |rate| ((output_tokens as f64) * rate).round() as u64);
    in_cost.saturating_add(out_cost)
}

/// Fallback token count estimator (`chars / 4`, rounded up).
#[must_use]
pub fn estimate_tokens_fallback(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// Classify an HTTP error status code and response body into [`ProviderErrorKind`].
#[must_use]
pub fn classify_http_error(status: u16, body: &str) -> ProviderErrorKind {
    match status {
        401 | 403 => ProviderErrorKind::AuthError,
        429 | 502 | 503 | 504 | 529 => ProviderErrorKind::Retryable,
        400 | 413 | 422 => {
            let lower = body.to_ascii_lowercase();
            if lower.contains("context_length_exceeded")
                || lower.contains("maximum context length")
                || lower.contains("prompt is too long")
                || lower.contains("too many tokens")
                || lower.contains("context window")
            {
                ProviderErrorKind::ContextLengthExceeded
            } else {
                ProviderErrorKind::Fatal
            }
        }
        500..=599 => ProviderErrorKind::Retryable,
        _ => ProviderErrorKind::Fatal,
    }
}

/// A single parsed Server-Sent Events (SSE) frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// Optional `event:` field value.
    pub event: Option<String>,
    /// Joined `data:` payload lines.
    pub data: String,
}

/// Incremental SSE line/event parser handling arbitrary TCP chunk fragmentation
/// and redacting any complete secret in assembled event payloads.
///
/// Cross-event streaming token redaction is applied on the consumer delta stream
/// via [`StreamRedactor::push_chunk`].
#[derive(Debug, Clone, Default)]
pub struct SseLineParser {
    byte_buf: Vec<u8>,
    redactor: StreamRedactor,
    current_event: Option<String>,
    current_data: Vec<String>,
}

impl SseLineParser {
    /// Create a new [`SseLineParser`] using `redactor`.
    #[must_use]
    pub fn new(redactor: StreamRedactor) -> Self {
        Self {
            byte_buf: Vec::new(),
            redactor,
            current_event: None,
            current_data: Vec::new(),
        }
    }

    /// Feed a raw byte chunk from an HTTP response stream and return any
    /// complete, secret-redacted [`SseEvent`]s.
    pub fn push_bytes(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.byte_buf.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some(pos) = self.byte_buf.iter().position(|&b| b == b'\n') {
            let mut line_bytes: Vec<u8> = self.byte_buf.drain(..=pos).collect();
            if line_bytes.ends_with(b"\n") {
                line_bytes.pop();
            }
            if line_bytes.ends_with(b"\r") {
                line_bytes.pop();
            }
            let raw_line = String::from_utf8_lossy(&line_bytes);
            if raw_line.is_empty() {
                if let Some(ev) = self.flush_current_event() {
                    events.push(ev);
                }
                continue;
            }
            if raw_line.starts_with(':') {
                continue;
            }
            if let Some(ev_name) = raw_line.strip_prefix("event:") {
                self.current_event = Some(ev_name.trim().to_string());
            } else if let Some(data_part) = raw_line.strip_prefix("data:") {
                let trimmed = data_part.strip_prefix(' ').unwrap_or(data_part);
                self.current_data.push(trimmed.to_string());
            }
        }
        events
    }

    /// Flush any remaining buffered SSE event at end-of-stream.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.byte_buf.is_empty() {
            let tail = std::mem::take(&mut self.byte_buf);
            let line = String::from_utf8_lossy(&tail);
            if let Some(data_part) = line.strip_prefix("data:") {
                let trimmed = data_part.strip_prefix(' ').unwrap_or(data_part);
                self.current_data.push(trimmed.to_string());
            }
        }
        self.flush_current_event()
    }

    fn flush_current_event(&mut self) -> Option<SseEvent> {
        if self.current_data.is_empty() && self.current_event.is_none() {
            return None;
        }
        let raw = self.current_data.join("\n");
        let data = self.redactor.redact_str(&raw);
        self.current_data.clear();
        let event = self.current_event.take();
        Some(SseEvent { event, data })
    }
}

/// Shared `reqwest` HTTP/SSE client (`rustls-tls`) reused by all [`ModelProvider`]
/// adapters with built-in error classification, cancellation, and SSE parsing.
#[derive(Debug, Clone)]
pub struct SharedHttpClient {
    inner: reqwest::Client,
}

impl SharedHttpClient {
    /// Construct a [`SharedHttpClient`] with sensible connect/request defaults.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Http`] if TLS/client initialization fails.
    pub fn new(timeout_secs: u64) -> Result<Self, RuntimeError> {
        let inner = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(timeout_secs))
            .build()?;
        Ok(Self { inner })
    }

    /// Borrow the underlying `reqwest::Client`.
    #[must_use]
    pub fn client(&self) -> &reqwest::Client {
        &self.inner
    }

    /// Execute a JSON HTTP POST request with cancellation and standardized
    /// [`ProviderErrorKind`] classification.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ProviderError`] on HTTP error statuses or
    /// cancellation, or [`RuntimeError::Http`] on transport failure.
    pub async fn post_json(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &Value,
        cancel: CancellationToken,
    ) -> Result<Value, RuntimeError> {
        let mut req = self.inner.post(url).json(body);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = tokio::select! {
            res = req.send() => res?,
            () = cancel.cancelled() => {
                return Err(RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Retryable,
                    message: "HTTP request cancelled".to_string(),
                });
            }
        };
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(RuntimeError::ProviderError {
                kind: classify_http_error(status.as_u16(), &text),
                message: format!("HTTP {}: {text}", status.as_u16()),
            });
        }
        Ok(serde_json::from_str(&text)?)
    }

    /// Execute a streaming SSE HTTP POST request, feeding each parsed,
    /// secret-redacted [`SseEvent`] into `on_event` while honoring `cancel`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::ProviderError`] on HTTP error status or cancellation.
    pub async fn post_sse<F>(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &Value,
        redactor: StreamRedactor,
        cancel: CancellationToken,
        mut on_event: F,
    ) -> Result<(), RuntimeError>
    where
        F: FnMut(SseEvent) -> Result<(), RuntimeError>,
    {
        let mut req = self.inner.post(url).json(body);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        let mut resp = tokio::select! {
            res = req.send() => res?,
            () = cancel.cancelled() => {
                return Err(RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Retryable,
                    message: "SSE request cancelled".to_string(),
                });
            }
        };
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(RuntimeError::ProviderError {
                kind: classify_http_error(status.as_u16(), &text),
                message: format!("HTTP {}: {text}", status.as_u16()),
            });
        }

        let mut parser = SseLineParser::new(redactor);
        loop {
            let chunk_opt = tokio::select! {
                chunk = resp.chunk() => chunk?,
                () = cancel.cancelled() => {
                    return Err(RuntimeError::ProviderError {
                        kind: ProviderErrorKind::Retryable,
                        message: "SSE stream cancelled".to_string(),
                    });
                }
            };
            let Some(chunk) = chunk_opt else {
                break;
            };
            for ev in parser.push_bytes(&chunk) {
                on_event(ev)?;
            }
        }
        if let Some(ev) = parser.finish() {
            on_event(ev)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct MockProvider;
    impl ModelProvider for MockProvider {
        fn id(&self) -> &str {
            "mock_prov"
        }
        fn capabilities(&self, _model: &str) -> ModelCapabilities {
            ModelCapabilities::default()
        }
        fn invoke<'a>(
            &'a self,
            req: &'a ModelRequest,
            _ctx: &'a ProviderContext<'a>,
            _on_delta: StreamDeltaFn<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>>
        {
            Box::pin(async move {
                Ok(ModelResponse {
                    kind: req.kind,
                    output: json!({"ok": true}),
                    usage: Usage::default(),
                })
            })
        }
    }

    #[test]
    fn target_resolution_supports_custom_aliases_and_micro_usd_cost() {
        let mut reg = ProviderRegistry::new();
        reg.register_provider(Arc::new(MockProvider));
        let mut cfgs = BTreeMap::new();
        cfgs.insert(
            "fast_alias".to_string(),
            ProviderConfig {
                adapter: Some("mock_prov".to_string()),
                endpoint: Some("https://custom.local/v1".to_string()),
                model: "fast-m1".to_string(),
                input_cost_per_mtok_usd: Some(3.0),
                output_cost_per_mtok_usd: Some(15.0),
                env_key: None,
                extra: BTreeMap::new(),
            },
        );
        reg.merge_configs(cfgs);

        let resolved = reg.resolve_target("fast_alias").expect("custom alias");
        assert_eq!(resolved.provider.id(), "mock_prov");
        assert_eq!(resolved.model, "fast-m1");
        assert_eq!(
            compute_cost_micro_usd(1_000, 500, resolved.config.as_ref()),
            10_500
        );

        let explicit = reg
            .resolve_target("fast_alias/override-m2")
            .expect("alias with explicit model");
        assert_eq!(explicit.provider.id(), "mock_prov");
        assert_eq!(explicit.model, "override-m2");
    }

    #[test]
    fn sse_parser_redacts_split_secret_and_classifies_http_errors() {
        let redactor = StreamRedactor::from_secrets(vec!["secret_token_999".to_string()]);
        let mut parser = SseLineParser::new(redactor);
        let mut events = Vec::new();
        events.extend(parser.push_bytes(b"event: delta\ndata: {\"k\":\"secret_"));
        events.extend(parser.push_bytes(b"token_999\"}\n\n"));
        if let Some(ev) = parser.finish() {
            events.push(ev);
        }
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("delta"));
        assert_eq!(events[0].data, "{\"k\":\"[REDACTED]\"}");

        assert_eq!(classify_http_error(429, ""), ProviderErrorKind::Retryable);
        assert_eq!(classify_http_error(529, ""), ProviderErrorKind::Retryable);
        assert_eq!(
            classify_http_error(400, "prompt is too long"),
            ProviderErrorKind::ContextLengthExceeded
        );
        assert_eq!(classify_http_error(401, ""), ProviderErrorKind::AuthError);
    }
}
