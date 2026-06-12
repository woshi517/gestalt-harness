use gestalt_core::session::ExecutionMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub workspace_root: PathBuf,
    pub execution_mode: ExecutionMode,
    pub max_turns: usize,
    pub model: String,
    pub provider: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub max_context_window: Option<usize>,
    pub reserved_output_tokens: Option<usize>,
    pub bash_timeout_secs: Option<u64>,
    pub max_output_tokens: Option<usize>,
    pub allow_network: bool,
    pub environment: HashMap<String, String>,
    pub enabled_cli_features: Vec<String>,
    pub tool_profile: Option<crate::tool_catalog_planner::ToolProfile>,
    /// Extension ids whose annotations are promoted to
    /// `BuiltInTrusted`. Extensions not in this list are treated as
    /// `ExtensionDeclared` regardless of manifest claims.
    #[serde(default)]
    pub trusted_extension_ids: Vec<String>,
    /// Discovered skill descriptors available at runtime startup.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovered_skills: Vec<gestalt_skills::SkillDescriptor>,
    /// Names of skills explicitly activated for this session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_skills: Vec<String>,
    /// Configured MCP servers
    #[serde(default)]
    pub mcp_servers: HashMap<String, gestalt_mcp::McpServerConfig>,
    /// Threshold to switch to progressive discovery
    #[serde(default)]
    pub mcp_discovery_threshold: Option<usize>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            execution_mode: ExecutionMode::Confirm,
            max_turns: 10,
            model: String::new(),
            provider: String::new(),
            max_tokens: 4096,
            temperature: Some(0.0),
            max_context_window: None,
            reserved_output_tokens: None,
            bash_timeout_secs: None,
            max_output_tokens: None,
            allow_network: false,
            environment: HashMap::new(),
            enabled_cli_features: Vec::new(),
            tool_profile: None,
            trusted_extension_ids: Vec::new(),
            discovered_skills: Vec::new(),
            active_skills: Vec::new(),
            mcp_servers: HashMap::new(),
            mcp_discovery_threshold: Some(5),
        }
    }
}
