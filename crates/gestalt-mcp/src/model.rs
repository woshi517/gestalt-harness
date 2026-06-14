use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use schemars::JsonSchema;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct McpTimeoutsConfig {
    pub connect_ms: Option<u64>,
    pub request_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        env: HashMap<String, String>,
    },
    #[serde(alias = "sse", alias = "remote")]
    Http {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpLifecycleMode {
    #[default]
    Lazy,
    AlwaysOn,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub transport: McpTransportConfig,
    #[serde(default)]
    pub lifecycle: McpLifecycleMode,
    #[serde(default)]
    pub trust_level: Option<String>,
    #[serde(default)]
    pub allow_sampling: bool,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub tool_annotations: HashMap<String, HashMap<String, String>>,
    #[serde(default)]
    pub timeouts: Option<McpTimeoutsConfig>,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct McpServerId(pub String);

impl std::fmt::Display for McpServerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct McpToolIdentity {
    pub server_id: McpServerId,
    pub tool_name: String,
}

impl McpToolIdentity {
    pub fn new(server_id: McpServerId, tool_name: String) -> Self {
        Self { server_id, tool_name }
    }

    pub fn to_canonical_id(&self) -> gestalt_core::tool_descriptor::CanonicalToolId {
        gestalt_core::tool_descriptor::CanonicalToolId {
            namespace: gestalt_core::tool_descriptor::ToolNamespace::Mcp(self.server_id.0.clone()),
            name: self.tool_name.clone(),
        }
    }
}

impl std::fmt::Display for McpToolIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mcp:{}:{}", self.server_id.0, self.tool_name)
    }
}

impl std::str::FromStr for McpToolIdentity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 || parts[0] != "mcp" {
            return Err(format!("Invalid McpToolIdentity format: {}", s));
        }
        Ok(Self {
            server_id: McpServerId(parts[1].to_string()),
            tool_name: parts[2].to_string(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolSummary {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpCallResult {
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerState {
    pub server_id: McpServerId,
    pub connection_state: McpConnectionState,
    pub tool_count: usize,
    pub trust_level: Option<String>,
    pub discovery_mode: bool,
    pub cache_fresh: bool,
    pub last_error: Option<String>,
}

pub fn parse_mcp_call_result(val: &serde_json::Value) -> Result<McpCallResult, String> {
    let obj = val.as_object().ok_or_else(|| "Call tool result must be a JSON object".to_string())?;
    
    let is_error = obj.get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
        
    let mut text_parts = Vec::new();
    if let Some(content) = obj.get("content") {
        if let Some(arr) = content.as_array() {
            for item in arr {
                if let Some(item_obj) = item.as_object() {
                    let type_str = item_obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if type_str == "text" {
                        if let Some(text) = item_obj.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(text.to_string());
                        }
                    } else if type_str == "image" {
                        text_parts.push("[Image content returned]".to_string());
                    } else if type_str == "resource" {
                        if let Some(resource) = item_obj.get("resource") {
                            if let Some(text) = resource.get("text").and_then(|v| v.as_str()) {
                                text_parts.push(text.to_string());
                            }
                        }
                    }
                }
            }
        } else if let Some(s) = content.as_str() {
            text_parts.push(s.to_string());
        }
    }
    
    let content = if text_parts.is_empty() {
        val.to_string()
    } else {
        text_parts.join("\n")
    };
    
    Ok(McpCallResult { content, is_error })
}

#[derive(Debug, Clone)]
pub enum McpRegistryEvent {
    Connecting { server_name: String },
    Connected { server_name: String, protocol_version: String, tool_count: usize },
    ConnectionFailed { server_name: String, reason: String },
    ToolCatalogRefreshed { server_name: String, tool_count: usize, schema_hash: String },
    ToolListChanged { server_name: String },
}

pub type McpEventCallback = std::sync::Arc<dyn Fn(McpRegistryEvent) + Send + Sync>;

