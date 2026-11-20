#![forbid(unsafe_code)]
//! Official Model Context Protocol (MCP `2025-11-25`) stdio client bridge:
//! version negotiation (`initialize` / `notifications/initialized`), paginated
//! `tools/list` (`nextCursor`), `tools/call` (`content` + `structuredContent` +
//! `isError`), `notifications/cancelled`, server `ping` handling, and graceful
//! stdio EOF shutdown.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use gestalt_kernel::{ExtensionRegistry, SandboxPolicy, SideEffectClass, StepResult, ToolSpec};
use serde_json::{Value, json};

use crate::blobs::BlobStore;
use crate::config::ExternalExtensionConfig;
use crate::credentials::StreamRedactor;
use crate::sandbox::SandboxDriver;
use crate::stdio_rpc::{IncomingControl, StdioJsonRpcTransport};
use crate::{CancellationToken, RuntimeError};

/// Latest MCP protocol version requested by [`McpClient::connect`].
pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// Protocol versions accepted during MCP `initialize` version negotiation.
const SUPPORTED_MCP_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Hard cap on `tools/list` pagination pages to prevent infinite cursor loops.
const MAX_LIST_PAGES: usize = 64;

/// Negotiated MCP server metadata returned by `initialize`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpServerInfo {
    /// Negotiated protocol version string.
    pub protocol_version: String,
    /// Server capabilities JSON object.
    pub capabilities: Value,
    /// Server implementation metadata (`name`, `version`, etc.).
    pub server_info: Value,
    /// Optional server-provided usage instructions.
    pub instructions: Option<String>,
}

/// Stdio MCP client bridging an external MCP server into `ExtensionRegistry`.
pub struct McpClient {
    transport: StdioJsonRpcTransport,
    server: McpServerInfo,
}

impl McpClient {
    /// Spawn a stdio MCP server inside `sandbox_driver`, perform `initialize`
    /// with strict protocol version negotiation, and send
    /// `notifications/initialized`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if spawning, initialization, or version
    /// negotiation fails.
    pub async fn connect(
        name: String,
        config: &ExternalExtensionConfig,
        workspace_root: &Path,
        sandbox_policy: &SandboxPolicy,
        sandbox_driver: &Arc<dyn SandboxDriver>,
        redactor: StreamRedactor,
    ) -> Result<Self, RuntimeError> {
        let mut transport = StdioJsonRpcTransport::spawn(
            name.clone(),
            config,
            workspace_root,
            sandbox_policy,
            sandbox_driver,
            redactor,
        )?;

        let init_res = transport
            .call_rpc(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "gestalt-runtime",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
                CancellationToken::new(),
                true,
                handle_mcp_server_message,
            )
            .await?;

        let negotiated_version = init_res
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !SUPPORTED_MCP_VERSIONS.contains(&negotiated_version.as_str()) {
            transport.close_stdin_and_shutdown(100).await;
            return Err(RuntimeError::IpcProtocol {
                ext_name: name,
                message: format!(
                    "unsupported MCP protocolVersion '{negotiated_version}' (supported: {})",
                    SUPPORTED_MCP_VERSIONS.join(", ")
                ),
            });
        }

        let server = McpServerInfo {
            protocol_version: negotiated_version,
            capabilities: init_res
                .get("capabilities")
                .cloned()
                .unwrap_or_else(|| json!({})),
            server_info: init_res
                .get("serverInfo")
                .cloned()
                .unwrap_or_else(|| json!({})),
            instructions: init_res
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_string),
        };

        transport
            .send_notification("notifications/initialized", json!({}))
            .await?;

        Ok(Self { transport, server })
    }

    /// Borrow negotiated [`McpServerInfo`].
    #[must_use]
    pub fn server_info(&self) -> &McpServerInfo {
        &self.server
    }

    /// Discover all tools exposed by this MCP server across all `nextCursor`
    /// pages (`tools/list`), defaulting each tool's [`SideEffectClass`] to
    /// [`SideEffectClass::Irreversible`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on IPC timeout, cursor cycle, or malformed response.
    pub async fn list_tools(&mut self) -> Result<Vec<ToolSpec>, RuntimeError> {
        let mut specs = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = BTreeSet::new();

        for _ in 0..MAX_LIST_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let res = self
                .transport
                .call_rpc(
                    "tools/list",
                    params,
                    CancellationToken::new(),
                    false,
                    handle_mcp_server_message,
                )
                .await?;

            let tools_arr = res.get("tools").and_then(Value::as_array).ok_or_else(|| {
                RuntimeError::IpcProtocol {
                    ext_name: self.transport.name().to_string(),
                    message: "MCP tools/list response missing 'tools' array".to_string(),
                }
            })?;

            for item in tools_arr {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| RuntimeError::IpcProtocol {
                        ext_name: self.transport.name().to_string(),
                        message: "MCP tool entry missing 'name'".to_string(),
                    })?
                    .to_string();
                let description = item
                    .get("description")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("title").and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string();
                let parameters_schema = item
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"}));

                specs.push(ToolSpec {
                    name,
                    description,
                    parameters_schema,
                    default_side_effect: SideEffectClass::Irreversible,
                    source_extension: Some(self.transport.name().to_string()),
                });
            }

            match res.get("nextCursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() => {
                    if !seen_cursors.insert(next.to_string()) {
                        return Err(RuntimeError::IpcProtocol {
                            ext_name: self.transport.name().to_string(),
                            message: format!("MCP tools/list pagination cycle on cursor '{next}'"),
                        });
                    }
                    cursor = Some(next.to_string());
                }
                _ => break,
            }
        }

        Ok(specs)
    }

    /// Register all discovered MCP tools into [`ExtensionRegistry`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `tools/list` fails.
    pub async fn register_tools(
        &mut self,
        registry: &mut ExtensionRegistry,
    ) -> Result<Vec<String>, RuntimeError> {
        let specs = self.list_tools().await?;
        let mut names = Vec::with_capacity(specs.len());
        for spec in specs {
            names.push(spec.name.clone());
            registry.tools.insert(spec.name.clone(), spec);
        }
        Ok(names)
    }

    /// Invoke an MCP tool (`tools/call`), preserving both `content` and optional
    /// `structuredContent`, redacting secrets, and spilling outputs exceeding
    /// `32 KB` to [`BlobStore`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on IPC timeout, cancellation, or protocol error.
    pub async fn call_tool(
        &mut self,
        tool_name: &str,
        args: &Value,
        effect_id: &str,
        blob_store: &BlobStore,
        cancel: CancellationToken,
    ) -> Result<StepResult, RuntimeError> {
        let params = if args.is_null() {
            json!({ "name": tool_name })
        } else {
            json!({ "name": tool_name, "arguments": args })
        };
        let res = self
            .transport
            .call_rpc(
                "tools/call",
                params,
                cancel,
                false,
                handle_mcp_server_message,
            )
            .await?;

        let is_error = res.get("isError").and_then(Value::as_bool).unwrap_or(false);
        let mut payload = match (res.get("content"), res.get("structuredContent")) {
            (Some(content), Some(structured)) => {
                json!({ "content": content, "structuredContent": structured })
            }
            (Some(content), None) => content.clone(),
            (None, Some(structured)) => structured.clone(),
            (None, None) => res,
        };
        self.transport.redactor().redact_json(&mut payload);

        let serialized = serde_json::to_string(&payload)?;
        let (final_output, blob_digest) = if serialized.len() > blob_store.spill_threshold() {
            let (preview, digest) = blob_store.spill_if_oversized(&serialized)?;
            (Value::String(preview), digest)
        } else {
            (payload, None)
        };

        Ok(StepResult::Tool {
            effect_id: effect_id.to_string(),
            output: final_output,
            is_error,
            blob_digest,
        })
    }

    /// Gracefully shut down the MCP server following the official stdio
    /// lifecycle: close `stdin` first, wait for clean exit, then terminate.
    pub async fn shutdown(&mut self) {
        self.transport.close_stdin_and_shutdown(250).await;
    }
}

/// Handle server-initiated JSON-RPC requests (`ping`, etc.) and notifications
/// per the MCP specification so the server never deadlocks awaiting a response.
fn handle_mcp_server_message(msg: &Value) -> (Option<Value>, IncomingControl) {
    if let Some(id) = msg.get("id")
        && let Some(method) = msg.get("method").and_then(Value::as_str)
    {
        let reply = if method == "ping" {
            json!({ "jsonrpc": "2.0", "id": id, "result": {} })
        } else {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method '{method}' not supported by client") }
            })
        };
        return (Some(reply), IncomingControl::Continue);
    }
    (None, IncomingControl::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::UnsandboxedFallbackDriver;

    #[cfg(unix)]
    #[tokio::test]
    async fn mcp_client_negotiates_version_paginates_tools_handles_ping_and_structured_content() {
        let dir = std::env::temp_dir().join(format!("gestalt_mcp_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let script_path = dir.join("mcp.sh");
        std::fs::write(
            &script_path,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"demo","version":"1.0"},"instructions":"be safe"}}\n' "$id"
      ;;
    *'"method":"tools/list"'*'"cursor":"page2"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"mcp_pong","description":"pong","inputSchema":{"type":"object"}}]}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":999,"method":"ping"}\n'
      IFS= read -r _pong
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"mcp_ping","description":"ping","inputSchema":{"type":"object"}}],"nextCursor":"page2"}}\n' "$id"
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"isError":false,"content":[{"type":"text","text":"ok"}],"structuredContent":{"code":200}}}\n' "$id"
      ;;
  esac
done
"#,
        )
        .expect("write script");

        let cfg = ExternalExtensionConfig {
            command: vec!["sh".to_string(), script_path.to_string_lossy().to_string()],
            protocol: crate::config::ExtensionProtocol::Mcp,
            timeout_ms: 1_500,
        };
        let mut client = McpClient::connect(
            "mock_mcp".to_string(),
            &cfg,
            &dir,
            &SandboxPolicy::default(),
            &(Arc::new(UnsandboxedFallbackDriver) as Arc<dyn SandboxDriver>),
            StreamRedactor::default(),
        )
        .await
        .expect("connect mcp");
        assert_eq!(client.server_info().protocol_version, "2025-11-25");
        assert_eq!(
            client.server_info().instructions.as_deref(),
            Some("be safe")
        );

        let mut reg = ExtensionRegistry::new();
        let names = client
            .register_tools(&mut reg)
            .await
            .expect("list paginated");
        assert_eq!(names, vec!["mcp_ping", "mcp_pong"]);
        assert_eq!(
            reg.tools["mcp_ping"].default_side_effect,
            SideEffectClass::Irreversible
        );

        let blobs = BlobStore::new(&dir);
        let res = client
            .call_tool(
                "mcp_ping",
                &json!({}),
                "eff_1",
                &blobs,
                CancellationToken::new(),
            )
            .await
            .expect("call_tool");
        match res {
            StepResult::Tool {
                output,
                is_error: false,
                ..
            } => {
                assert_eq!(output.pointer("/structuredContent/code"), Some(&json!(200)));
            }
            other => panic!("unexpected result: {other:?}"),
        }

        client.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
