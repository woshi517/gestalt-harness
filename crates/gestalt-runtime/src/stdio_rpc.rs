#![forbid(unsafe_code)]
//! Shared sandboxed stdio JSON-RPC 2.0 transport reused by `ext_ipc` and `mcp`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use gestalt_kernel::SandboxPolicy;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::config::ExternalExtensionConfig;
use crate::credentials::StreamRedactor;
use crate::sandbox::{SandboxCommand, SandboxDriver, build_clean_env};
use crate::{CancellationToken, RuntimeError};

/// Action returned by an incoming-message hook while awaiting a JSON-RPC response.
pub(crate) enum IncomingControl {
    /// Continue waiting for the correlated response ID.
    Continue,
}

/// Sandboxed child-process newline-delimited JSON-RPC 2.0 transport.
pub(crate) struct StdioJsonRpcTransport {
    name: String,
    timeout_ms: u64,
    redactor: StreamRedactor,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl StdioJsonRpcTransport {
    /// Spawn a child process inside `sandbox_driver` with `env_clear()`,
    /// process-group isolation, and redacted `stderr` logging.
    pub(crate) fn spawn(
        name: String,
        config: &ExternalExtensionConfig,
        workspace_root: &Path,
        sandbox_policy: &SandboxPolicy,
        sandbox_driver: &Arc<dyn SandboxDriver>,
        redactor: StreamRedactor,
    ) -> Result<Self, RuntimeError> {
        let Some((program, args)) = config.command.split_first() else {
            return Err(RuntimeError::IpcProtocol {
                ext_name: name,
                message: "external command cannot be empty".to_string(),
            });
        };

        let sb_cmd = SandboxCommand {
            program: program.clone(),
            args: args.to_vec(),
            cwd: workspace_root.to_path_buf(),
            env: BTreeMap::new(),
            stdin_bytes: None,
            timeout_ms: None,
        };
        let (exec_prog, exec_args) =
            sandbox_driver.build_os_command(&sb_cmd, workspace_root, sandbox_policy)?;

        let mut command = Command::new(&exec_prog);
        command
            .args(&exec_args)
            .current_dir(workspace_root)
            .env_clear()
            .envs(build_clean_env(&BTreeMap::new()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(unix)]
        {
            command.process_group(0);
        }

        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::IpcProtocol {
                ext_name: name.clone(),
                message: "failed to capture child stdin".to_string(),
            })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::IpcProtocol {
                ext_name: name.clone(),
                message: "failed to capture child stdout".to_string(),
            })?;

        if let Some(stderr) = child.stderr.take() {
            let worker_name = name.clone();
            let err_redactor = redactor.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let clean = err_redactor.redact_str(&line);
                    tracing::warn!(ext = %worker_name, "{clean}");
                }
            });
        }

        Ok(Self {
            name,
            timeout_ms: config.timeout_ms.max(1),
            redactor,
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn redactor(&self) -> &StreamRedactor {
        &self.redactor
    }

    pub(crate) fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Write a single newline-delimited JSON-RPC 2.0 frame to child stdin.
    pub(crate) async fn send_json(&mut self, msg: &Value) -> Result<(), RuntimeError> {
        let Some(ref mut stdin) = self.stdin else {
            return Err(RuntimeError::IpcProtocol {
                ext_name: self.name.clone(),
                message: "child stdin is already closed".to_string(),
            });
        };
        let encoded = serde_json::to_string(msg)?;
        stdin.write_all(encoded.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Send a JSON-RPC 2.0 notification (no `id`).
    pub(crate) async fn send_notification(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(), RuntimeError> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.send_json(&msg).await
    }

    /// Send a JSON-RPC 2.0 request and wait for its correlated response,
    /// dispatching any interleaved notifications or server-initiated requests
    /// (such as MCP `ping`) through `on_other`.
    pub(crate) async fn call_rpc<F>(
        &mut self,
        method: &str,
        params: Value,
        cancel: CancellationToken,
        kill_on_cancel: bool,
        mut on_other: F,
    ) -> Result<Value, RuntimeError>
    where
        F: FnMut(&Value) -> (Option<Value>, IncomingControl),
    {
        let req_id = self.allocate_id();
        let request = json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params,
        });
        self.send_json(&request).await?;

        let deadline = tokio::time::sleep(Duration::from_millis(self.timeout_ms));
        tokio::pin!(deadline);

        let mut line = String::new();
        loop {
            line.clear();
            tokio::select! {
                () = &mut deadline => {
                    return Err(RuntimeError::IpcTimeout {
                        ext_name: self.name.clone(),
                        method: method.to_string(),
                        timeout_ms: self.timeout_ms,
                    });
                }
                () = cancel.cancelled() => {
                    if kill_on_cancel {
                        let _ = self.child.start_kill();
                    } else {
                        // Send MCP-compliant cancellation notification without killing server.
                        let _ = self.send_notification(
                            "notifications/cancelled",
                            json!({ "requestId": req_id, "reason": "cancelled by client" }),
                        ).await;
                    }
                    return Err(RuntimeError::IpcProtocol {
                        ext_name: self.name.clone(),
                        message: format!("IPC call '{method}' cancelled"),
                    });
                }
                read_res = self.stdout.read_line(&mut line) => {
                    let bytes = read_res?;
                    if bytes == 0 {
                        return Err(RuntimeError::IpcProtocol {
                            ext_name: self.name.clone(),
                            message: format!("process closed stdout while awaiting '{method}'"),
                        });
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let msg: Value = serde_json::from_str(trimmed).map_err(|err| {
                        RuntimeError::IpcProtocol {
                            ext_name: self.name.clone(),
                            message: format!("invalid JSON-RPC frame: {err}"),
                        }
                    })?;

                    if msg.get("id").and_then(Value::as_u64) == Some(req_id)
                        && msg.get("method").is_none()
                    {
                        if let Some(err_obj) = msg.get("error") {
                            return Err(RuntimeError::IpcProtocol {
                                ext_name: self.name.clone(),
                                message: format!("JSON-RPC error: {err_obj}"),
                            });
                        }
                        return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                    }

                    let (reply, IncomingControl::Continue) = on_other(&msg);
                    if let Some(reply_msg) = reply {
                        let _ = self.send_json(&reply_msg).await;
                    }
                }
            }
        }
    }

    /// Close child `stdin` (signaling EOF per MCP stdio shutdown spec), wait up
    /// to `grace_ms` for clean exit, then fall back to `start_kill()`.
    pub(crate) async fn close_stdin_and_shutdown(&mut self, grace_ms: u64) {
        self.stdin.take();
        if tokio::time::timeout(Duration::from_millis(grace_ms), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_millis(100), self.child.wait()).await;
        }
    }
}
