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
use crate::extension::GestaltExtension;
use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use crate::manifest::{Capabilities, ExtensionManifest, ToolDeclaration};
use crate::registry::RuntimeRegistry;

pub struct ProcessExtensionBroker {
    manifest: ExtensionManifest,
    pub(crate) event_bus: RuntimeEventBus,
    tx: mpsc::Sender<(
        JsonRpcRequest,
        oneshot::Sender<std::result::Result<JsonRpcResponse, String>>,
    )>,
    child: Arc<Mutex<Option<Child>>>,
    timeouts: ExtensionTimeoutsConfig,
    limits: ExtensionLimitsConfig,
    is_trusted: bool,
    negotiated_version: Arc<Mutex<String>>,
    negotiated_capabilities: Arc<Mutex<Capabilities>>,
    pending_requests: Arc<Mutex<HashMap<
        String,
        oneshot::Sender<std::result::Result<JsonRpcResponse, String>>,
    >>>,
}

impl ProcessExtensionBroker {
    pub fn is_trusted(&self) -> bool {
        self.is_trusted
    }

    pub fn negotiated_version(&self) -> String {
        self.negotiated_version.try_lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn negotiated_capabilities(&self) -> Capabilities {
        self.negotiated_capabilities.try_lock().map(|g| g.clone()).unwrap_or_default()
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

fn validate_jsonrpc_response(val: &serde_json::Value) -> std::result::Result<(String, JsonRpcResponse), String> {
    let obj = val.as_object().ok_or_else(|| "Message is not a JSON object".to_string())?;

    let jsonrpc = obj.get("jsonrpc")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing jsonrpc field".to_string())?;
    if jsonrpc != "2.0" {
        return Err(format!("Unsupported jsonrpc version: {}", jsonrpc));
    }

    let id_val = obj.get("id").ok_or_else(|| "Response is missing 'id'".to_string())?;
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
        manifest: ExtensionManifest,
        event_bus: RuntimeEventBus,
        timeouts: ExtensionTimeoutsConfig,
        limits: ExtensionLimitsConfig,
        is_trusted: bool,
    ) -> Result<Self> {
        let extension_id = manifest.id.clone();

        if let Err(reason) = crate::manifest::validate_shell_entrypoint(
            &manifest.entrypoint,
            manifest.permissions.allow_shell,
        ) {
            event_bus.publish(RuntimeEvent::ExtensionRejected {
                extension_id: extension_id.clone(),
                reason,
            });
            return Err(RuntimeError::Extension(
                "Missing shell permission for command".to_string(),
            ));
        }

        let mut cmd = Command::new(&manifest.entrypoint.command);
        cmd.args(&manifest.entrypoint.args);
        cmd.env_clear();

        // Inherit only safe/non-sensitive env variables from parent
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

        // Spawn stderr drainer task
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

        let pending_requests: Arc<Mutex<HashMap<
            String,
            oneshot::Sender<std::result::Result<JsonRpcResponse, String>>,
        >>> = Arc::new(Mutex::new(HashMap::new()));

        let pending_requests_clone1 = pending_requests.clone();
        let limits_for_writer = limits.clone();

        // Spawn stdin writer task
        tokio::spawn(async move {
            let mut stdin = stdin;
            let max_pending = limits_for_writer.max_pending_requests.unwrap_or(16);

            while let Some((req, response_tx)) = rx.recv().await {
                let id_str = req.id.as_ref().map(jsonrpc_id_to_string).unwrap_or_default();
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

        // Spawn stdout reader task
        tokio::spawn(async move {
            let mut stdout_reader = BufReader::new(stdout);
            let mut line = String::new();

            let read_limit = limits_clone.max_message_bytes.unwrap_or(8388608);
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
                        let parse_res: serde_json::Result<serde_json::Value> = serde_json::from_str(&line);
                        match parse_res {
                            Err(e) => {
                                protocol_errors += 1;
                                event_bus_clone2.publish(RuntimeEvent::ExtensionError {
                                    extension_id: extension_id_clone2.clone(),
                                    message: format!("ExtensionProtocolError: Malformed JSON: {}", e),
                                });
                            }
                            Ok(val) => {
                                match validate_jsonrpc_response(&val) {
                                    Ok((id_str, resp)) => {
                                        let mut lock = pending_requests_clone2.lock().await;
                                        if let Some(tx) = lock.remove(&id_str) {
                                            let _ = tx.send(Ok(resp));
                                        } else {
                                            protocol_errors += 1;
                                            event_bus_clone2.publish(RuntimeEvent::ExtensionError {
                                                extension_id: extension_id_clone2.clone(),
                                                message: format!("ExtensionProtocolError: Unknown response ID: {}", id_str),
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
                                                let _ = tx.send(Err(format!("ExtensionProtocolError: {}", err_msg)));
                                            }
                                        }
                                    }
                                }
                            }
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
        let negotiated_capabilities = Arc::new(Mutex::new(Capabilities::default()));

        let broker = Self {
            manifest: manifest.clone(),
            event_bus: event_bus.clone(),
            tx,
            child: child_arc,
            timeouts,
            limits,
            is_trusted,
            negotiated_version: negotiated_version.clone(),
            negotiated_capabilities: negotiated_capabilities.clone(),
            pending_requests: pending_requests.clone(),
        };

        // Initialize handshake
        let init_params = serde_json::json!({
            "capabilities": manifest.capabilities,
            "version": manifest.protocol_version.clone().unwrap_or_else(|| "1.0".to_string())
        });

        let init_res = broker.call("initialize", Some(init_params)).await;
        let (negotiated_ver, negotiated_caps) = match init_res {
            Ok(val) => {
                let ver = val.get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("1.0")
                    .to_string();
                let caps = if let Some(caps_val) = val.get("capabilities") {
                    serde_json::from_value::<Capabilities>(caps_val.clone())
                        .unwrap_or_else(|_| manifest.capabilities.clone())
                } else {
                    manifest.capabilities.clone()
                };
                (ver, caps)
            }
            Err(e) => {
                broker.shutdown().await;
                event_bus.publish(RuntimeEvent::ExtensionRejected {
                    extension_id: extension_id.clone(),
                    reason: format!("Initialization failed: {}", e),
                });
                return Err(RuntimeError::Extension(format!(
                    "Initialization failed: {}",
                    e
                )));
            }
        };

        let manifest_proto = manifest.protocol_version.as_deref().unwrap_or("1.0");
        if manifest_proto == "1.1" {
            if negotiated_ver != "1.1" && negotiated_ver != "1.0" {
                broker.shutdown().await;
                event_bus.publish(RuntimeEvent::ExtensionRejected {
                    extension_id: extension_id.clone(),
                    reason: format!("No mutually supported protocol version (manifest: {}, extension negotiated: {})", manifest_proto, negotiated_ver),
                });
                return Err(RuntimeError::Extension("No mutually supported protocol version".to_string()));
            }
        } else if manifest_proto == "1.0" {
            if negotiated_ver != "1.0" {
                broker.shutdown().await;
                event_bus.publish(RuntimeEvent::ExtensionRejected {
                    extension_id: extension_id.clone(),
                    reason: format!("No mutually supported protocol version (manifest: {}, extension negotiated: {})", manifest_proto, negotiated_ver),
                });
                return Err(RuntimeError::Extension("No mutually supported protocol version".to_string()));
            }
        } else {
            broker.shutdown().await;
            event_bus.publish(RuntimeEvent::ExtensionRejected {
                extension_id: extension_id.clone(),
                reason: format!("Unsupported manifest protocol version: {}", manifest_proto),
            });
            return Err(RuntimeError::Extension("Unsupported manifest protocol version".to_string()));
        }

        {
            let mut nv = negotiated_version.lock().await;
            *nv = negotiated_ver;
            let mut nc = negotiated_capabilities.lock().await;
            *nc = negotiated_caps;
        }

        event_bus.publish(RuntimeEvent::ExtensionLoaded { extension_id });

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
            extension_id: self.manifest.id.clone(),
            method: method.to_string(),
            request_id: request_id.clone(),
        });

        let (resp_tx, resp_rx) = oneshot::channel();
        if self.tx.send((req, resp_tx)).await.is_err() {
            self.event_bus.publish(RuntimeEvent::RpcResponse {
                extension_id: self.manifest.id.clone(),
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

        let supports_cancel = {
            let caps = self.negotiated_capabilities.lock().await;
            caps.supports_cancellation
        };

        let result = tokio::time::timeout(timeout_dur, resp_rx).await;
        match result {
            Ok(Ok(Ok(response))) => {
                self.event_bus.publish(RuntimeEvent::RpcResponse {
                    extension_id: self.manifest.id.clone(),
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
                    extension_id: self.manifest.id.clone(),
                    method: method.to_string(),
                    request_id: request_id.clone(),
                    success: false,
                });
                Err(e)
            }
            Ok(Err(_)) => {
                self.event_bus.publish(RuntimeEvent::RpcResponse {
                    extension_id: self.manifest.id.clone(),
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
                    extension_id: self.manifest.id.clone(),
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
            match tokio::time::timeout(Duration::from_millis(shutdown_timeout), child.wait()).await {
                Ok(status) => {
                    self.event_bus.publish(RuntimeEvent::ProcessExited {
                        extension_id: self.manifest.id.clone(),
                        exit_code: status.ok().and_then(|s| s.code()),
                    });
                }
                Err(_) => {
                    let _ = child.kill().await;
                    let status = child.wait().await;
                    self.event_bus.publish(RuntimeEvent::ProcessExited {
                        extension_id: self.manifest.id.clone(),
                        exit_code: status.ok().and_then(|s| s.code()),
                    });
                }
            }
        }
    }
}

fn check_input_permissions(
    manifest: &ExtensionManifest,
    input: &serde_json::Value,
    workspace_root: &std::path::Path,
    event_bus: &RuntimeEventBus,
) -> std::result::Result<(), gestalt_core::error::ToolError> {
    match input {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key_lower = k.to_lowercase();
                if let serde_json::Value::String(s) = v {
                    if key_lower.contains("path")
                        || key_lower.contains("file")
                        || key_lower.contains("dir")
                        || key_lower.contains("dest")
                        || key_lower.contains("src")
                        || key_lower.contains("target")
                        || key_lower.contains("output")
                    {
                        let is_write = key_lower.contains("write")
                            || key_lower.contains("dest")
                            || key_lower.contains("output")
                            || key_lower.contains("target");
                        let p = std::path::Path::new(s);
                        if let Err(e) = crate::permissions::check_path_permission(
                            manifest,
                            workspace_root,
                            p,
                            is_write,
                            event_bus,
                        ) {
                            return Err(gestalt_core::error::ToolError::PathNotAllowed(e));
                        }
                    }
                    if key_lower.contains("url")
                        || key_lower.contains("host")
                        || key_lower.contains("uri")
                        || key_lower.contains("address")
                    {
                        let host = if let Ok(url) = url::Url::parse(s) {
                            url.host_str().unwrap_or(s).to_string()
                        } else {
                            s.clone()
                        };
                        if let Err(e) =
                            crate::permissions::check_network_permission(manifest, &host, event_bus)
                        {
                            return Err(gestalt_core::error::ToolError::NetworkDenied(e));
                        }
                    }
                } else {
                    check_input_permissions(manifest, v, workspace_root, event_bus)?;
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                check_input_permissions(manifest, v, workspace_root, event_bus)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub struct ProcessBackedTool {
    broker: Arc<ProcessExtensionBroker>,
    name: String,
    description: String,
    schema: gestalt_core::tool::ToolSchema,
    risk: gestalt_core::tool::RiskLevel,
    /// Manifest declaration for this tool. We retain it so the
    /// trust/provenance-aware descriptor builder can be used at
    /// `descriptor()` time. Keeping the original declaration also
    /// preserves the extension-declared `risk` and any trust
    /// annotations the manifest provides, so policy decisions and
    /// trace metadata stay consistent with what the extension
    /// shipped.
    tool_decl: ToolDeclaration,
}

#[async_trait::async_trait]
impl gestalt_core::tool::Tool for ProcessBackedTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> gestalt_core::tool::ToolSchema {
        self.schema.clone()
    }

    fn risk(&self, _input: &serde_json::Value) -> gestalt_core::tool::RiskLevel {
        self.risk
    }

    fn descriptor(&self) -> gestalt_core::tool_descriptor::ToolDescriptor {
        crate::extension_trust::build_extension_tool_descriptor(
            &self.broker.manifest,
            &self.tool_decl,
            self.broker.is_trusted(),
        )
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        ctx: &gestalt_core::tool::ToolContext,
    ) -> std::result::Result<gestalt_core::tool::ToolOutput, gestalt_core::error::ToolError> {
        let workspace_root = ctx
            .workspace_root
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("."));

        if !self.broker.manifest.capabilities.tools {
            return Err(gestalt_core::error::ToolError::Denied(
                "Tools capability is not enabled in manifest".to_string(),
            ));
        }

        check_input_permissions(
            &self.broker.manifest,
            &input,
            workspace_root,
            &self.broker.event_bus,
        )?;

        let res = self
            .broker
            .call(
                "tools/call",
                Some(serde_json::json!({
                    "name": self.name.clone(),
                    "input": input
                })),
            )
            .await
            .map_err(|e| {
                gestalt_core::error::ToolError::ExecutionFailed(std::io::Error::other(e))
            })?;

        if let Some(artifacts_val) = res.get("artifacts").and_then(|v| v.as_array()) {
            if !artifacts_val.is_empty() {
                let art = &artifacts_val[0];
                let path = art.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let mime_type = art.get("mime_type").and_then(|v| v.as_str()).unwrap_or("text/plain");
                let size_bytes = art.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

                return Ok(gestalt_core::tool::ToolOutput::Artifact {
                    path: std::path::PathBuf::from(path),
                    mime_type: mime_type.to_string(),
                    size_bytes,
                });
            }
        }

        let content = if let Some(content_str) = res.get("content").and_then(|v| v.as_str()) {
            content_str.to_string()
        } else {
            res.to_string()
        };

        Ok(gestalt_core::tool::ToolOutput::Text { content })
    }
}

pub struct ProcessBackedContextContributor {
    broker: Arc<ProcessExtensionBroker>,
    name: String,
    stability: gestalt_core::ContextStability,
}

#[async_trait::async_trait]
impl crate::context::ContextContributor for ProcessBackedContextContributor {
    fn name(&self) -> &str {
        &self.name
    }

    fn stability(&self) -> gestalt_core::ContextStability {
        self.stability
    }

    async fn contribute(
        &self,
        workspace_root: &std::path::Path,
    ) -> Result<gestalt_core::message::Message> {
        if !self.broker.manifest.capabilities.context {
            return Err(RuntimeError::Extension(
                "Context capability is not enabled in manifest".to_string(),
            ));
        }

        if let Err(e) = crate::permissions::check_path_permission(
            &self.broker.manifest,
            workspace_root,
            workspace_root,
            false,
            &self.broker.event_bus,
        ) {
            return Err(RuntimeError::Extension(e));
        }

        let res = self
            .broker
            .call(
                "context/inject",
                Some(serde_json::json!({
                    "name": self.name.clone()
                })),
            )
            .await
            .map_err(RuntimeError::Extension)?;

        let content = if let Some(items_val) = res.get("items").and_then(|v| v.as_array()) {
            let mut combined_content = String::new();
            for item in items_val {
                let mut trust = item.get("trust").and_then(|t| t.as_str()).unwrap_or("untrusted");
                let mut priority = item.get("priority").and_then(|p| p.as_str()).unwrap_or("medium");
                let item_content = item.get("content").and_then(|c| c.as_str()).unwrap_or("");

                let is_broker_trusted = self.broker.is_trusted();
                if !is_broker_trusted {
                    if trust == "trusted" {
                        eprintln!(
                            "Warning: Context contribution from untrusted extension '{}' claimed trusted status. Downgrading to untrusted.",
                            self.broker.manifest.id
                        );
                        trust = "untrusted";
                    }
                    if priority == "critical" {
                        eprintln!(
                            "Warning: Context contribution from untrusted extension '{}' claimed critical priority. Downgrading to high.",
                            self.broker.manifest.id
                        );
                        priority = "high";
                    }
                }

                let _ = (trust, priority); // keep compiler happy for unused warnings

                if !combined_content.is_empty() {
                    combined_content.push('\n');
                }
                combined_content.push_str(item_content);
            }
            combined_content
        } else {
            res.get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };

        Ok(gestalt_core::message::Message::System { content })
    }
}

pub struct ProcessExtension {
    pub manifest: ExtensionManifest,
    pub broker: Arc<ProcessExtensionBroker>,
}

impl ProcessExtension {
    pub fn new(manifest: ExtensionManifest, broker: Arc<ProcessExtensionBroker>) -> Self {
        Self { manifest, broker }
    }
}

impl GestaltExtension for ProcessExtension {
    #[allow(clippy::misnamed_getters)]
    fn name(&self) -> &str {
        &self.manifest.id
    }

    fn as_process_extension(&self) -> Option<&crate::process_extension::ProcessExtension> {
        Some(self)
    }

    fn register(&self, registry: &mut RuntimeRegistry) -> Result<()> {
        let extension_id = self.manifest.id.clone();

        for tool in &self.manifest.tools {
            let schema = tool.input_schema.clone();
            let tool_schema: gestalt_core::tool::ToolSchema =
                serde_json::from_value(serde_json::json!({
                    "name": tool.name.clone(),
                    "description": tool.description.clone(),
                    "input_schema": schema
                }))
                .unwrap();

            let risk = match tool.risk.as_deref() {
                Some("low") => gestalt_core::tool::RiskLevel::Low,
                Some("medium") => gestalt_core::tool::RiskLevel::Medium,
                Some("high") => gestalt_core::tool::RiskLevel::High,
                Some("critical") => gestalt_core::tool::RiskLevel::Critical,
                _ => gestalt_core::tool::RiskLevel::High, // default to high for safety
            };

            let wrapped_tool = Arc::new(ProcessBackedTool {
                broker: self.broker.clone(),
                name: tool.name.clone(),
                description: tool.description.clone(),
                schema: tool_schema.clone(),
                risk,
                tool_decl: tool.clone(),
            });

            registry.register_executable_tool(
                tool.name.clone(),
                tool_schema,
                wrapped_tool,
                Some(extension_id.clone()),
            )?;
        }

        for injector in &self.manifest.context_injectors {
            let stability = injector.stability.ok_or_else(|| {
                RuntimeError::Extension(format!(
                    "Context injector '{}' must declare stability",
                    injector.name
                ))
            })?;
            let contributor = Arc::new(ProcessBackedContextContributor {
                broker: self.broker.clone(),
                name: injector.name.clone(),
                stability,
            });

            registry.register_executable_context_contributor(
                injector.name.clone(),
                contributor,
                Some(extension_id.clone()),
            )?;
        }

        for hook in &self.manifest.hooks {
            registry.register_hook(hook.name.clone())?;
        }

        Ok(())
    }
}
