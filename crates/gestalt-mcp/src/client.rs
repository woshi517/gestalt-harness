use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::error::{McpError, Result};
use crate::model::{
    parse_mcp_call_result, McpCallResult, McpServerConfig, McpServerId, McpToolSchema,
    McpTransportConfig,
};
use crate::transport::stdio::StdioTransport;
use crate::transport::McpTransport;

pub struct McpClient {
    pub server_id: McpServerId,
    pub config: McpServerConfig,
    transport: Arc<dyn McpTransport>,
    cached_tools: Arc<Mutex<Option<Vec<McpToolSchema>>>>,
    capabilities: Arc<Mutex<Option<Value>>>,
    event_callback: Option<crate::model::McpEventCallback>,
}

impl McpClient {
    pub async fn connect(
        config: McpServerConfig,
        workspace_root: &std::path::Path,
        event_callback: Option<crate::model::McpEventCallback>,
    ) -> Result<Self> {
        let server_id = McpServerId(config.name.clone());
        let transport: Arc<dyn McpTransport> = match &config.transport {
            McpTransportConfig::Stdio {
                command,
                args,
                cwd,
                env,
            } => {
                let mut merged_env = env.clone();
                for (k, v) in &config.env {
                    merged_env.insert(k.clone(), v.clone());
                }
                let resolved_cwd = cwd.as_ref().map(|dir| {
                    let path = std::path::Path::new(dir);
                    if path.is_relative() {
                        workspace_root.join(path).to_string_lossy().into_owned()
                    } else {
                        dir.clone()
                    }
                });
                let stdio = StdioTransport::spawn(
                    &config.name,
                    command,
                    args,
                    resolved_cwd.as_deref(),
                    &merged_env,
                )
                .await?;
                Arc::new(stdio)
            }
            McpTransportConfig::Http { .. } => {
                return Err(McpError::Config(
                    "HTTP transport is not supported in this version".to_string(),
                ));
            }
        };

        let client = Self {
            server_id: server_id.clone(),
            config,
            transport: transport.clone(),
            cached_tools: Arc::new(Mutex::new(None)),
            capabilities: Arc::new(Mutex::new(None)),
            event_callback: event_callback.clone(),
        };

        client.initialize(workspace_root).await?;

        // Spawn a background task to listen to notifications and handle tools/list_changed
        let cached_tools_clone = client.cached_tools.clone();
        let transport_clone = transport.clone();
        let server_id_clone = server_id.clone();
        let event_callback_clone = event_callback.clone();
        tokio::spawn(async move {
            while let Some((method, _params)) = transport_clone.recv_notification().await {
                if method == "notifications/tools/list_changed" {
                    eprintln!("[MCP Client '{}'] Received tools/list_changed notification. Invalidating cache.", server_id_clone);
                    let mut lock = cached_tools_clone.lock().await;
                    *lock = None;
                    if let Some(ref cb) = event_callback_clone {
                        cb(crate::model::McpRegistryEvent::ToolListChanged {
                            server_name: server_id_clone.0.clone(),
                        });
                    }
                }
            }
        });

        Ok(client)
    }

    async fn initialize(&self, workspace_root: &std::path::Path) -> Result<()> {
        let root_path_str = workspace_root.to_string_lossy().to_string();
        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "roots": {
                    "listChanged": false
                }
            },
            "clientInfo": {
                "name": "gestalt-harness",
                "version": env!("CARGO_PKG_VERSION")
            },
            "roots": [
                {
                    "uri": format!("file://{}", root_path_str),
                    "name": "workspace"
                }
            ]
        });

        let resp = self.transport.call("initialize", Some(init_params)).await?;

        if let Some(capabilities) = resp.get("capabilities") {
            let mut lock = self.capabilities.lock().await;
            *lock = Some(capabilities.clone());
        }

        self.transport
            .notify("initialized", Some(serde_json::json!({})))
            .await?;

        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolSchema>> {
        {
            let lock = self.cached_tools.lock().await;
            if let Some(ref tools) = *lock {
                return Ok(tools.clone());
            }
        }

        let resp = self.transport.call("tools/list", None).await?;
        let tools_val = resp.get("tools").ok_or_else(|| {
            McpError::Protocol("Server response for tools/list did not contain 'tools'".to_string())
        })?;

        let raw_tools: Vec<Value> = serde_json::from_value(tools_val.clone())
            .map_err(|e| McpError::Protocol(format!("Failed to parse tools array: {}", e)))?;

        let mut tools = Vec::new();
        for tool_val in raw_tools {
            let name = tool_val
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::Protocol("Tool definition missing 'name'".to_string()))?
                .to_string();
            let description = tool_val
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = tool_val
                .get("inputSchema")
                .cloned()
                .unwrap_or(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }));

            tools.push(McpToolSchema {
                name,
                description,
                input_schema,
            });
        }

        let mut lock = self.cached_tools.lock().await;
        *lock = Some(tools.clone());

        // Emit catalog refreshed event!
        if let Some(ref cb) = self.event_callback {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            for t in &tools {
                hasher.update(t.name.as_bytes());
                hasher.update(t.description.as_bytes());
                hasher.update(
                    serde_json::to_string(&t.input_schema)
                        .unwrap_or_default()
                        .as_bytes(),
                );
            }
            let hash_str = format!("{:x}", hasher.finalize());

            cb(crate::model::McpRegistryEvent::ToolCatalogRefreshed {
                server_name: self.server_id.0.clone(),
                tool_count: tools.len(),
                schema_hash: hash_str,
            });
        }

        Ok(tools)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpCallResult> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments
        });

        let resp = self.transport.call("tools/call", Some(params)).await?;
        parse_mcp_call_result(&resp).map_err(McpError::Protocol)
    }

    pub async fn is_cache_fresh(&self) -> bool {
        let lock = self.cached_tools.lock().await;
        lock.is_some()
    }

    pub fn get_cached_tools(&self) -> Option<Vec<McpToolSchema>> {
        if let Ok(lock) = self.cached_tools.try_lock() {
            lock.clone()
        } else {
            None
        }
    }

    pub async fn shutdown(&self) {
        self.transport.shutdown().await;
    }
}
