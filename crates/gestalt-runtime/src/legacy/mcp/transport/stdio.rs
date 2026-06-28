use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::mcp_error::{McpError, Result};
use crate::transport::McpTransport;

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct JsonRpcErrorDetail {
    code: i64,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[allow(clippy::type_complexity)]
pub struct StdioTransport {
    child: Arc<Mutex<Option<Child>>>,
    tx_request: mpsc::Sender<(Value, oneshot::Sender<std::result::Result<Value, String>>)>,
    rx_notification: Arc<Mutex<mpsc::Receiver<(String, Option<Value>)>>>,
    next_id: AtomicU64,
}

impl StdioTransport {
    pub async fn spawn(
        server_name: &str,
        command: &str,
        args: &[String],
        cwd: Option<&str>,
        custom_env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.args(args);
        cmd.env_clear();

        // Safe baseline environment variables
        let safe_envs = [
            "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TERM", "LANG", "LC_ALL", "LC_CTYPE",
            "TMPDIR", "TEMP", "TMP",
        ];
        for var in &safe_envs {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        // Add explicit custom env variables (redacted from traces/diagnostics internally)
        for (k, v) in custom_env {
            cmd.env(k, v);
        }

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| {
            McpError::Initialization(format!(
                "Failed to spawn MCP server '{}': {}",
                server_name, e
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Initialization("Failed to open child stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Initialization("Failed to open child stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| McpError::Initialization("Failed to open child stderr".to_string()))?;

        let (tx_request, mut rx_request) =
            mpsc::channel::<(Value, oneshot::Sender<std::result::Result<Value, String>>)>(100);
        let (tx_notification, rx_notification) = mpsc::channel::<(String, Option<Value>)>(100);

        let child_arc = Arc::new(Mutex::new(Some(child)));
        let child_clone = child_arc.clone();
        let server_name_str = server_name.to_string();

        // Stderr reading thread
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                // Log stderr message safely
                eprintln!("[MCP Server '{}' stderr] {}", server_name_str, line.trim());
                line.clear();
            }
        });

        // Stdin/Stdout JSON-RPC handling thread
        let child_clone2 = child_arc.clone();
        let server_name_str2 = server_name.to_string();
        tokio::spawn(async move {
            let mut stdin = stdin;
            let mut stdout_reader = BufReader::new(stdout);
            let mut pending: HashMap<String, oneshot::Sender<std::result::Result<Value, String>>> =
                HashMap::new();
            let mut line = String::new();

            loop {
                tokio::select! {
                    req_opt = rx_request.recv() => {
                        match req_opt {
                            Some((msg, resp_tx)) => {
                                let id_str = msg.get("id")
                                    .and_then(|v| v.as_str().map(|s| s.to_string()).or_else(|| v.as_i64().map(|i| i.to_string())))
                                    .unwrap_or_default();

                                if !id_str.is_empty() {
                                    pending.insert(id_str.clone(), resp_tx);
                                }

                                let mut bytes = serde_json::to_vec(&msg).unwrap_or_default();
                                bytes.push(b'\n');
                                if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
                                    if !id_str.is_empty() {
                                        if let Some(tx) = pending.remove(&id_str) {
                                            let _ = tx.send(Err("Failed to write to stdin".to_string()));
                                        }
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                    read_res = stdout_reader.read_line(&mut line) => {
                        match read_res {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {
                                let parsed: serde_json::Result<Value> = serde_json::from_str(&line);
                                if let Ok(val) = parsed {
                                    if let Some(id_val) = val.get("id") {
                                        let id_str = id_val.as_str().map(|s| s.to_string())
                                            .or_else(|| id_val.as_i64().map(|i| i.to_string()))
                                            .unwrap_or_default();
                                        if let Some(tx) = pending.remove(&id_str) {
                                            let _ = tx.send(Ok(val));
                                        }
                                    } else if let Some(method_val) = val.get("method") {
                                        // It is a notification from the server
                                        if let Some(method) = method_val.as_str() {
                                            let params = val.get("params").cloned();
                                            let _ = tx_notification.send((method.to_string(), params)).await;
                                        }
                                    }
                                }
                                line.clear();
                            }
                        }
                    }
                }
            }

            for (_, tx) in pending {
                let _ = tx.send(Err("Server exited or pipe closed".to_string()));
            }

            let mut lock = child_clone2.lock().await;
            if let Some(mut child) = lock.take() {
                let _ = child.kill().await;
                let _ = child.wait().await;
                eprintln!("[MCP Server '{}'] Process terminated.", server_name_str2);
            }
        });

        Ok(Self {
            child: child_clone,
            tx_request,
            rx_notification: Arc::new(Mutex::new(rx_notification)),
            next_id: AtomicU64::new(1),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn call(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id_num = self.next_id.fetch_add(1, Ordering::SeqCst);
        let id_val = Value::Number(id_num.into());
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id: Some(id_val.clone()),
        };

        let req_val = serde_json::to_value(&req)
            .map_err(|e| McpError::Protocol(format!("Failed to serialize request: {}", e)))?;

        let (resp_tx, resp_rx) = oneshot::channel();
        if self.tx_request.send((req_val, resp_tx)).await.is_err() {
            return Err(McpError::Transport(
                "Failed to send request to backend handler".to_string(),
            ));
        }

        // 30 seconds timeout
        let result = tokio::time::timeout(Duration::from_secs(30), resp_rx).await;
        match result {
            Ok(Ok(Ok(val))) => {
                // Parse response
                if let Some(err_val) = val.get("error") {
                    if let Ok(err) = serde_json::from_value::<JsonRpcErrorDetail>(err_val.clone()) {
                        return Err(McpError::Execution(format!(
                            "Server returned error [{}]: {}",
                            err.code, err.message
                        )));
                    }
                }
                Ok(val.get("result").cloned().unwrap_or(Value::Null))
            }
            Ok(Ok(Err(e))) => Err(McpError::Execution(e)),
            Ok(Err(_)) => Err(McpError::Transport(
                "Response channel closed before receiving result".to_string(),
            )),
            Err(_) => Err(McpError::Timeout(
                "Request timed out after 30 seconds".to_string(),
            )),
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id: None,
        };

        let req_val = serde_json::to_value(&req)
            .map_err(|e| McpError::Protocol(format!("Failed to serialize notification: {}", e)))?;

        let (resp_tx, _) = oneshot::channel();
        self.tx_request
            .send((req_val, resp_tx))
            .await
            .map_err(|_| {
                McpError::Transport("Failed to send notification to backend handler".to_string())
            })?;

        Ok(())
    }

    async fn recv_notification(&self) -> Option<(String, Option<Value>)> {
        let mut guard = self.rx_notification.lock().await;
        guard.recv().await
    }

    async fn shutdown(&self) {
        let mut lock = self.child.lock().await;
        if let Some(mut child) = lock.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}
