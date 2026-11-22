#![forbid(unsafe_code)]
//! Bounded HTTP/1.1 webhook listener (`/webhook`) trigger handler.

use std::net::SocketAddr;

use gestalt_runtime::{
    CancellationToken, RuntimeError, TriggerContext, TriggerEvent, TriggerHandler,
    TriggerInstanceConfig, current_epoch_ms,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const MAX_WEBHOOK_REQUEST_BYTES: usize = 64 * 1024;

fn parse_bind_and_path(config: &Value) -> Result<(SocketAddr, String, bool), RuntimeError> {
    let bind_str = config
        .get("bind_addr")
        .and_then(Value::as_str)
        .unwrap_or("127.0.0.1:0");
    let addr: SocketAddr = bind_str.parse().map_err(|err| {
        RuntimeError::ArtifactError(format!("invalid webhook bind_addr '{bind_str}': {err}"))
    })?;
    let allow_external = config
        .get("allow_external")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !addr.ip().is_loopback() && !allow_external {
        return Err(RuntimeError::SandboxViolation(format!(
            "webhook bind_addr '{addr}' is non-loopback; set allow_external=true explicitly to expose"
        )));
    }
    let path = config
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("/webhook")
        .to_string();
    if !path.starts_with('/') {
        return Err(RuntimeError::ArtifactError(format!(
            "webhook path '{path}' must start with '/'"
        )));
    }
    Ok((addr, path, allow_external))
}

/// Bounded localhost HTTP/1.1 webhook listener (`/webhook`) [`TriggerHandler`].
#[derive(Debug, Clone, Default)]
pub struct WebhookTriggerHandler;

impl TriggerHandler for WebhookTriggerHandler {
    fn kind(&self) -> &str {
        "webhook"
    }

    fn description(&self) -> &str {
        "Bounded HTTP/1.1 webhook listener trigger (/webhook)"
    }

    fn validate(&self, config: &Value) -> Result<(), RuntimeError> {
        let _ = parse_bind_and_path(config)?;
        Ok(())
    }

    fn start(
        &self,
        instance: TriggerInstanceConfig,
        ctx: TriggerContext,
        event_tx: mpsc::UnboundedSender<TriggerEvent>,
        cancel: CancellationToken,
    ) -> Result<JoinHandle<()>, RuntimeError> {
        let (addr, expected_path, _) = parse_bind_and_path(&instance.config)?;
        let std_listener = std::net::TcpListener::bind(addr)?;
        std_listener.set_nonblocking(true)?;
        let local_addr = std_listener.local_addr()?;
        let listener = TcpListener::from_std(std_listener)?;

        let expected_secret = instance
            .config
            .get("secret")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .or_else(|| {
                let cred_key = instance
                    .config
                    .get("credential_key")
                    .and_then(Value::as_str)
                    .unwrap_or("webhook");
                let env_key = instance.config.get("secret_env").and_then(Value::as_str);
                ctx.credentials.get(cred_key, env_key)
            });

        Ok(tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    accept_res = listener.accept() => {
                        let Ok((mut stream, _peer)) = accept_res else {
                            break;
                        };
                        let mut buf = vec![0_u8; MAX_WEBHOOK_REQUEST_BYTES + 1];
                        let n = match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => continue,
                            Ok(n) => n,
                        };
                        if n > MAX_WEBHOOK_REQUEST_BYTES {
                            let _ = write_http_resp(&mut stream, "413 Payload Too Large", r#"{"error":"payload too large"}"#).await;
                            continue;
                        }
                        let raw = String::from_utf8_lossy(&buf[..n]);
                        let Some((head, body)) = raw.split_once("\r\n\r\n").or_else(|| raw.split_once("\n\n")) else {
                            let _ = write_http_resp(&mut stream, "400 Bad Request", r#"{"error":"malformed http"}"#).await;
                            continue;
                        };
                        let mut lines = head.lines();
                        let req_line = lines.next().unwrap_or("");
                        let mut parts = req_line.split_whitespace();
                        let method = parts.next().unwrap_or("");
                        let req_path = parts.next().unwrap_or("");
                        if method != "POST" || req_path != expected_path {
                            let _ = write_http_resp(&mut stream, "404 Not Found", r#"{"error":"not found"}"#).await;
                            continue;
                        }

                        if let Some(ref secret) = expected_secret {
                            let auth_ok = lines.any(|hdr| {
                                if let Some((k, v)) = hdr.split_once(':') {
                                    let kl = k.trim().to_ascii_lowercase();
                                    let vt = v.trim();
                                    (kl == "authorization" && vt.strip_prefix("Bearer ").is_some_and(|tok| tok == secret))
                                        || (kl == "x-gestalt-secret" && vt == secret)
                                } else {
                                    false
                                }
                            });
                            if !auth_ok {
                                let _ = write_http_resp(&mut stream, "401 Unauthorized", r#"{"error":"unauthorized"}"#).await;
                                continue;
                            }
                        }

                        let mut payload: Value = serde_json::from_str(body.trim())
                            .unwrap_or_else(|_| Value::String(body.trim().to_string()));
                        ctx.redactor.redact_json(&mut payload);

                        let ev = TriggerEvent {
                            trigger_id: instance.id.clone(),
                            kind: instance.kind.clone(),
                            prompt: instance.prompt.clone(),
                            preset: instance.preset.clone(),
                            extra_decorators: instance.extra_decorators.clone(),
                            metadata: json!({
                                "listen_addr": local_addr.to_string(),
                                "path": expected_path,
                                "payload": payload,
                            }),
                            timestamp_ms: current_epoch_ms(),
                        };
                        let _ = event_tx.send(ev);
                        let resp_body = json!({
                            "queued": true,
                            "trigger_id": instance.id,
                        })
                        .to_string();
                        let _ = write_http_resp(&mut stream, "202 Accepted", &resp_body).await;
                    }
                }
            }
        }))
    }
}

async fn write_http_resp(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    json_body: &str,
) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
        json_body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_runtime::sandbox::WorkspacePathGuard;
    use gestalt_runtime::{CredentialStore, SharedHttpClient};
    use std::collections::BTreeSet;
    use std::time::Duration;

    #[tokio::test]
    async fn webhook_trigger_validates_loopback_enforces_auth_and_emits_redacted_event() {
        let dir = std::env::temp_dir();
        let mut creds = CredentialStore::default();
        creds.insert_runtime_secret("api", "super_secret_token_999");
        let ctx = TriggerContext {
            workspace_root: dir.clone(),
            path_guard: WorkspacePathGuard::new(&dir, &BTreeSet::new()).expect("guard"),
            credentials: creds.clone(),
            http: SharedHttpClient::new(5).expect("http"),
            redactor: creds.redactor(),
        };

        // Non-loopback without allow_external fails closed
        assert!(matches!(
            WebhookTriggerHandler.validate(&json!({ "bind_addr": "0.0.0.0:8080" })),
            Err(RuntimeError::SandboxViolation(_))
        ));

        // Pick an ephemeral free port to connect to
        let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe");
        let port = probe.local_addr().expect("addr").port();
        drop(probe);
        let bind_addr = format!("127.0.0.1:{port}");

        let (tx, mut rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let handle = WebhookTriggerHandler
            .start(
                TriggerInstanceConfig {
                    id: "wh_1".into(),
                    kind: "webhook".into(),
                    enabled: true,
                    prompt: json!("handle webhook"),
                    preset: None,
                    extra_decorators: Vec::new(),
                    config: json!({
                        "bind_addr": bind_addr,
                        "path": "/hook",
                        "secret": "hook_tok",
                    }),
                },
                ctx,
                tx,
                cancel.clone(),
            )
            .expect("start webhook");

        let shared_http = SharedHttpClient::new(5).expect("http");
        let client = shared_http.client();
        let url = format!("http://127.0.0.1:{port}/hook");

        // Unauthorized request -> 401
        let unauth = client
            .post(&url)
            .json(&json!({"event":"push"}))
            .send()
            .await
            .expect("unauth");
        assert_eq!(unauth.status(), 401);

        // Authorized request with secret in body -> 202 + redacted event
        let ok = client
            .post(&url)
            .header("Authorization", "Bearer hook_tok")
            .json(&json!({"event": "push", "leak": "super_secret_token_999"}))
            .send()
            .await
            .expect("auth");
        assert_eq!(ok.status(), 202);

        let ev = tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .expect("timeout")
            .expect("event");
        assert_eq!(ev.trigger_id, "wh_1");
        assert!(!ev.metadata.to_string().contains("super_secret_token_999"));

        cancel.cancel();
        let _ = handle.await;
    }
}
