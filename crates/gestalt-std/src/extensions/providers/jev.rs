#![forbid(unsafe_code)]
//! TypeSafe AI Jev System One (`ModelKind::Decision`) provider adapter (`"jev"`).

use std::future::Future;
use std::pin::Pin;

use gestalt_kernel::{ModelCapabilities, ModelKind, Usage};
use gestalt_runtime::{
    ModelProvider, ModelRequest, ModelResponse, ProviderContext, ProviderErrorKind, RuntimeError,
    StreamDeltaFn, compute_cost_micro_usd, estimate_tokens_fallback,
};
use serde_json::{Value, json};

use crate::extensions::providers::resolve_auth_headers;
use crate::extensions::{DecisionInput, DecisionOutput};

const DEFAULT_JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/decision";

/// [`ModelProvider`] adapter for `ModelKind::Decision` (`"jev"`).
#[derive(Debug, Clone, Default)]
pub struct JevProvider;

impl ModelProvider for JevProvider {
    fn id(&self) -> &str {
        "jev"
    }

    fn capabilities(&self, _model: &str) -> ModelCapabilities {
        ModelCapabilities {
            max_context_tokens: 32_768,
            max_output_tokens: 512,
            supports_prompt_caching: false,
            supports_reasoning: false,
            supports_vision: false,
        }
    }

    fn invoke<'a>(
        &'a self,
        req: &'a ModelRequest,
        ctx: &'a ProviderContext<'a>,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            if req.kind != ModelKind::Decision {
                return Err(RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Fatal,
                    message: format!(
                        "jev provider only supports ModelKind::Decision, got {:?}",
                        req.kind
                    ),
                });
            }

            let endpoint = req
                .config
                .as_ref()
                .and_then(|c| c.endpoint.as_deref())
                .unwrap_or(DEFAULT_JEV_ENDPOINT);

            let mut headers = resolve_auth_headers(req, ctx, "Authorization", true).await?;
            headers.insert("content-type".to_string(), "application/json".to_string());

            let parsed_input: DecisionInput = serde_json::from_value(req.input.clone())
                .unwrap_or_else(|_| DecisionInput {
                    state: req.input.clone(),
                    choices: Vec::new(),
                    rubric: None,
                });
            let body = json!({
                "model": req.model,
                "state": parsed_input.state,
                "choices": parsed_input.choices,
                "rubric": parsed_input.rubric,
            });

            let resp_json = ctx
                .http
                .post_json(endpoint, &headers, &body, ctx.cancel.clone())
                .await?;

            let choice = resp_json
                .get("choice")
                .and_then(Value::as_str)
                .ok_or_else(|| RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Fatal,
                    message: format!("jev response missing 'choice': {resp_json}"),
                })?
                .to_string();
            let confidence = resp_json
                .get("confidence")
                .and_then(Value::as_f64)
                .ok_or_else(|| RuntimeError::ProviderError {
                    kind: ProviderErrorKind::Fatal,
                    message: format!("jev response missing 'confidence': {resp_json}"),
                })?;
            let latency_ms = resp_json
                .get("latency_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0);

            let input_units = resp_json
                .pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_else(|| estimate_tokens_fallback(&body.to_string()));
            let output_units = resp_json
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(16);
            let cost_micro_usd =
                compute_cost_micro_usd(input_units, output_units, req.config.as_ref());

            let out = DecisionOutput {
                choice,
                confidence,
                latency_ms,
            };
            Ok(ModelResponse {
                kind: ModelKind::Decision,
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

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_runtime::{
        BlobStore, CancellationToken, CredentialStore, ProviderConfig, SharedHttpClient,
        StreamRedactor,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn jev_provider_posts_decision_payload_and_parses_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");

        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = sock.read(&mut buf).await;
                let body = r#"{"choice":"run_tests","confidence":0.93,"latency_ms":41,"usage":{"input_tokens":100,"output_tokens":10}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });

        let mut creds = CredentialStore::default();
        creds.insert_runtime_secret("jev", "jev_test_token_12345");
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
            provider_id: "jev".into(),
            model: "jev-1.13.0".into(),
            kind: ModelKind::Decision,
            input: json!({
                "state": { "prompt": "run tests" },
                "choices": [{ "id": "run_tests", "description": "Run tests" }],
                "rubric": "Pick run_tests"
            }),
            schema: None,
            config: Some(ProviderConfig {
                endpoint: Some(format!("http://{addr}/v1/decision")),
                model: "jev-1.13.0".into(),
                input_cost_per_mtok_usd: Some(1.0),
                output_cost_per_mtok_usd: Some(2.0),
                ..ProviderConfig::default()
            }),
        };

        let noop = |_: &str, _: Value| {};
        let resp = JevProvider
            .invoke(&req, &pctx, &noop)
            .await
            .expect("jev invoke");
        assert_eq!(resp.output["choice"], "run_tests");
        assert_eq!(resp.output["confidence"], 0.93);
        assert_eq!(resp.usage.input_units, 100);
        assert_eq!(resp.usage.cost_micro_usd, 120);
    }
}
