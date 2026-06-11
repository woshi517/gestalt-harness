use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::sync::{mpsc, oneshot};

use crate::error::{Result, RuntimeError};
use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use crate::extension::GestaltExtension;
use crate::jsonrpc::{JsonRpcRequest, JsonRpcResponse};
use crate::manifest::{ExtensionManifest, ToolDeclaration};
use crate::registry::RuntimeRegistry;

pub struct ProcessExtensionBroker {
    manifest: ExtensionManifest,
    event_bus: RuntimeEventBus,
    tx: mpsc::Sender<(
        JsonRpcRequest,
        oneshot::Sender<std::result::Result<JsonRpcResponse, String>>,
    )>,
    child: Arc<Mutex<Option<Child>>>,
}

impl ProcessExtensionBroker {
    pub async fn spawn(manifest: ExtensionManifest, event_bus: RuntimeEventBus) -> Result<Self> {
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

        let extension_id_clone2 = extension_id.clone();
        let event_bus_clone2 = event_bus.clone();

        // Spawn stdin/stdout JSON-RPC loop task
        tokio::spawn(async move {
            let mut stdin = stdin;
            let mut stdout_reader = BufReader::new(stdout);
            let mut pending_requests: HashMap<
                String,
                oneshot::Sender<std::result::Result<JsonRpcResponse, String>>,
            > = HashMap::new();
            let mut line = String::new();

            loop {
                tokio::select! {
                    req_opt = rx.recv() => {
                        match req_opt {
                            Some((req, response_tx)) => {
                                let id_str = req.id.as_ref().map(|id| id.to_string()).unwrap_or_default();
                                if req.id.is_some() {
                                    pending_requests.insert(id_str.clone(), response_tx);
                                }
                                let mut req_bytes = serde_json::to_vec(&req).unwrap_or_default();
                                req_bytes.push(b'\n');
                                if (stdin.write_all(&req_bytes).await.is_err() || stdin.flush().await.is_err()) && req.id.is_some() {
                                    if let Some(tx) = pending_requests.remove(&id_str) {
                                        let _ = tx.send(Err("Failed to write to stdin".to_string()));
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
                                if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&line) {
                                    let id_str = resp.id.as_ref().map(|id| id.to_string()).unwrap_or_default();
                                    if let Some(tx) = pending_requests.remove(&id_str) {
                                        let _ = tx.send(Ok(resp));
                                    }
                                }
                                line.clear();
                            }
                        }
                    }
                }
            }

            for (_, tx) in pending_requests {
                let _ = tx.send(Err("Process exited".to_string()));
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

        let broker = Self {
            manifest: manifest.clone(),
            event_bus: event_bus.clone(),
            tx,
            child: child_arc,
        };

        // Initialize handshake
        let init_params = serde_json::json!({
            "capabilities": manifest.capabilities,
            "version": manifest.version
        });

        let init_res = broker.call("initialize", Some(init_params)).await;
        if let Err(e) = init_res {
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

        let result = tokio::time::timeout(Duration::from_secs(30), resp_rx).await;
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
                Err("Oneshot channel dropped".to_string())
            }
            Err(_) => {
                self.event_bus.publish(RuntimeEvent::RpcResponse {
                    extension_id: self.manifest.id.clone(),
                    method: method.to_string(),
                    request_id: request_id.clone(),
                    success: false,
                });
                Err("Request timed out".to_string())
            }
        }
    }

    pub async fn shutdown(&self) {
        let mut lock = self.child.lock().await;
        if let Some(mut child) = lock.take() {
            let _ = child.kill().await;
            let status = child.wait().await;
            self.event_bus.publish(RuntimeEvent::ProcessExited {
                extension_id: self.manifest.id.clone(),
                exit_code: status.ok().and_then(|s| s.code()),
            });
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
        // Delegate to the trust builder so extension provenance,
        // `ExtensionDeclared` annotation source, and harness-side
        // trust normalization stay in one place. The previous
        // hand-rolled version here used `ToolAnnotations::default()`
        // and silently downgraded trust; routing through
        // `build_extension_tool_descriptor` closes that gap.
        crate::extension_trust::build_extension_tool_descriptor(
            &self.broker.manifest,
            &self.tool_decl,
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

        let content = res
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
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
