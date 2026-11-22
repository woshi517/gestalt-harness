#![forbid(unsafe_code)]
//! Shared sandboxed stdio JSON-RPC 2.0 transport reused by `ext_ipc` and `mcp`.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use gestalt_kernel::SandboxPolicy;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::config::ExternalExtensionConfig;
use crate::credentials::StreamRedactor;
use crate::sandbox::{SandboxCommand, SandboxDriver, build_clean_env};
use crate::{CancellationToken, RuntimeError};

/// Action returned by an incoming-message hook while awaiting a JSON-RPC response.
pub(crate) enum IncomingControl {
    /// Continue waiting for the correlated response ID.
    Continue,
}

type PendingMap = Arc<StdMutex<BTreeMap<u64, oneshot::Sender<Result<Value, RuntimeError>>>>>;
type ChannelSlot = Arc<StdMutex<Option<mpsc::UnboundedSender<Value>>>>;

/// Sandboxed child-process newline-delimited JSON-RPC 2.0 transport with
/// background stdout demultiplexing for responses and unsolicited notifications.
pub(crate) struct StdioJsonRpcTransport {
    name: String,
    timeout_ms: u64,
    redactor: StreamRedactor,
    child: Child,
    stdin: Option<ChildStdin>,
    pending: PendingMap,
    active_other_tx: ChannelSlot,
    unsolicited_tx: ChannelSlot,
    reader_task: Option<JoinHandle<()>>,
    next_id: u64,
}

impl Drop for StdioJsonRpcTransport {
    fn drop(&mut self) {
        if let Some(handle) = self.reader_task.take() {
            handle.abort();
        }
    }
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

        let pending: PendingMap = Arc::new(StdMutex::new(BTreeMap::new()));
        let active_other_tx: ChannelSlot = Arc::new(StdMutex::new(None));
        let unsolicited_tx: ChannelSlot = Arc::new(StdMutex::new(None));

        let reader_name = name.clone();
        let reader_pending = Arc::clone(&pending);
        let reader_other = Arc::clone(&active_other_tx);
        let reader_unsolicited = Arc::clone(&unsolicited_tx);
        let reader_task = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        fail_all_pending(
                            &reader_pending,
                            &reader_name,
                            "process closed stdout".to_string(),
                        );
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        let msg: Value = match serde_json::from_str(trimmed) {
                            Ok(v) => v,
                            Err(err) => {
                                fail_all_pending(
                                    &reader_pending,
                                    &reader_name,
                                    format!("invalid JSON-RPC frame: {err}"),
                                );
                                break;
                            }
                        };
                        if let Some(req_id) = msg.get("id").and_then(Value::as_u64)
                            && msg.get("method").is_none()
                        {
                            let tx = reader_pending
                                .lock()
                                .ok()
                                .and_then(|mut map| map.remove(&req_id));
                            if let Some(tx) = tx {
                                if let Some(err_obj) = msg.get("error") {
                                    let _ = tx.send(Err(RuntimeError::IpcProtocol {
                                        ext_name: reader_name.clone(),
                                        message: format!("JSON-RPC error: {err_obj}"),
                                    }));
                                } else {
                                    let _ = tx.send(Ok(msg
                                        .get("result")
                                        .cloned()
                                        .unwrap_or(Value::Null)));
                                }
                            }
                            continue;
                        }

                        let is_trigger =
                            msg.get("method").and_then(Value::as_str) == Some("trigger/fire");
                        if is_trigger {
                            if let Ok(guard) = reader_unsolicited.lock()
                                && let Some(ref tx) = *guard
                            {
                                let _ = tx.send(msg);
                            }
                            continue;
                        }

                        let forwarded = if let Ok(guard) = reader_other.lock()
                            && let Some(ref tx) = *guard
                        {
                            tx.send(msg.clone()).is_ok()
                        } else {
                            false
                        };
                        if !forwarded
                            && let Ok(guard) = reader_unsolicited.lock()
                            && let Some(ref tx) = *guard
                        {
                            let _ = tx.send(msg);
                        }
                    }
                    Err(err) => {
                        fail_all_pending(
                            &reader_pending,
                            &reader_name,
                            format!("stdout read error: {err}"),
                        );
                        break;
                    }
                }
            }
        });

        Ok(Self {
            name,
            timeout_ms: config.timeout_ms.max(1),
            redactor,
            child,
            stdin: Some(stdin),
            pending,
            active_other_tx,
            unsolicited_tx,
            reader_task: Some(reader_task),
            next_id: 1,
        })
    }

    pub(crate) fn set_unsolicited_sender(&self, tx: Option<mpsc::UnboundedSender<Value>>) {
        if let Ok(mut guard) = self.unsolicited_tx.lock() {
            *guard = tx;
        }
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
        let (resp_tx, mut resp_rx) = oneshot::channel();
        if let Ok(mut map) = self.pending.lock() {
            map.insert(req_id, resp_tx);
        }
        let (other_tx, mut other_rx) = mpsc::unbounded_channel();
        if let Ok(mut guard) = self.active_other_tx.lock() {
            *guard = Some(other_tx);
        }

        let request = json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params,
        });
        if let Err(err) = self.send_json(&request).await {
            if let Ok(mut map) = self.pending.lock() {
                map.remove(&req_id);
            }
            if let Ok(mut guard) = self.active_other_tx.lock() {
                *guard = None;
            }
            return Err(err);
        }

        let deadline = tokio::time::sleep(Duration::from_millis(self.timeout_ms));
        tokio::pin!(deadline);

        let result = loop {
            tokio::select! {
                biased;
                Some(msg) = other_rx.recv() => {
                    let (reply, IncomingControl::Continue) = on_other(&msg);
                    if let Some(reply_msg) = reply {
                        let _ = self.send_json(&reply_msg).await;
                    }
                }
                () = &mut deadline => {
                    break Err(RuntimeError::IpcTimeout {
                        ext_name: self.name.clone(),
                        method: method.to_string(),
                        timeout_ms: self.timeout_ms,
                    });
                }
                () = cancel.cancelled() => {
                    if kill_on_cancel {
                        let _ = self.child.start_kill();
                    } else {
                        let _ = self.send_notification(
                            "notifications/cancelled",
                            json!({ "requestId": req_id, "reason": "cancelled by client" }),
                        ).await;
                    }
                    break Err(RuntimeError::IpcProtocol {
                        ext_name: self.name.clone(),
                        message: format!("IPC call '{method}' cancelled"),
                    });
                }
                res = &mut resp_rx => {
                    while let Ok(msg) = other_rx.try_recv() {
                        let (reply, IncomingControl::Continue) = on_other(&msg);
                        if let Some(reply_msg) = reply {
                            let _ = self.send_json(&reply_msg).await;
                        }
                    }
                    break match res {
                        Ok(inner) => inner,
                        Err(_) => Err(RuntimeError::IpcProtocol {
                            ext_name: self.name.clone(),
                            message: format!("process closed stdout while awaiting '{method}'"),
                        }),
                    };
                }
            }
        };

        if let Ok(mut map) = self.pending.lock() {
            map.remove(&req_id);
        }
        if let Ok(mut guard) = self.active_other_tx.lock() {
            *guard = None;
        }
        result
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
        if let Some(handle) = self.reader_task.take() {
            handle.abort();
        }
    }
}

fn fail_all_pending(pending: &PendingMap, ext_name: &str, message: String) {
    if let Ok(mut map) = pending.lock() {
        let entries = std::mem::take(&mut *map);
        for (_, tx) in entries {
            let _ = tx.send(Err(RuntimeError::IpcProtocol {
                ext_name: ext_name.to_string(),
                message: message.clone(),
            }));
        }
    }
}
