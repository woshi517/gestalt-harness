use gestalt_core::session::ExecutionMode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[cfg(feature = "skills")]
pub use crate::skills::SkillDescriptor;

#[cfg(not(feature = "skills"))]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub skill_path: std::path::PathBuf,
    pub triggers: Vec<String>,
    pub manifest_hash: String,
}

#[cfg(feature = "mcp")]
pub use crate::mcp::McpServerConfig;

#[cfg(not(feature = "mcp"))]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

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
    pub resolved_model: Option<gestalt_core::ResolvedModelSnapshot>,
    pub context_management_policy: Option<gestalt_core::ContextManagementPolicy>,
    pub bash_timeout_secs: Option<u64>,
    pub max_output_tokens: Option<usize>,
    pub allow_network: bool,
    pub environment: HashMap<String, String>,
    pub enabled_host_features: Vec<String>,
    pub tool_profile: Option<crate::tool_catalog_planner::ToolProfile>,
    /// Extension ids whose annotations are promoted to
    /// `BuiltInTrusted`. Extensions not in this list are treated as
    /// `ExtensionDeclared` regardless of manifest claims.
    #[serde(default)]
    pub trusted_extension_ids: Vec<String>,
    /// Discovered skill descriptors available at runtime startup.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovered_skills: Vec<SkillDescriptor>,
    /// Names of skills explicitly activated for this session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_skills: Vec<String>,
    /// Configured MCP servers
    #[serde(default)]
    pub mcp_servers: HashMap<String, McpServerConfig>,
    /// Threshold to switch to progressive discovery
    #[serde(default)]
    pub mcp_discovery_threshold: Option<usize>,
    /// Configured ignore patterns for file discovery and text search
    #[serde(default)]
    pub ignore_patterns: Vec<String>,
    pub top_p: Option<f32>,
    pub reasoning_effort: Option<gestalt_core::provider::ReasoningEffort>,
    pub text_verbosity: Option<gestalt_core::provider::TextVerbosity>,
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub extension_timeouts: ExtensionTimeoutsConfig,
    #[serde(default)]
    pub extension_limits: ExtensionLimitsConfig,
    #[serde(default)]
    pub extension_instances: BTreeMap<String, crate::extension::ExtensionInstanceConfig>,
    #[serde(default)]
    pub effective_config_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionTimeoutsConfig {
    pub initialize_ms: Option<u64>,
    pub hook_ms: Option<u64>,
    pub context_ms: Option<u64>,
    pub tool_ms: Option<u64>,
    pub shutdown_ms: Option<u64>,
}

impl Default for ExtensionTimeoutsConfig {
    fn default() -> Self {
        Self {
            initialize_ms: Some(10000),
            hook_ms: Some(5000),
            context_ms: Some(15000),
            tool_ms: Some(60000),
            shutdown_ms: Some(5000),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionLimitsConfig {
    pub max_message_bytes: Option<usize>,
    pub max_pending_requests: Option<usize>,
    pub max_protocol_errors: Option<usize>,
}

impl Default for ExtensionLimitsConfig {
    fn default() -> Self {
        Self {
            max_message_bytes: Some(8_388_608),
            max_pending_requests: Some(16),
            max_protocol_errors: Some(3),
        }
    }
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
            resolved_model: None,
            context_management_policy: Some(gestalt_core::ContextManagementPolicy::default()),
            bash_timeout_secs: None,
            max_output_tokens: None,
            allow_network: false,
            environment: HashMap::new(),
            enabled_host_features: Vec::new(),
            tool_profile: None,
            trusted_extension_ids: Vec::new(),
            discovered_skills: Vec::new(),
            active_skills: Vec::new(),
            mcp_servers: HashMap::new(),
            mcp_discovery_threshold: Some(5),
            ignore_patterns: Vec::new(),
            top_p: None,
            reasoning_effort: None,
            text_verbosity: None,
            metadata: serde_json::Value::Null,
            extension_timeouts: ExtensionTimeoutsConfig::default(),
            extension_limits: ExtensionLimitsConfig::default(),
            extension_instances: BTreeMap::new(),
            effective_config_fingerprint: None,
        }
    }
}
