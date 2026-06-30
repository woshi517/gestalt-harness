#![allow(clippy::type_complexity)]
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::sync::{mpsc, oneshot};

use crate::config::{ExtensionLimitsConfig, ExtensionTimeoutsConfig};
use crate::error::{Result, RuntimeError};
use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use crate::lifecycle::InitializeRequestV2;
use crate::extension::ExtensionRuntimeComponent;

pub struct ProcessExtensionBroker {
    component_id: String,
    pub(crate) event_bus: RuntimeEventBus,
    tx: mpsc::Sender<(
        JsonRpcRequest,
        oneshot::Sender<std::result::Result<JsonRpcResponse, String>>,
    )>,
    child: Arc<Mutex<Option<Child>>>,
    timeouts: ExtensionTimeoutsConfig,
    _limits: ExtensionLimitsConfig,
    is_trusted: bool,
    negotiated_version: Arc<Mutex<String>>,
    supports_cancellation: Arc<Mutex<bool>>,
    pending_requests:
        Arc<Mutex<HashMap<String, oneshot::Sender<std::result::Result<JsonRpcResponse, String>>>>>,
}

impl ProcessExtensionBroker {
    pub fn is_trusted(&self) -> bool {
        self.is_trusted
    }

    pub fn negotiated_version(&self) -> String {
        self.negotiated_version
            .try_lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    pub fn supports_cancellation(&self) -> bool {
        self.supports_cancellation
            .try_lock()
            .is_ok_and(|g| *g)
    }
}

async fn read_line_bounded<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    buf: &mut String,
    max_bytes: usize,
) -> std::io::Result<usize> {
    let mut temp_buf = [0; 1];
    let mut total_bytes = 0;
    buf.clear();

    loop {
        if total_bytes >= max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Message size limit exceeded",
            ));
        }

        let n = reader.read(&mut temp_buf).await?;
        if n == 0 {
            return Ok(total_bytes);
        }

        let b = temp_buf[0];
        total_bytes += 1;

        if b == b'\n' {
            return Ok(total_bytes);
        }

        buf.push(b as char);
    }
}

fn jsonrpc_id_to_string(id: &serde_json::Value) -> String {
    match id {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => id.to_string(),
    }
}

fn validate_jsonrpc_response(
    val: &serde_json::Value,
) -> std::result::Result<(String, JsonRpcResponse), String> {
    let obj = val
        .as_object()
        .ok_or_else(|| "Message is not a JSON object".to_string())?;

    let jsonrpc = obj
        .get("jsonrpc")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing jsonrpc field".to_string())?;
    if jsonrpc != "2.0" {
        return Err(format!("Unsupported jsonrpc version: {}", jsonrpc));
    }

    let id_val = obj
        .get("id")
        .ok_or_else(|| "Response is missing 'id'".to_string())?;
    if id_val.is_null() {
        return Err("id cannot be null".to_string());
    }
    let id_str = jsonrpc_id_to_string(id_val);

    let has_result = obj.contains_key("result");
    let has_error = obj.contains_key("error");

    if has_result && has_error {
        return Err("Response contains both result and error".to_string());
    }
    if !has_result && !has_error {
        return Err("Response must contain either result or error".to_string());
    }

    let resp: JsonRpcResponse = serde_json::from_value(val.clone())
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok((id_str, resp))
}

impl ProcessExtensionBroker {
    pub async fn spawn(
        component: ExtensionRuntimeComponent,
        event_bus: RuntimeEventBus,
        timeouts: ExtensionTimeoutsConfig,
        limits: ExtensionLimitsConfig,
        is_trusted: bool,
    ) -> Result<Self> {
        Self::spawn_with_component(component, event_bus, timeouts, limits, is_trusted).await
    }

    pub async fn spawn_with_component(
        component: ExtensionRuntimeComponent,
        event_bus: RuntimeEventBus,
        timeouts: ExtensionTimeoutsConfig,
        limits: ExtensionLimitsConfig,
        is_trusted: bool,
    ) -> Result<Self> {
        if component.kind != crate::extension::ComponentKind::GestaltLifecycle {
            return Err(RuntimeError::Extension(format!(
                "Process broker only supports lifecycle components, got '{}'",
                component.id.canonical_id()
            )));
        }

        let extension_id = component.id.canonical_id();
        let entrypoint = crate::manifest::Entrypoint {
            command: component.entrypoint_command.clone(),
            args: component.entrypoint_args.clone(),
        };
        let shell_allowed = crate::permissions::check_shell_permission_effective(
            &component.permissions,
            Some(&component.grants),
            &event_bus,
            &extension_id,
        )
        .is_ok();

        if let Err(reason) = crate::manifest::validate_shell_entrypoint(&entrypoint, shell_allowed) {
            event_bus.publish(RuntimeEvent::ExtensionRejected {
                extension_id: extension_id.clone(),
                reason: reason.clone(),
            });
            return Err(RuntimeError::Extension(reason));
        }

        let mut cmd = if let Some(ref source_root) = component.package_source_root {
            let resolved_cmd_path = if std::path::Path::new(&component.entrypoint_command).is_absolute() {
                std::path::PathBuf::from(&component.entrypoint_command)
            } else {
                source_root.join(&component.entrypoint_command)
            };
            let mut c = Command::new(resolved_cmd_path);
            c.current_dir(source_root);
            c
        } else {
            Command::new(&component.entrypoint_command)
        };

        cmd.args(&component.entrypoint_args);
        cmd.env_clear();

        let safe_envs = [
            "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TERM", "LANG", "LC_ALL", "LC_CTYPE",
            "TMPDIR", "TEMP", "TMP",
        ];
        for var in &safe_envs {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| {
            event_bus.publish(RuntimeEvent::ExtensionRejected {
                extension_id: extension_id.clone(),
                reason: format!("Failed to spawn child process: {}", e),
            });
            RuntimeError::Extension(format!("Spawn failed: {}", e))
        })?;

        let pid = child.id().unwrap_or(0);
        event_bus.publish(RuntimeEvent::ProcessSpawned {
            extension_id: extension_id.clone(),
            pid,
        });

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::Extension("Stdin piped was not captured".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::Extension("Stdout piped was not captured".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| RuntimeError::Extension("Stderr piped was not captured".to_string()))?;

        let (tx, mut rx) = mpsc::channel::<(
            JsonRpcRequest,
            oneshot::Sender<std::result::Result<JsonRpcResponse, String>>,
        )>(100);
        let child_arc = Arc::new(Mutex::new(Some(child)));
        let child_clone = child_arc.clone();
        let extension_id_clone = extension_id.clone();
        let event_bus_clone = event_bus.clone();

        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    break;
                }
                event_bus_clone.publish(RuntimeEvent::ExtensionError {
                    extension_id: extension_id_clone.clone(),
                    message: format!("stderr: {}", line.trim()),
                });
                line.clear();
            }
        });

        let pending_requests: Arc<
            Mutex<HashMap<String, oneshot::Sender<std::result::Result<JsonRpcResponse, String>>>>,
        > = Arc::new(Mutex::new(HashMap::new()));
        let pending_requests_clone1 = pending_requests.clone();
        let limits_for_writer = limits.clone();

        tokio::spawn(async move {
            let mut stdin = stdin;
            let max_pending = limits_for_writer.max_pending_requests.unwrap_or(16);

            while let Some((req, response_tx)) = rx.recv().await {
                let id_str = req
                    .id
                    .as_ref()
                    .map(jsonrpc_id_to_string)
                    .unwrap_or_default();
                if req.id.is_some() {
                    let current_len = {
                        let lock = pending_requests_clone1.lock().await;
                        lock.len()
                    };
                    if current_len >= max_pending {
                        let _ = response_tx.send(Err("Too many pending requests".to_string()));
                        continue;
                    }
                    let mut lock = pending_requests_clone1.lock().await;
                    lock.insert(id_str.clone(), response_tx);
                }

                let mut req_bytes = serde_json::to_vec(&req).unwrap_or_default();
                req_bytes.push(b'\n');
                if stdin.write_all(&req_bytes).await.is_err() || stdin.flush().await.is_err() {
                    if req.id.is_some() {
                        let mut lock = pending_requests_clone1.lock().await;
                        if let Some(tx) = lock.remove(&id_str) {
                            let _ = tx.send(Err("Failed to write to stdin".to_string()));
                        }
                    }
                }
            }
        });

        let pending_requests_clone2 = pending_requests.clone();
        let extension_id_clone2 = extension_id.clone();
        let event_bus_clone2 = event_bus.clone();
        let limits_clone = limits.clone();

        tokio::spawn(async move {
            let mut stdout_reader = BufReader::new(stdout);
            let mut line = String::new();

            let read_limit = limits_clone.max_message_bytes.unwrap_or(8_388_608);
            let max_errors = limits_clone.max_protocol_errors.unwrap_or(3);
            let mut protocol_errors = 0;

            loop {
                let read_res = read_line_bounded(&mut stdout_reader, &mut line, read_limit).await;
                match read_res {
                    Err(e) => {
                        event_bus_clone2.publish(RuntimeEvent::ExtensionError {
                            extension_id: extension_id_clone2.clone(),
                            message: format!("ExtensionProtocolError: {}", e),
                        });
                        break;
                    }
                    Ok(0) => break,
                    Ok(_) => {
                        let parse_res: serde_json::Result<serde_json::Value> =
                            serde_json::from_str(&line);
                        match parse_res {
                            Err(e) => {
                                protocol_errors += 1;
                                event_bus_clone2.publish(RuntimeEvent::ExtensionError {
                                    extension_id: extension_id_clone2.clone(),
                                    message: format!(
                                        "ExtensionProtocolError: Malformed JSON: {}",
                                        e
                                    ),
                                });
                            }
                            Ok(val) => match validate_jsonrpc_response(&val) {
                                Ok((id_str, resp)) => {
                                    let mut lock = pending_requests_clone2.lock().await;
                                    if let Some(tx) = lock.remove(&id_str) {
                                        let _ = tx.send(Ok(resp));
                                    } else {
                                        protocol_errors += 1;
                                        event_bus_clone2.publish(RuntimeEvent::ExtensionError {
                                            extension_id: extension_id_clone2.clone(),
                                            message: format!(
                                                "ExtensionProtocolError: Unknown response ID: {}",
                                                id_str
                                            ),
                                        });
                                    }
                                }
                                Err(err_msg) => {
                                    protocol_errors += 1;
                                    event_bus_clone2.publish(RuntimeEvent::ExtensionError {
                                        extension_id: extension_id_clone2.clone(),
                                        message: format!("ExtensionProtocolError: {}", err_msg),
                                    });
                                    if let Some(id_val) = val.get("id") {
                                        let id_str = match id_val {
                                            serde_json::Value::String(s) => s.clone(),
                                            serde_json::Value::Number(n) => n.to_string(),
                                            _ => id_val.to_string(),
                                        };
                                        let mut lock = pending_requests_clone2.lock().await;
                                        if let Some(tx) = lock.remove(&id_str) {
                                            let _ = tx.send(Err(format!(
                                                "ExtensionProtocolError: {}",
                                                err_msg
                                            )));
                                        }
                                    }
                                }
                            },
                        }
                        line.clear();
                        if protocol_errors >= max_errors {
                            event_bus_clone2.publish(RuntimeEvent::ProcessKilled {
                                extension_id: extension_id_clone2.clone(),
                                reason: "Max protocol errors exceeded".to_string(),
                            });
                            break;
                        }
                    }
                }
            }

            {
                let mut lock = pending_requests_clone2.lock().await;
                for (_, tx) in lock.drain() {
                    let _ = tx.send(Err("Process exited".to_string()));
                }
            }

            let mut lock = child_clone.lock().await;
            if let Some(mut child) = lock.take() {
                let _ = child.kill().await;
                let status = child.wait().await;
                event_bus_clone2.publish(RuntimeEvent::ProcessExited {
                    extension_id: extension_id_clone2,
                    exit_code: status.ok().and_then(|s| s.code()),
                });
            }
        });

        let negotiated_version = Arc::new(Mutex::new(String::new()));
        let supports_cancellation = Arc::new(Mutex::new(false));

        let broker = Self {
            component_id: extension_id.clone(),
            event_bus: event_bus.clone(),
            tx,
            child: child_arc,
            timeouts,
            _limits: limits,
            is_trusted,
            negotiated_version: negotiated_version.clone(),
            supports_cancellation: supports_cancellation.clone(),
            pending_requests: pending_requests.clone(),
        };

        let init_res = broker
            .call(
                "initialize",
                Some(serde_json::to_value(InitializeRequestV2 {
                    supported_versions: vec!["2.0".to_string()],
                })
                .unwrap_or_else(|_| serde_json::json!({ "supported_versions": ["2.0"] }))),
            )
            .await;

        let init = match init_res {
            Ok(val) => match serde_json::from_value::<crate::lifecycle::InitializeResponseV2>(val)
            {
                Ok(init) => init,
                Err(err) => {
                    broker.shutdown().await;
                    event_bus.publish(RuntimeEvent::ExtensionRejected {
                        extension_id: extension_id.clone(),
                        reason: format!("invalid initialize response: {err}"),
                    });
                    return Err(RuntimeError::Extension(format!(
                        "invalid initialize response: {err}"
                    )));
                }
            },
            Err(err) => {
                broker.shutdown().await;
                event_bus.publish(RuntimeEvent::ExtensionRejected {
                    extension_id: extension_id.clone(),
                    reason: format!("Initialization failed: {}", err),
                });
                return Err(RuntimeError::Extension(format!(
                    "Initialization failed: {}",
                    err
                )));
            }
        };

        if init.negotiated_version != "2.0" {
            broker.shutdown().await;
            event_bus.publish(RuntimeEvent::ExtensionRejected {
                extension_id: extension_id.clone(),
                reason: format!(
                    "No mutually supported protocol version (manifest: 2.0, extension negotiated: {})",
                    init.negotiated_version
                ),
            });
            return Err(RuntimeError::Extension(
                "No mutually supported protocol version".to_string(),
            ));
        }

        {
            let mut nv = negotiated_version.lock().await;
            *nv = init.negotiated_version;
            let mut nc = supports_cancellation.lock().await;
            *nc = init.supports_cancellation;
        }

        event_bus.publish(RuntimeEvent::ExtensionLoaded {
            extension_id: extension_id.clone(),
        });

        Ok(broker)
    }

    pub async fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> std::result::Result<serde_json::Value, String> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let req = JsonRpcRequest::new(
            method,
            params,
            Some(serde_json::Value::String(request_id.clone())),
        );

        self.event_bus.publish(RuntimeEvent::RpcRequest {
            extension_id: self.component_id.clone(),
            method: method.to_string(),
            request_id: request_id.clone(),
        });

        let (resp_tx, resp_rx) = oneshot::channel();
        if self.tx.send((req, resp_tx)).await.is_err() {
            self.event_bus.publish(RuntimeEvent::RpcResponse {
                extension_id: self.component_id.clone(),
                method: method.to_string(),
                request_id: request_id.clone(),
                success: false,
            });
            return Err("Broker channel closed".to_string());
        }

        let timeout_ms = match method {
            "initialize" => self.timeouts.initialize_ms.unwrap_or(10000),
            "shutdown" => self.timeouts.shutdown_ms.unwrap_or(5000),
            m if m.starts_with("tools/") => self.timeouts.tool_ms.unwrap_or(60000),
            m if m.starts_with("context/") => self.timeouts.context_ms.unwrap_or(15000),
            _ => self.timeouts.hook_ms.unwrap_or(5000),
        };
        let timeout_dur = Duration::from_millis(timeout_ms);

        let supports_cancel = self.supports_cancellation();

        let result = tokio::time::timeout(timeout_dur, resp_rx).await;
        match result {
            Ok(Ok(Ok(response))) => {
                self.event_bus.publish(RuntimeEvent::RpcResponse {
                    extension_id: self.component_id.clone(),
                    method: method.to_string(),
                    request_id: request_id.clone(),
                    success: response.error.is_none(),
                });
                if let Some(err) = response.error {
                    Err(format!("JSON-RPC Error {}: {}", err.code, err.message))
                } else {
                    Ok(response.result.unwrap_or(serde_json::Value::Null))
                }
            }
            Ok(Ok(Err(e))) => {
                self.event_bus.publish(RuntimeEvent::RpcResponse {
                    extension_id: self.component_id.clone(),
                    method: method.to_string(),
                    request_id: request_id.clone(),
                    success: false,
                });
                Err(e)
            }
            Ok(Err(_)) => {
                self.event_bus.publish(RuntimeEvent::RpcResponse {
                    extension_id: self.component_id.clone(),
                    method: method.to_string(),
                    request_id: request_id.clone(),
                    success: false,
                });
                {
                    let mut lock = self.pending_requests.lock().await;
                    lock.remove(&request_id);
                }
                Err("Oneshot channel dropped".to_string())
            }
            Err(_) => {
                self.event_bus.publish(RuntimeEvent::RpcResponse {
                    extension_id: self.component_id.clone(),
                    method: method.to_string(),
                    request_id: request_id.clone(),
                    success: false,
                });
                {
                    let mut lock = self.pending_requests.lock().await;
                    lock.remove(&request_id);
                }
                if supports_cancel {
                    let cancel_req = JsonRpcRequest::new(
                        "$/cancelRequest",
                        Some(serde_json::json!({ "id": request_id.clone() })),
                        None,
                    );
                    let _ = self.tx.send((cancel_req, oneshot::channel().0)).await;
                }
                Err("Request timed out".to_string())
            }
        }
    }

    pub async fn shutdown(&self) {
        let _ = self.call("shutdown", None).await;
        let exit_req = JsonRpcRequest::new("exit", None, None);
        let _ = self.tx.send((exit_req, oneshot::channel().0)).await;

        let mut lock = self.child.lock().await;
        if let Some(mut child) = lock.take() {
            let shutdown_timeout = self.timeouts.shutdown_ms.unwrap_or(5000);
            if let Ok(status) =
                tokio::time::timeout(Duration::from_millis(shutdown_timeout), child.wait()).await
            {
                self.event_bus.publish(RuntimeEvent::ProcessExited {
                    extension_id: self.component_id.clone(),
                    exit_code: status.ok().and_then(|s| s.code()),
                });
            } else {
                let _ = child.kill().await;
                let status = child.wait().await;
                self.event_bus.publish(RuntimeEvent::ProcessExited {
                    extension_id: self.component_id.clone(),
                    exit_code: status.ok().and_then(|s| s.code()),
                });
            }
        }
    }
}
