use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use gestalt_core::{
    ConfigError, ExecutionMode, HarnessError, PromptAssemblyStrategy, ProviderError,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

fn default_version() -> u32 {
    1
}

fn version_schema(_gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
    let mut schema = schemars::schema::SchemaObject::default();
    schema.instance_type = Some(schemars::schema::InstanceType::Integer.into());
    schema.const_value = Some(serde_json::json!(1));
    schemars::schema::Schema::Object(schema)
}

fn is_json_output() -> bool {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if (args[i] == "--format" || args[i] == "-f") && i + 1 < args.len() && args[i + 1] == "json"
        {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SandboxType {
    None,
    Docker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    Jsonl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    Confirm,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ProviderKind {
    Openai,
    Anthropic,
    OpenaiCompatible,
    Custom(String),
}

impl schemars::JsonSchema for ProviderKind {
    fn schema_name() -> String {
        "ProviderKind".to_string()
    }

    fn json_schema(gen: &mut schemars::gen::SchemaGenerator) -> schemars::schema::Schema {
        String::json_schema(gen)
    }
}

impl From<String> for ProviderKind {
    fn from(s: String) -> Self {
        match s.as_str() {
            "openai" => Self::Openai,
            "anthropic" => Self::Anthropic,
            "openai-compatible" => Self::OpenaiCompatible,
            _ => Self::Custom(s),
        }
    }
}

impl From<ProviderKind> for String {
    fn from(k: ProviderKind) -> Self {
        match k {
            ProviderKind::Openai => "openai".to_string(),
            ProviderKind::Anthropic => "anthropic".to_string(),
            ProviderKind::OpenaiCompatible => "openai-compatible".to_string(),
            ProviderKind::Custom(s) => s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
    Xhigh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextVerbosity {
    None,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicThinkingConfig {
    Adaptive { effort: String },
    Enabled { budget_tokens: u32 },
    Disabled,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelOptionsConfig {
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub text_verbosity: Option<TextVerbosity>,
    pub thinking: Option<AnthropicThinkingConfig>,
    pub adapter_options: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelVariantConfig {
    pub extends: Option<String>,
    pub options: Option<ModelOptionsConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelDefinitionConfig {
    pub display_name: Option<String>,
    pub options: Option<ModelOptionsConfig>,
    #[serde(default)]
    pub variants: HashMap<String, ModelVariantConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderRequestConfig {
    pub timeout_ms: Option<u64>,
    pub stream_chunk_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub protocol: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub api_key_env: Option<String>,
    pub auth_ref: Option<String>,
    pub kind: Option<ProviderKind>,
    pub models_endpoint: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub request: Option<ProviderRequestConfig>,
    pub capabilities: Option<serde_json::Value>,
    #[serde(default)]
    pub models: HashMap<String, ModelDefinitionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMetaConfig {
    #[serde(default)]
    pub initialized: bool,
    #[serde(default)]
    pub format_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    #[serde(default, rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default = "default_version")]
    #[schemars(schema_with = "version_schema")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceMetaConfig>,
    #[serde(default)]
    pub defaults: Option<DefaultsConfig>,
    #[serde(default)]
    pub tools: Option<ToolsConfig>,
    #[serde(default)]
    pub context: Option<ContextConfig>,
    #[serde(default)]
    pub observe: Option<ObserveConfig>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
    #[serde(default)]
    pub tui: Option<TuiConfig>,
    #[serde(default)]
    pub prompt: Option<PromptConfig>,
    #[serde(default)]
    pub policies: Option<PoliciesConfig>,
    #[serde(default)]
    pub extensions: Option<ExtensionsConfig>,
    #[serde(default)]
    pub skills: Option<SkillsConfig>,
    #[serde(default)]
    pub mcp: Option<McpConfig>,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            schema: None,
            version: 1,
            workspace: None,
            defaults: None,
            tools: None,
            context: None,
            observe: None,
            providers: HashMap::new(),
            profiles: HashMap::new(),
            tui: None,
            prompt: None,
            policies: None,
            extensions: None,
            skills: None,
            mcp: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtensionTimeoutsConfig {
    pub initialize_ms: Option<u64>,
    pub hook_ms: Option<u64>,
    pub context_ms: Option<u64>,
    pub tool_ms: Option<u64>,
    pub shutdown_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtensionLimitsConfig {
    pub max_message_bytes: Option<usize>,
    pub max_pending_requests: Option<usize>,
    pub max_protocol_errors: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExtensionsConfig {
    #[serde(default)]
    pub explicit_loads: Vec<String>,
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub trusted: Vec<String>,
    #[serde(default)]
    pub allow_untrusted: bool,
    #[serde(default)]
    pub timeouts: ExtensionTimeoutsConfig,
    #[serde(default)]
    pub limits: ExtensionLimitsConfig,
    #[serde(default)]
    pub instances: BTreeMap<String, ExtensionInstanceConfig>,
}

fn default_extension_instance_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInstanceConfig {
    pub package: String,
    #[serde(default = "default_extension_instance_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub components: BTreeMap<String, bool>,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub grants: ExtensionGrantConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionGrantConfig {
    #[serde(default, alias = "workspaceRead")]
    pub workspace_read: bool,
    #[serde(default, alias = "workspaceWrite")]
    pub workspace_write: bool,
    #[serde(default)]
    pub shell: bool,
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default, alias = "allowedPaths")]
    pub allowed_paths: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkillsConfig {
    #[serde(default)]
    pub explicit_paths: Vec<String>,
    #[serde(default)]
    pub active: Vec<String>,
    #[serde(default)]
    pub trusted: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, gestalt_mcp::McpServerConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_threshold: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PromptConfig {
    #[serde(default, rename = "override", skip_serializing_if = "Option::is_none")]
    pub r#override: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assembly_strategy: Option<PromptAssemblyStrategy>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoliciesConfig {
    #[serde(default)]
    pub paths: PolicyPathsConfig,
    #[serde(default)]
    pub bash: PolicyBashConfig,
    #[serde(default)]
    pub network: PolicyNetworkConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyPathsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_read: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_write: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_write: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_read: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyBashConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<PolicyAction>,
    #[serde(default, rename = "allow", skip_serializing_if = "Option::is_none")]
    pub allow: Option<Vec<String>>,
    #[serde(default, rename = "confirm", skip_serializing_if = "Option::is_none")]
    pub confirm: Option<Vec<String>>,
    #[serde(default, rename = "deny", skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,
    #[serde(default, skip_serializing)]
    pub yolo_allow: Option<Vec<String>>,
    #[serde(default, skip_serializing)]
    pub always_confirm: Option<Vec<String>>,
    #[serde(default, skip_serializing)]
    pub always_deny: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PolicyNetworkConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<PolicyAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_domains: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny_domains: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TuiConfig {
    #[serde(default)]
    pub diagnostics: Option<TuiDiagnosticsConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TuiDiagnosticsConfig {
    pub max_log_lines: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DefaultsConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub mode: Option<ExecutionMode>,
    pub max_turns: Option<usize>,
    pub profile: Option<String>,
    pub max_output_tokens: Option<usize>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub mode: Option<ExecutionMode>,
    pub max_turns: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub variant: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolsConfig {
    pub default_timeout_secs: Option<u64>,
    pub bash_timeout_secs: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub max_output_tokens: Option<usize>,
    pub max_parallel_calls: Option<usize>,
    pub sandbox_type: Option<SandboxType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore_patterns: Option<Vec<String>>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: Some(60),
            bash_timeout_secs: Some(60),
            max_output_bytes: Some(1_048_576),
            max_output_tokens: Some(4000),
            max_parallel_calls: Some(4),
            sandbox_type: Some(SandboxType::None),
            ignore_patterns: None,
        }
    }
}

pub use gestalt_runtime::workspace_context::{
    ContextSnapshotMode, MemoryContextConfig, MemorySelectionStrategy, MemoryWriteMode,
    WorkspaceContextConfig,
};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ContextConfig {
    pub context_window_override: Option<usize>,
    pub reserved_output_tokens: Option<usize>,
    pub safety_margin_tokens: Option<usize>,
    pub workspace_file: Option<String>,
    pub memory_file: Option<String>,
    #[serde(default, skip_serializing)]
    pub max_context_window: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceContextConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryContextConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub management: Option<gestalt_core::ContextManagementPolicy>,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            context_window_override: Some(120_000),
            reserved_output_tokens: Some(8_000),
            safety_margin_tokens: Some(2048),
            workspace_file: None,
            memory_file: None,
            max_context_window: None,
            workspace: None,
            memory: None,
            management: Some(gestalt_core::ContextManagementPolicy::default()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveConfig {
    pub run_log_dir: Option<String>,
    pub log_format: Option<LogFormat>,
}

impl Default for ObserveConfig {
    fn default() -> Self {
        Self {
            run_log_dir: Some(".gestalt/runs".to_string()),
            log_format: Some(LogFormat::Jsonl),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct LegacyPoliciesConfig {
    #[serde(default)]
    pub prompt: Option<PromptConfig>,
    #[serde(default)]
    pub paths: PolicyPathsConfig,
    #[serde(default)]
    pub tools: LegacyToolsConfig,
    #[serde(default)]
    pub network: PolicyNetworkConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct LegacyToolsConfig {
    #[serde(default)]
    pub bash: PolicyBashConfig,
}

fn with_default<T>(value: Option<Vec<T>>, default: Vec<T>) -> Vec<T> {
    value.filter(|items| !items.is_empty()).unwrap_or(default)
}

impl PoliciesConfig {
    pub fn to_policy_config(&self) -> gestalt_policy::PolicyConfig {
        let default_paths = gestalt_policy::PathPolicy::default();
        let default_bash = gestalt_policy::BashPolicy::default();
        let default_network = gestalt_policy::NetworkPolicy::default();

        let bash_default = self
            .bash
            .default
            .map(|a| match a {
                PolicyAction::Allow => gestalt_policy::PolicyAction::Allow,
                PolicyAction::Confirm => gestalt_policy::PolicyAction::Confirm,
                PolicyAction::Deny => gestalt_policy::PolicyAction::Deny,
            })
            .unwrap_or(default_bash.default);

        let mut allow_list = self.bash.allow.clone().unwrap_or_default();
        if let Some(ref yolo) = self.bash.yolo_allow {
            allow_list.extend(yolo.clone());
        }
        let allow_list = with_default(
            if allow_list.is_empty() {
                None
            } else {
                Some(allow_list)
            },
            default_bash.yolo_allow,
        );

        let mut confirm_list = self.bash.confirm.clone().unwrap_or_default();
        if let Some(ref conf) = self.bash.always_confirm {
            confirm_list.extend(conf.clone());
        }
        let confirm_list = with_default(
            if confirm_list.is_empty() {
                None
            } else {
                Some(confirm_list)
            },
            default_bash.always_confirm,
        );

        let mut deny_list = self.bash.deny.clone().unwrap_or_default();
        if let Some(ref deny) = self.bash.always_deny {
            deny_list.extend(deny.clone());
        }
        let deny_list = with_default(
            if deny_list.is_empty() {
                None
            } else {
                Some(deny_list)
            },
            default_bash.always_deny,
        );

        let network_default = self
            .network
            .default
            .map(|a| match a {
                PolicyAction::Allow => gestalt_policy::PolicyAction::Allow,
                PolicyAction::Confirm => gestalt_policy::PolicyAction::Confirm,
                PolicyAction::Deny => gestalt_policy::PolicyAction::Deny,
            })
            .unwrap_or(default_network.default);

        gestalt_policy::PolicyConfig {
            paths: gestalt_policy::PathPolicy {
                allow_read: with_default(self.paths.allow_read.clone(), default_paths.allow_read),
                allow_write: with_default(
                    self.paths.allow_write.clone(),
                    default_paths.allow_write,
                ),
                deny_write: with_default(self.paths.deny_write.clone(), default_paths.deny_write),
                deny_read: with_default(self.paths.deny_read.clone(), default_paths.deny_read),
            },
            bash: gestalt_policy::BashPolicy {
                default: bash_default,
                yolo_allow: allow_list,
                always_confirm: confirm_list,
                always_deny: deny_list,
            },
            network: gestalt_policy::NetworkPolicy {
                default: network_default,
                allow_domains: with_default(
                    self.network.allow_domains.clone(),
                    default_network.allow_domains,
                ),
                deny_domains: with_default(
                    self.network.deny_domains.clone(),
                    default_network.deny_domains,
                ),
            },
            memory_paths: Vec::new(),
        }
    }

    pub fn from_policy_config(config: &gestalt_policy::PolicyConfig) -> Self {
        Self {
            paths: PolicyPathsConfig {
                allow_read: Some(config.paths.allow_read.clone()),
                allow_write: Some(config.paths.allow_write.clone()),
                deny_write: Some(config.paths.deny_write.clone()),
                deny_read: Some(config.paths.deny_read.clone()),
            },
            bash: PolicyBashConfig {
                default: Some(match config.bash.default {
                    gestalt_policy::PolicyAction::Allow => PolicyAction::Allow,
                    gestalt_policy::PolicyAction::Confirm => PolicyAction::Confirm,
                    gestalt_policy::PolicyAction::Deny => PolicyAction::Deny,
                }),
                allow: Some(config.bash.yolo_allow.clone()),
                confirm: Some(config.bash.always_confirm.clone()),
                deny: Some(config.bash.always_deny.clone()),
                yolo_allow: Some(config.bash.yolo_allow.clone()),
                always_confirm: Some(config.bash.always_confirm.clone()),
                always_deny: Some(config.bash.always_deny.clone()),
            },
            network: PolicyNetworkConfig {
                default: Some(match config.network.default {
                    gestalt_policy::PolicyAction::Allow => PolicyAction::Allow,
                    gestalt_policy::PolicyAction::Confirm => PolicyAction::Confirm,
                    gestalt_policy::PolicyAction::Deny => PolicyAction::Deny,
                }),
                allow_domains: Some(config.network.allow_domains.clone()),
                deny_domains: Some(config.network.deny_domains.clone()),
            },
        }
    }
}

pub fn workspace_config_path(root: &Path) -> PathBuf {
    root.join("gestalt.json")
}

pub fn legacy_workspace_config_path(root: &Path) -> PathBuf {
    root.join(".gestalt/config.toml")
}

pub fn legacy_workspace_policies_path(root: &Path) -> PathBuf {
    root.join(".gestalt/policies.toml")
}

pub fn global_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gestalt/gestalt.json")
}

pub fn legacy_global_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gestalt/config.toml")
}

fn load_workspace_config_file(path: &Path) -> Result<WorkspaceConfig, HarnessError> {
    let input = fs::read_to_string(path).map_err(|err| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: path.display().to_string(),
            reason: err.to_string(),
        })
    })?;

    match path.extension().and_then(|ext| ext.to_str()) {
        Some("json") => serde_json::from_str(&input).map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: path.display().to_string(),
                reason: err.to_string(),
            })
        }),
        _ => toml::from_str(&input).map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: path.display().to_string(),
                reason: err.to_string(),
            })
        }),
    }
}

fn load_legacy_policies_file(path: &Path) -> Result<WorkspaceConfig, HarnessError> {
    if !is_json_output() {
        eprintln!(
            "Warning: Loading legacy TOML policies configuration from '{}' is deprecated and will be removed in a future version. Please migrate to gestalt.json.",
            path.display()
        );
    }
    let input = fs::read_to_string(path).map_err(|err| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: path.display().to_string(),
            reason: err.to_string(),
        })
    })?;
    let raw: LegacyPoliciesConfig = toml::from_str(&input).map_err(|err| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: path.display().to_string(),
            reason: err.to_string(),
        })
    })?;

    Ok(WorkspaceConfig {
        prompt: raw.prompt,
        policies: Some(PoliciesConfig {
            paths: raw.paths,
            bash: raw.tools.bash,
            network: raw.network,
        }),
        ..WorkspaceConfig::default()
    })
}

pub fn write_workspace_config_file(
    path: &Path,
    config: &WorkspaceConfig,
) -> Result<(), HarnessError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: path.display().to_string(),
                reason: err.to_string(),
            })
        })?;
    }
    let serialized = serde_json::to_string_pretty(config).map_err(|err| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: path.display().to_string(),
            reason: err.to_string(),
        })
    })?;
    fs::write(path, serialized).map_err(|err| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: path.display().to_string(),
            reason: err.to_string(),
        })
    })
}

fn seed_workspace_config_from_legacy(root: &Path) -> Result<WorkspaceConfig, HarnessError> {
    let mut config = WorkspaceConfig::default();

    let legacy_config_path = legacy_workspace_config_path(root);
    if legacy_config_path.exists() {
        config = config.merge(load_workspace_config_file(&legacy_config_path)?)?;
    }

    let legacy_policies_path = legacy_workspace_policies_path(root);
    if legacy_policies_path.exists() {
        config = config.merge(load_legacy_policies_file(&legacy_policies_path)?)?;
    }

    Ok(config)
}

fn seed_global_config_from_legacy() -> Result<WorkspaceConfig, HarnessError> {
    let legacy_global_path = legacy_global_config_path();
    if legacy_global_path.exists() {
        load_workspace_config_file(&legacy_global_path)
    } else {
        Ok(WorkspaceConfig::default())
    }
}

pub fn mutate_workspace_config_file(
    path: &Path,
    mutator: impl FnOnce(&mut WorkspaceConfig),
) -> Result<(), HarnessError> {
    let mut config = if path.exists() {
        load_workspace_config_file(path)?
    } else if path == global_config_path() {
        seed_global_config_from_legacy()?
    } else {
        let root = path.parent().ok_or_else(|| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: path.display().to_string(),
                reason: "configuration path has no parent directory".to_string(),
            })
        })?;
        seed_workspace_config_from_legacy(root)?
    };
    mutator(&mut config);
    write_workspace_config_file(path, &config)
}

fn bootstrap_global_config(path: &Path) -> Result<(), HarnessError> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: path.display().to_string(),
                reason: err.to_string(),
            })
        })?;
    }

    let bootstrap = WorkspaceConfig {
        version: 1,
        ..WorkspaceConfig::default()
    };
    write_workspace_config_file(path, &bootstrap)
}

pub fn default_workspace_config() -> WorkspaceConfig {
    let mut scaffold_policies = gestalt_policy::PolicyConfig::default();
    scaffold_policies.paths.allow_write = vec!["docs/".to_string(), ".gestalt/".to_string()];
    WorkspaceConfig {
        version: 1,
        workspace: Some(WorkspaceMetaConfig {
            initialized: true,
            format_version: 1,
        }),
        defaults: Some(DefaultsConfig {
            provider: None,
            model: None,
            mode: Some(ExecutionMode::Confirm),
            max_turns: Some(50),
            profile: Some("default".to_string()),
            ..DefaultsConfig::default()
        }),
        profiles: {
            let mut profiles = HashMap::new();
            profiles.insert(
                "default".to_string(),
                ProfileConfig {
                    provider: Some("openrouter".to_string()),
                    model: Some("openrouter/free".to_string()),
                    ..ProfileConfig::default()
                },
            );
            profiles
        },
        tools: Some(ToolsConfig {
            bash_timeout_secs: Some(60),
            max_output_tokens: Some(4000),
            sandbox_type: Some(SandboxType::None),
            ..ToolsConfig::default()
        }),
        context: Some(ContextConfig::default()),
        observe: Some(ObserveConfig::default()),
        policies: Some(PoliciesConfig::from_policy_config(&scaffold_policies)),
        ..WorkspaceConfig::default()
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CliOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub max_turns: Option<usize>,
    pub workspace: Option<PathBuf>,
    pub profile: Option<String>,
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EffectiveConfig {
    pub workspace_root: PathBuf,
    pub config_path: PathBuf,
    pub defaults: DefaultsConfig,
    pub tools: ToolsConfig,
    pub context: ContextConfig,
    pub observe: ObserveConfig,
    pub providers: HashMap<String, ProviderConfig>,
    pub profiles: HashMap<String, ProfileConfig>,
    pub prompt: PromptConfig,
    pub policies: PoliciesConfig,
    pub provider_override: Option<String>,
    pub model_override: Option<String>,
    pub tui: TuiConfig,
    pub extensions: ExtensionsConfig,
    pub skills: SkillsConfig,
    pub mcp: Option<McpConfig>,
}

impl EffectiveConfig {
    pub fn compute_fingerprint(&self) -> String {
        #[derive(Serialize)]
        struct StableConfig<'a> {
            defaults: &'a DefaultsConfig,
            tools: &'a ToolsConfig,
            context: &'a ContextConfig,
            observe: &'a ObserveConfig,
            providers: &'a HashMap<String, ProviderConfig>,
            profiles: &'a HashMap<String, ProfileConfig>,
            prompt: &'a PromptConfig,
            policies: &'a PoliciesConfig,
            provider_override: &'a Option<String>,
            model_override: &'a Option<String>,
            tui: &'a TuiConfig,
            extensions: &'a ExtensionsConfig,
            skills: &'a SkillsConfig,
            mcp: &'a Option<McpConfig>,
        }

        let stable = StableConfig {
            defaults: &self.defaults,
            tools: &self.tools,
            context: &self.context,
            observe: &self.observe,
            providers: &self.providers,
            profiles: &self.profiles,
            prompt: &self.prompt,
            policies: &self.policies,
            provider_override: &self.provider_override,
            model_override: &self.model_override,
            tui: &self.tui,
            extensions: &self.extensions,
            skills: &self.skills,
            mcp: &self.mcp,
        };

        use sha2::{Digest, Sha256};
        let serialized = serde_json::to_string(&stable).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

impl WorkspaceConfig {
    pub fn from_file(path: &Path) -> Result<Self, HarnessError> {
        let input = fs::read_to_string(path).map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: path.display().to_string(),
                reason: err.to_string(),
            })
        })?;
        let cfg: Self = if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            let v: serde_json::Value = serde_json::from_str(&input).map_err(|err| {
                HarnessError::Config(ConfigError::InvalidValue {
                    field: path.display().to_string(),
                    reason: err.to_string(),
                })
            })?;
            if v.get("version").is_none() {
                return Err(HarnessError::Config(ConfigError::InvalidValue {
                    field: "version".to_string(),
                    reason: "missing field `version`".to_string(),
                }));
            }
            serde_json::from_value(v).map_err(|err| {
                HarnessError::Config(ConfigError::InvalidValue {
                    field: path.display().to_string(),
                    reason: err.to_string(),
                })
            })?
        } else {
            if !is_json_output() {
                eprintln!(
                    "Warning: Loading legacy TOML configuration from '{}' is deprecated and will be removed in a future version. Please migrate to gestalt.json.",
                    path.display()
                );
            }
            toml::from_str(&input).map_err(|err| {
                HarnessError::Config(ConfigError::InvalidValue {
                    field: path.display().to_string(),
                    reason: err.to_string(),
                })
            })?
        };
        if cfg.version != 1 {
            return Err(HarnessError::Config(ConfigError::InvalidValue {
                field: "version".to_string(),
                reason: format!("version must be 1, found {}", cfg.version),
            }));
        }
        if !is_json_output() {
            if let Some(ref p) = cfg.policies {
                if p.bash.yolo_allow.is_some() {
                    eprintln!("Warning: 'yolo_allow' is deprecated, please use 'allow' instead.");
                }
                if p.bash.always_confirm.is_some() {
                    eprintln!(
                        "Warning: 'always_confirm' is deprecated, please use 'confirm' instead."
                    );
                }
                if p.bash.always_deny.is_some() {
                    eprintln!("Warning: 'always_deny' is deprecated, please use 'deny' instead.");
                }
            }
            if let Some(ref c) = cfg.context {
                if c.workspace_file.is_some() {
                    if c.workspace.is_some() {
                        eprintln!("Warning: Both 'context.workspace_file' and 'context.workspace' are specified. 'context.workspace' takes precedence.");
                    } else {
                        eprintln!("Warning: 'context.workspace_file' is deprecated. Please migrate to 'context.workspace.path'.");
                    }
                }
                if c.memory_file.is_some() {
                    if c.memory.is_some() {
                        eprintln!("Warning: Both 'context.memory_file' and 'context.memory' are specified. 'context.memory' takes precedence.");
                    } else {
                        eprintln!("Warning: 'context.memory_file' is deprecated. Please migrate to 'context.memory.path'.");
                    }
                }
            }
        }
        if let Some(ref c) = cfg.context {
            if let Some(ref w) = c.workspace {
                if w.enabled == Some(false) && w.required == Some(true) {
                    return Err(HarnessError::Config(ConfigError::InvalidValue {
                        field: "context.workspace".to_string(),
                        reason:
                            "cannot set workspace.enabled = false while workspace.required = true"
                                .to_string(),
                    }));
                }
            }
            if let Some(ref m) = c.memory {
                if m.enabled == Some(false) && m.required == Some(true) {
                    return Err(HarnessError::Config(ConfigError::InvalidValue {
                        field: "context.memory".to_string(),
                        reason: "cannot set memory.enabled = false while memory.required = true"
                            .to_string(),
                    }));
                }
            }
        }
        Ok(cfg)
    }

    pub fn merge(mut self, other: Self) -> Result<Self, HarnessError> {
        if let Some(other_defaults) = other.defaults {
            let mut self_defaults = self.defaults.unwrap_or_default();
            self_defaults.provider = other_defaults.provider.or(self_defaults.provider);
            self_defaults.model = other_defaults.model.or(self_defaults.model);
            self_defaults.mode = other_defaults.mode.or(self_defaults.mode);
            self_defaults.max_turns = other_defaults.max_turns.or(self_defaults.max_turns);
            self_defaults.profile = other_defaults.profile.or(self_defaults.profile);
            self_defaults.variant = other_defaults.variant.or(self_defaults.variant);
            self_defaults.max_output_tokens = other_defaults
                .max_output_tokens
                .or(self_defaults.max_output_tokens);
            self_defaults.temperature = other_defaults.temperature.or(self_defaults.temperature);
            self_defaults.top_p = other_defaults.top_p.or(self_defaults.top_p);
            self.defaults = Some(self_defaults);
        }

        if let Some(other_tools) = other.tools {
            let mut self_tools = self.tools.unwrap_or_default();
            self_tools.default_timeout_secs = other_tools
                .default_timeout_secs
                .or(self_tools.default_timeout_secs);
            self_tools.bash_timeout_secs = other_tools
                .bash_timeout_secs
                .or(self_tools.bash_timeout_secs);
            self_tools.max_output_bytes =
                other_tools.max_output_bytes.or(self_tools.max_output_bytes);
            self_tools.max_output_tokens = other_tools
                .max_output_tokens
                .or(self_tools.max_output_tokens);
            self_tools.max_parallel_calls = other_tools
                .max_parallel_calls
                .or(self_tools.max_parallel_calls);
            self_tools.sandbox_type = other_tools.sandbox_type.or(self_tools.sandbox_type);
            self_tools.ignore_patterns = other_tools.ignore_patterns.or(self_tools.ignore_patterns);
            self.tools = Some(self_tools);
        }

        if let Some(other_context) = other.context {
            let mut self_context = self.context.unwrap_or_default();
            self_context.context_window_override = other_context
                .context_window_override
                .or(self_context.context_window_override);
            self_context.max_context_window = other_context
                .max_context_window
                .or(self_context.max_context_window);
            self_context.reserved_output_tokens = other_context
                .reserved_output_tokens
                .or(self_context.reserved_output_tokens);
            self_context.safety_margin_tokens = other_context
                .safety_margin_tokens
                .or(self_context.safety_margin_tokens);
            self_context.workspace_file =
                other_context.workspace_file.or(self_context.workspace_file);
            self_context.memory_file = other_context.memory_file.or(self_context.memory_file);

            self_context.workspace = match (self_context.workspace.take(), other_context.workspace)
            {
                (Some(s_w), Some(o_w)) => Some(WorkspaceContextConfig {
                    enabled: o_w.enabled.or(s_w.enabled),
                    path: o_w.path.or(s_w.path),
                    required: o_w.required.or(s_w.required),
                    max_tokens: o_w.max_tokens.or(s_w.max_tokens),
                    max_bytes: o_w.max_bytes.or(s_w.max_bytes),
                    snapshot: o_w.snapshot.or(s_w.snapshot),
                }),
                (Some(s_w), None) => Some(s_w),
                (None, Some(o_w)) => Some(o_w),
                (None, None) => None,
            };

            self_context.memory = match (self_context.memory.take(), other_context.memory) {
                (Some(s_m), Some(o_m)) => Some(MemoryContextConfig {
                    enabled: o_m.enabled.or(s_m.enabled),
                    path: o_m.path.or(s_m.path),
                    required: o_m.required.or(s_m.required),
                    strategy: o_m.strategy.or(s_m.strategy),
                    max_tokens: o_m.max_tokens.or(s_m.max_tokens),
                    max_bytes: o_m.max_bytes.or(s_m.max_bytes),
                    pinned_section: o_m.pinned_section.or(s_m.pinned_section),
                    snapshot: o_m.snapshot.or(s_m.snapshot),
                    write_mode: o_m.write_mode.or(s_m.write_mode),
                }),
                (Some(s_m), None) => Some(s_m),
                (None, Some(o_m)) => Some(o_m),
                (None, None) => None,
            };

            self_context.management = other_context.management.or(self_context.management);
            self.context = Some(self_context);
        }

        if let Some(other_observe) = other.observe {
            let mut self_observe = self.observe.unwrap_or_default();
            self_observe.run_log_dir = other_observe.run_log_dir.or(self_observe.run_log_dir);
            self_observe.log_format = other_observe.log_format.or(self_observe.log_format);
            self.observe = Some(self_observe);
        }

        if let Some(other_prompt) = other.prompt {
            let mut self_prompt = self.prompt.unwrap_or_default();
            self_prompt.r#override = other_prompt.r#override.or(self_prompt.r#override);
            self_prompt.override_file = other_prompt.override_file.or(self_prompt.override_file);
            self_prompt.assembly_strategy = other_prompt
                .assembly_strategy
                .or(self_prompt.assembly_strategy);
            self.prompt = Some(self_prompt);
        }

        if let Some(other_policies) = other.policies {
            let mut self_policies = self.policies.unwrap_or_default();

            // validate allow list subset constraint:
            validate_subset(
                self_policies.paths.allow_read.as_ref(),
                other_policies.paths.allow_read.as_ref(),
                "policies.paths.allow_read",
            )?;
            validate_subset(
                self_policies.paths.allow_write.as_ref(),
                other_policies.paths.allow_write.as_ref(),
                "policies.paths.allow_write",
            )?;
            validate_subset(
                self_policies.bash.allow.as_ref(),
                other_policies.bash.allow.as_ref(),
                "policies.bash.allow",
            )?;
            validate_subset(
                self_policies.network.allow_domains.as_ref(),
                other_policies.network.allow_domains.as_ref(),
                "policies.network.allow_domains",
            )?;

            // Replace allow lists with non-empty overrides:
            if other_policies.paths.allow_read.is_some() {
                self_policies.paths.allow_read = other_policies.paths.allow_read;
            }
            if other_policies.paths.allow_write.is_some() {
                self_policies.paths.allow_write = other_policies.paths.allow_write;
            }
            if other_policies.bash.allow.is_some() {
                self_policies.bash.allow = other_policies.bash.allow;
            }
            if other_policies.network.allow_domains.is_some() {
                self_policies.network.allow_domains = other_policies.network.allow_domains;
            }

            // Union merge for deny lists:
            self_policies.paths.deny_write = merge_unions(
                self_policies.paths.deny_write,
                other_policies.paths.deny_write,
            );
            self_policies.paths.deny_read = merge_unions(
                self_policies.paths.deny_read,
                other_policies.paths.deny_read,
            );
            self_policies.bash.deny =
                merge_unions(self_policies.bash.deny, other_policies.bash.deny);
            self_policies.bash.always_deny = merge_unions(
                self_policies.bash.always_deny,
                other_policies.bash.always_deny,
            );
            self_policies.network.deny_domains = merge_unions(
                self_policies.network.deny_domains,
                other_policies.network.deny_domains,
            );

            self_policies.bash.default = other_policies.bash.default.or(self_policies.bash.default);
            self_policies.bash.confirm = other_policies.bash.confirm.or(self_policies.bash.confirm);
            self_policies.bash.yolo_allow = other_policies
                .bash
                .yolo_allow
                .or(self_policies.bash.yolo_allow);
            self_policies.bash.always_confirm = other_policies
                .bash
                .always_confirm
                .or(self_policies.bash.always_confirm);

            self_policies.network.default = other_policies
                .network
                .default
                .or(self_policies.network.default);

            self.policies = Some(self_policies);
        }

        if let Some(other_tui) = other.tui {
            let mut self_tui = self.tui.unwrap_or_default();
            if let Some(other_diag) = other_tui.diagnostics {
                let mut self_diag = self_tui.diagnostics.unwrap_or_default();
                self_diag.max_log_lines = other_diag.max_log_lines.or(self_diag.max_log_lines);
                self_tui.diagnostics = Some(self_diag);
            }
            self.tui = Some(self_tui);
        }

        if let Some(other_extensions) = other.extensions {
            let mut self_extensions = self.extensions.unwrap_or_default();
            self_extensions
                .explicit_loads
                .extend(other_extensions.explicit_loads);
            self_extensions.disabled.extend(other_extensions.disabled);
            self_extensions.trusted.extend(other_extensions.trusted);
            self_extensions.allow_untrusted =
                other_extensions.allow_untrusted || self_extensions.allow_untrusted;

            // Merge timeouts
            if other_extensions.timeouts.initialize_ms.is_some() {
                self_extensions.timeouts.initialize_ms = other_extensions.timeouts.initialize_ms;
            }
            if other_extensions.timeouts.hook_ms.is_some() {
                self_extensions.timeouts.hook_ms = other_extensions.timeouts.hook_ms;
            }
            if other_extensions.timeouts.context_ms.is_some() {
                self_extensions.timeouts.context_ms = other_extensions.timeouts.context_ms;
            }
            if other_extensions.timeouts.tool_ms.is_some() {
                self_extensions.timeouts.tool_ms = other_extensions.timeouts.tool_ms;
            }
            if other_extensions.timeouts.shutdown_ms.is_some() {
                self_extensions.timeouts.shutdown_ms = other_extensions.timeouts.shutdown_ms;
            }

            // Merge limits
            if other_extensions.limits.max_message_bytes.is_some() {
                self_extensions.limits.max_message_bytes =
                    other_extensions.limits.max_message_bytes;
            }
            if other_extensions.limits.max_pending_requests.is_some() {
                self_extensions.limits.max_pending_requests =
                    other_extensions.limits.max_pending_requests;
            }
            if other_extensions.limits.max_protocol_errors.is_some() {
                self_extensions.limits.max_protocol_errors =
                    other_extensions.limits.max_protocol_errors;
            }

            self_extensions.instances.extend(other_extensions.instances);

            self.extensions = Some(self_extensions);
        }

        if let Some(other_skills) = other.skills {
            let mut self_skills = self.skills.unwrap_or_default();
            self_skills
                .explicit_paths
                .extend(other_skills.explicit_paths);
            self_skills.active.extend(other_skills.active);
            self_skills.trusted.extend(other_skills.trusted);
            self.skills = Some(self_skills);
        }

        if let Some(other_mcp) = other.mcp {
            let mut self_mcp = self.mcp.unwrap_or_default();
            for (k, v) in other_mcp.servers {
                self_mcp.servers.insert(k, v);
            }
            if other_mcp.discovery_threshold.is_some() {
                self_mcp.discovery_threshold = other_mcp.discovery_threshold;
            }
            self.mcp = Some(self_mcp);
        }

        self.providers.extend(other.providers);
        self.profiles.extend(other.profiles);
        Ok(self)
    }
}

fn merge_unions(a: Option<Vec<String>>, b: Option<Vec<String>>) -> Option<Vec<String>> {
    match (a, b) {
        (Some(mut va), Some(vb)) => {
            for item in vb {
                if !va.contains(&item) {
                    va.push(item);
                }
            }
            Some(va)
        }
        (Some(va), None) => Some(va),
        (None, Some(vb)) => Some(vb),
        (None, None) => None,
    }
}

fn validate_subset(
    global: Option<&Vec<String>>,
    workspace: Option<&Vec<String>>,
    field_name: &str,
) -> Result<(), HarnessError> {
    if let (Some(g), Some(w)) = (global, workspace) {
        for item in w {
            if !g.contains(item) {
                return Err(HarnessError::Config(ConfigError::InvalidValue {
                    field: field_name.to_string(),
                    reason: format!(
                        "workspace policy tries to widen authority: value '{}' is not allowed by global configuration",
                        item
                    ),
                }));
            }
        }
    }
    Ok(())
}

pub fn load_effective_config(overrides: &CliOverrides) -> Result<EffectiveConfig, HarnessError> {
    let workspace_root = overrides
        .workspace
        .clone()
        .unwrap_or(std::env::current_dir().map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: "workspace".to_string(),
                reason: err.to_string(),
            })
        })?);
    let global_path = global_config_path();
    let legacy_global_path = legacy_global_config_path();
    let workspace_path = workspace_config_path(&workspace_root);
    let legacy_workspace_path = legacy_workspace_config_path(&workspace_root);
    let legacy_policies_path = legacy_workspace_policies_path(&workspace_root);

    if !global_path.exists() && !legacy_global_path.exists() {
        bootstrap_global_config(&global_path)?;
    }

    let mut config = WorkspaceConfig::default();
    let config_path = if workspace_path.exists() {
        workspace_path.clone()
    } else if legacy_workspace_path.exists() {
        legacy_workspace_path.clone()
    } else if legacy_policies_path.exists() {
        legacy_policies_path.clone()
    } else if global_path.exists() {
        global_path.clone()
    } else {
        legacy_global_path.clone()
    };

    if global_path.exists() {
        config = config.merge(WorkspaceConfig::from_file(&global_path)?)?;
    } else if legacy_global_path.exists() {
        config = config.merge(WorkspaceConfig::from_file(&legacy_global_path)?)?;
    }
    if workspace_path.exists() {
        config = config.merge(WorkspaceConfig::from_file(&workspace_path)?)?;
    } else {
        if legacy_workspace_path.exists() {
            config = config.merge(WorkspaceConfig::from_file(&legacy_workspace_path)?)?;
        }
        if legacy_policies_path.exists() {
            config = config.merge(load_legacy_policies_file(&legacy_policies_path)?)?;
        }
    }

    let mut defaults = config.defaults.unwrap_or_default();

    if let Ok(profile) = std::env::var("GESTALT_PROFILE") {
        defaults.profile = Some(profile);
    }
    if let Ok(provider) = std::env::var("GESTALT_PROVIDER") {
        defaults.provider = Some(provider);
    }
    if let Ok(model) = std::env::var("GESTALT_MODEL") {
        defaults.model = Some(model);
    }
    if let Ok(mode) = std::env::var("GESTALT_MODE") {
        if let Ok(m) = mode_from_str(&mode) {
            defaults.mode = Some(m);
        }
    }
    if let Ok(max_turns) = std::env::var("GESTALT_MAX_TURNS") {
        if let Ok(max_turns) = max_turns.parse::<usize>() {
            defaults.max_turns = Some(max_turns);
        }
    }

    if let Some(profile) = &overrides.profile {
        defaults.profile = Some(profile.clone());
    }
    if let Some(provider) = &overrides.provider {
        defaults.provider = Some(provider.clone());
    }
    if let Some(model) = &overrides.model {
        defaults.model = Some(model.clone());
    }
    if let Some(mode) = &overrides.mode {
        if let Ok(m) = mode_from_str(mode) {
            defaults.mode = Some(m);
        }
    }
    if let Some(max_turns) = overrides.max_turns {
        defaults.max_turns = Some(max_turns);
    }
    config.defaults = Some(defaults);

    let tools = {
        let mut t = config.tools.unwrap_or_default();
        let d = ToolsConfig::default();
        t.bash_timeout_secs = t.bash_timeout_secs.or(d.bash_timeout_secs);
        t.max_output_tokens = t.max_output_tokens.or(d.max_output_tokens);
        t.sandbox_type = t.sandbox_type.or(d.sandbox_type);
        t
    };

    let context = {
        let mut c = config.context.unwrap_or_default();
        let d = ContextConfig::default();
        if c.max_context_window.is_some() && c.context_window_override.is_none() {
            c.context_window_override = c.max_context_window;
        }
        if c.context_window_override.is_some() && c.max_context_window.is_none() {
            c.max_context_window = c.context_window_override;
        }
        c.context_window_override = c.context_window_override.or(d.context_window_override);
        c.max_context_window = c
            .max_context_window
            .or(d.max_context_window)
            .or(c.context_window_override);
        c.reserved_output_tokens = c.reserved_output_tokens.or(d.reserved_output_tokens);
        c.safety_margin_tokens = c.safety_margin_tokens.or(d.safety_margin_tokens);
        c.workspace_file = c.workspace_file.or(d.workspace_file);
        c.memory_file = c.memory_file.or(d.memory_file);

        // Resolve structured workspace config
        let mut w = c.workspace.unwrap_or_default();
        let resolved_workspace_path = if let Some(ref sp) = w.path {
            sp.clone()
        } else if let Some(ref lp) = c.workspace_file {
            PathBuf::from(lp)
        } else {
            PathBuf::from(".gestalt/workspace.md")
        };
        w.enabled = Some(w.enabled.unwrap_or(true));
        w.path = Some(resolved_workspace_path);
        w.required = Some(w.required.unwrap_or(false));
        w.max_tokens = Some(w.max_tokens.unwrap_or(12000));
        w.max_bytes = Some(w.max_bytes.unwrap_or(131_072));
        w.snapshot = Some(w.snapshot.unwrap_or(ContextSnapshotMode::Session));
        c.workspace = Some(w);

        // Resolve structured memory config
        let mut m = c.memory.unwrap_or_default();
        let resolved_memory_path = if let Some(ref sp) = m.path {
            sp.clone()
        } else if let Some(ref lp) = c.memory_file {
            PathBuf::from(lp)
        } else {
            PathBuf::from(".gestalt/memory.md")
        };
        m.enabled = Some(m.enabled.unwrap_or(true));
        m.path = Some(resolved_memory_path);
        m.required = Some(m.required.unwrap_or(false));
        m.strategy = Some(m.strategy.unwrap_or(MemorySelectionStrategy::Budgeted));
        m.max_tokens = Some(m.max_tokens.unwrap_or(8000));
        m.max_bytes = Some(m.max_bytes.unwrap_or(524_288));
        m.pinned_section = Some(m.pinned_section.unwrap_or_else(|| "Facts".to_string()));
        m.snapshot = Some(m.snapshot.unwrap_or(ContextSnapshotMode::Session));
        m.write_mode = Some(m.write_mode.unwrap_or(MemoryWriteMode::Proposal));
        c.memory = Some(m);

        c
    };

    let observe = {
        let mut o = config.observe.unwrap_or_default();
        let d = ObserveConfig::default();
        o.run_log_dir = o.run_log_dir.or(d.run_log_dir);
        o.log_format = o.log_format.or(d.log_format);
        o
    };

    let prompt = config.prompt.unwrap_or_default();
    let policies = config.policies.unwrap_or_default();

    let provider_override = overrides
        .provider
        .clone()
        .or_else(|| std::env::var("GESTALT_PROVIDER").ok());
    let model_override = overrides
        .model
        .clone()
        .or_else(|| std::env::var("GESTALT_MODEL").ok());

    let mut tui = config.tui.unwrap_or_default();
    let mut diagnostics = tui.diagnostics.unwrap_or_default();

    if let Ok(env_max) = std::env::var("GESTALT_TUI_MAX_LOG_LINES") {
        if let Ok(val) = env_max.parse::<usize>() {
            diagnostics.max_log_lines = Some(val);
        } else {
            return Err(HarnessError::Config(ConfigError::InvalidValue {
                field: "GESTALT_TUI_MAX_LOG_LINES".to_string(),
                reason: format!("Invalid integer: {env_max}"),
            }));
        }
    }

    let max_lines = diagnostics.max_log_lines.unwrap_or(1000);
    if !(100..=50_000).contains(&max_lines) {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: "tui.diagnostics.max_log_lines".to_string(),
            reason: format!("Value {max_lines} must be between 100 and 50,000"),
        }));
    }
    diagnostics.max_log_lines = Some(max_lines);
    tui.diagnostics = Some(diagnostics);

    let extensions = config.extensions.unwrap_or_default();
    let mut skills = config.skills.unwrap_or_default();
    for skill in &overrides.skills {
        if let Some(name) = skill.strip_prefix('!') {
            skills.active.retain(|active| active != name);
        } else if !skills.active.iter().any(|active| active == skill) {
            skills.active.push(skill.clone());
        }
    }
    let mcp = config.mcp;

    Ok(EffectiveConfig {
        workspace_root,
        config_path,
        defaults: config.defaults.unwrap_or_default(),
        tools,
        context,
        observe,
        providers: config.providers,
        profiles: config.profiles,
        prompt,
        policies,
        provider_override,
        model_override,
        tui,
        extensions,
        skills,
        mcp,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedProvider {
    pub profile_name: Option<String>,
    pub provider_name: String,
    pub kind: String,
    pub model: String,
    pub auth_ref: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub models_endpoint: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub resolved_options: ModelOptionsConfig,
    pub capabilities: Option<serde_json::Value>,
}

impl ResolvedProvider {
    pub fn provider_json(&self) -> Value {
        json!({
            "id": self.provider_name.clone(),
            "display_name": Some(self.provider_name.clone()),
            "kind": self.kind.clone(),
            "base_url": self.base_url,
            "default_model": Some(self.model.clone()),
            "api_key_env": self.api_key_env,
            "auth_ref": self.auth_ref,
            "models_endpoint": self.models_endpoint,
            "headers": self.headers,
            "capabilities": self.capabilities,
        })
    }
}

impl EffectiveConfig {
    #[allow(clippy::cast_possible_truncation)]
    pub fn resolve_provider(&self) -> Result<ResolvedProvider, HarnessError> {
        let active_profile = self.defaults.profile.clone();
        let active_provider = self.defaults.provider.clone();

        let (profile_name, mut provider_name, mut model_override) =
            if let Some(ref p) = active_profile {
                if let Some(prof_cfg) = self.profiles.get(p) {
                    let prov = prof_cfg.provider.clone().unwrap_or_else(|| p.clone());
                    let model = prof_cfg.model.clone();
                    (Some(p.clone()), prov, model)
                } else {
                    match p.as_str() {
                        "default" => (
                            Some(p.clone()),
                            "openrouter".to_string(),
                            Some("openrouter/free".to_string()),
                        ),
                        "openrouter" => (
                            Some(p.clone()),
                            "openrouter".to_string(),
                            Some("openrouter/free".to_string()),
                        ),
                        "anthropic" => (
                            Some(p.clone()),
                            "anthropic".to_string(),
                            Some("claude-3-5-sonnet-20241022".to_string()),
                        ),
                        "openai" => (
                            Some(p.clone()),
                            "openai".to_string(),
                            Some("gpt-4o-mini".to_string()),
                        ),
                        "ollama" => (
                            Some(p.clone()),
                            "ollama".to_string(),
                            Some("llama3".to_string()),
                        ),
                        "groq" => (
                            Some(p.clone()),
                            "groq".to_string(),
                            Some("llama3-8b-8192".to_string()),
                        ),
                        "together" => (
                            Some(p.clone()),
                            "together".to_string(),
                            Some("mistralai/Mixtral-8x7B-Instruct-v0.1".to_string()),
                        ),
                        _ => {
                            return Err(HarnessError::Config(ConfigError::InvalidValue {
                                field: "defaults.profile".to_string(),
                                reason: format!(
                                    "profile '{p}' not found in configuration or built-ins"
                                ),
                            }));
                        }
                    }
                }
            } else if let Some(prov) = active_provider {
                let model = self.defaults.model.clone();
                (None, prov, model)
            } else {
                let p = "default".to_string();
                (
                    Some(p),
                    "openrouter".to_string(),
                    Some("openrouter/free".to_string()),
                )
            };

        // Apply explicit CLI / Env overrides (which beat profiles and defaults)
        if let Some(ref prov_ovr) = self.provider_override {
            provider_name.clone_from(prov_ovr);
        }
        if let Some(ref model_ovr) = self.model_override {
            model_override = Some(model_ovr.clone());
        }

        let (
            kind,
            base_url,
            default_model,
            api_key_env,
            auth_ref,
            models_endpoint,
            headers,
            capabilities,
        ) = if let Some(prov_cfg) = self.providers.get(&provider_name) {
            let kind = prov_cfg
                .kind
                .clone()
                .or_else(|| {
                    if provider_name == "anthropic" {
                        Some(ProviderKind::Anthropic)
                    } else if provider_name == "openai" {
                        Some(ProviderKind::Openai)
                    } else if provider_name == "openai-compatible" {
                        Some(ProviderKind::OpenaiCompatible)
                    } else if gestalt_models::registry::registered().contains(&provider_name) {
                        if provider_name == "anthropic" {
                            Some(ProviderKind::Anthropic)
                        } else if provider_name == "openai" {
                            Some(ProviderKind::Openai)
                        } else {
                            Some(ProviderKind::Custom(provider_name.clone()))
                        }
                    } else {
                        Some(ProviderKind::OpenaiCompatible)
                    }
                })
                .unwrap_or(ProviderKind::OpenaiCompatible);
            let base = prov_cfg.base_url.clone();
            let model = prov_cfg.default_model.clone();
            let env = prov_cfg.api_key_env.clone();
            let auth = prov_cfg.auth_ref.clone();
            let endpoint = prov_cfg.models_endpoint.clone();
            let hdrs = prov_cfg.headers.clone();
            let caps = prov_cfg.capabilities.clone();
            (kind, base, model, env, auth, endpoint, hdrs, caps)
        } else if let Some(builtin) = crate::provider_catalog::get_builtin_provider(&provider_name)
        {
            (
                builtin
                    .kind
                    .clone()
                    .unwrap_or(ProviderKind::OpenaiCompatible),
                builtin.base_url,
                builtin.default_model,
                builtin.api_key_env,
                builtin.auth_ref,
                builtin.models_endpoint,
                builtin.headers,
                None,
            )
        } else if gestalt_models::registry::registered().contains(&provider_name) {
            let kind = if provider_name == "anthropic" {
                ProviderKind::Anthropic
            } else if provider_name == "openai" {
                ProviderKind::Openai
            } else {
                ProviderKind::Custom(provider_name.clone())
            };
            (kind, None, None, None, None, None, None, None)
        } else {
            return Err(HarnessError::Provider(ProviderError::UnknownProvider(
                provider_name.clone(),
            )));
        };

        let kind_str = match kind {
            ProviderKind::Openai => "openai".to_string(),
            ProviderKind::Anthropic => "anthropic".to_string(),
            ProviderKind::OpenaiCompatible => "openai-compatible".to_string(),
            ProviderKind::Custom(s) => s,
        };

        let model = model_override.or(default_model).unwrap_or_else(|| {
            if kind_str == "anthropic" {
                "claude-3-5-sonnet-20241022".to_string()
            } else if kind_str == "openai" {
                "gpt-4o-mini".to_string()
            } else {
                "openrouter/free".to_string()
            }
        });

        let mut active_variant = None;
        if let Some(ref p) = active_profile {
            if let Some(prof_cfg) = self.profiles.get(p) {
                if let Some(ref v) = prof_cfg.variant {
                    active_variant = Some(v.clone());
                }
            }
        }
        if let Some(ref v) = self.defaults.variant {
            active_variant = Some(v.clone());
        }
        if let Ok(v) = std::env::var("GESTALT_VARIANT") {
            active_variant = Some(v);
        }

        let mut resolved_options = ModelOptionsConfig::default();
        if let Some(prov_cfg) = self.providers.get(&provider_name) {
            if let Some(model_def) = prov_cfg.models.get(&model) {
                if let Some(ref opts) = model_def.options {
                    resolved_options = opts.clone();
                }
                if let Some(ref variant_name) = active_variant {
                    let mut visited = Vec::new();
                    let variant_opts =
                        resolve_variant(&model_def.variants, variant_name, &mut visited)?;
                    resolved_options = merge_options(resolved_options, variant_opts);
                }
            } else if active_variant.is_some() {
                return Err(HarnessError::Config(ConfigError::InvalidValue {
                    field: "variant".to_string(),
                    reason: format!("variant specified but model '{}' has no definitions", model),
                }));
            }
        } else if active_variant.is_some() {
            return Err(HarnessError::Config(ConfigError::InvalidValue {
                field: "variant".to_string(),
                reason: format!(
                    "variant specified but provider '{}' has no definitions",
                    provider_name
                ),
            }));
        }

        // Apply profile & defaults overrides on top:
        if let Some(ref p) = active_profile {
            if let Some(prof_cfg) = self.profiles.get(p) {
                if let Some(t) = prof_cfg.max_output_tokens {
                    resolved_options.max_output_tokens = Some(t as u32);
                }
                if let Some(temp) = prof_cfg.temperature {
                    resolved_options.temperature = Some(temp as f32);
                }
                if let Some(tp) = prof_cfg.top_p {
                    resolved_options.top_p = Some(tp as f32);
                }
            }
        }
        if let Some(t) = self.defaults.max_output_tokens {
            resolved_options.max_output_tokens = Some(t as u32);
        }
        if let Some(temp) = self.defaults.temperature {
            resolved_options.temperature = Some(temp as f32);
        }
        if let Some(tp) = self.defaults.top_p {
            resolved_options.top_p = Some(tp as f32);
        }

        Ok(ResolvedProvider {
            profile_name,
            provider_name,
            kind: kind_str,
            model,
            auth_ref,
            base_url,
            api_key_env,
            models_endpoint,
            headers,
            resolved_options,
            capabilities,
        })
    }

    pub fn selected_provider(&self) -> Result<String, HarnessError> {
        let resolved = self.resolve_provider()?;
        Ok(resolved.provider_name)
    }

    pub fn selected_model(&self) -> Option<String> {
        if let Ok(resolved) = self.resolve_provider() {
            Some(resolved.model)
        } else {
            self.defaults.model.clone()
        }
    }

    pub fn selected_mode(&self) -> Result<ExecutionMode, HarnessError> {
        Ok(self.defaults.mode.unwrap_or(ExecutionMode::Confirm))
    }

    pub fn max_turns(&self) -> usize {
        self.defaults.max_turns.unwrap_or(50)
    }

    pub fn run_log_dir(&self) -> PathBuf {
        let relative = self
            .observe
            .run_log_dir
            .clone()
            .unwrap_or_else(|| ".gestalt/runs".to_string());
        self.workspace_root.join(relative)
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_path.clone()
    }

    pub fn workspace_markdown_path(&self) -> PathBuf {
        self.workspace_file("workspace.md")
    }

    pub fn memory_markdown_path(&self) -> PathBuf {
        self.workspace_file("memory.md")
    }

    pub fn provider_json(&self, provider: &str) -> Value {
        let configured = self.providers.get(provider).cloned().unwrap_or_default();
        let mut base = crate::provider_catalog::get_builtin_provider(provider).unwrap_or_default();

        if let Some(id) = configured.id {
            base.id = Some(id);
        }
        if let Some(display) = configured.display_name {
            base.display_name = Some(display);
        }
        if let Some(protocol) = configured.protocol {
            base.protocol = Some(protocol);
        }
        if let Some(base_url) = configured.base_url {
            base.base_url = Some(base_url);
        }
        if let Some(def_model) = configured.default_model {
            base.default_model = Some(def_model);
        }
        if let Some(api_key_env) = configured.api_key_env {
            base.api_key_env = Some(api_key_env);
        }
        if let Some(auth_ref) = configured.auth_ref {
            base.auth_ref = Some(auth_ref);
        }
        if let Some(kind) = configured.kind {
            base.kind = Some(kind);
        }
        if let Some(models_endpoint) = configured.models_endpoint {
            base.models_endpoint = Some(models_endpoint);
        }
        if let Some(headers) = configured.headers {
            base.headers = Some(headers);
        }

        json!({
            "id": base.id.unwrap_or_else(|| provider.to_string()),
            "display_name": base.display_name,
            "protocol": base.protocol,
            "base_url": base.base_url,
            "default_model": base.default_model.or_else(|| self.selected_model()),
            "api_key_env": base.api_key_env,
            "auth_ref": base.auth_ref,
            "kind": base.kind,
            "models_endpoint": base.models_endpoint,
            "headers": base.headers,
        })
    }

    pub fn workspace_file(&self, name: &str) -> PathBuf {
        let relative = match name {
            "workspace.md" => self
                .context
                .workspace
                .as_ref()
                .and_then(|w| w.path.clone())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".gestalt/workspace.md".to_string()),
            "memory.md" => self
                .context
                .memory
                .as_ref()
                .and_then(|m| m.path.clone())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".gestalt/memory.md".to_string()),
            other => format!(".gestalt/{other}"),
        };
        self.workspace_root.join(relative)
    }
}

pub fn mode_from_str(value: &str) -> Result<ExecutionMode, HarnessError> {
    match value {
        "confirm" => Ok(ExecutionMode::Confirm),
        "yolo" => Ok(ExecutionMode::Yolo),
        "human" => Ok(ExecutionMode::Human),
        "dry_run" | "dry-run" => Ok(ExecutionMode::DryRun),
        "replay" => Ok(ExecutionMode::Replay),
        _ => Err(HarnessError::Config(ConfigError::InvalidValue {
            field: "defaults.mode".to_string(),
            reason: format!("unsupported mode: {value}"),
        })),
    }
}

pub fn validate_workspace_config(
    overrides: &CliOverrides,
) -> Result<EffectiveConfig, HarnessError> {
    load_effective_config(overrides)
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSourceInfo {
    pub value: Value,
    pub source: String,
}

pub fn explain_config(
    overrides: &CliOverrides,
) -> Result<HashMap<String, ConfigSourceInfo>, HarnessError> {
    let workspace_root = overrides
        .workspace
        .clone()
        .unwrap_or(std::env::current_dir().map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: "workspace".to_string(),
                reason: err.to_string(),
            })
        })?);
    let global_path = global_config_path();
    let legacy_global_path = legacy_global_config_path();
    let workspace_path = workspace_config_path(&workspace_root);
    let legacy_workspace_path = legacy_workspace_config_path(&workspace_root);
    let legacy_policies_path = legacy_workspace_policies_path(&workspace_root);

    let mut global_cfg = None;
    if global_path.exists() {
        global_cfg = Some(WorkspaceConfig::from_file(&global_path)?);
    } else if legacy_global_path.exists() {
        global_cfg = Some(WorkspaceConfig::from_file(&legacy_global_path)?);
    }
    let mut ws_cfg = None;
    if workspace_path.exists() {
        ws_cfg = Some(WorkspaceConfig::from_file(&workspace_path)?);
    } else {
        if legacy_workspace_path.exists() {
            ws_cfg = Some(WorkspaceConfig::from_file(&legacy_workspace_path)?);
        }
        if legacy_policies_path.exists() {
            let legacy = load_legacy_policies_file(&legacy_policies_path)?;
            ws_cfg = Some(match ws_cfg.take() {
                Some(existing) => existing.merge(legacy)?,
                None => legacy,
            });
        }
    }

    let mut map = HashMap::new();

    // Helper macro to resolve a key with precedence: CLI > Env Var > Workspace > Global > Default
    macro_rules! resolve {
        ($key:expr, $cli_val:expr, $env_name:expr, $ws_field:expr, $global_field:expr, $default_val:expr) => {
            let mut active_source = "Default".to_string();
            let mut active_value = json!($default_val);

            if let Some(ref g) = global_cfg {
                if let Some(val) = $global_field(g) {
                    active_source = "Global Config File".to_string();
                    active_value = json!(val);
                }
            }

            if let Some(ref w) = ws_cfg {
                if let Some(val) = $ws_field(w) {
                    active_source = "Workspace Config File".to_string();
                    active_value = json!(val);
                }
            }

            if let Some(ref env_name) = $env_name {
                if let Ok(val) = std::env::var(env_name) {
                    active_source = format!("Env Var ({})", env_name);
                    active_value = json!(val);
                }
            }

            if let Some(ref val) = $cli_val {
                active_source = "CLI Override".to_string();
                active_value = json!(val);
            }

            map.insert(
                $key.to_string(),
                ConfigSourceInfo {
                    value: active_value,
                    source: active_source,
                },
            );
        };
    }

    resolve!(
        "defaults.profile",
        overrides.profile,
        Some("GESTALT_PROFILE"),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.profile.clone())),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.profile.clone())),
        Value::Null
    );

    resolve!(
        "defaults.provider",
        overrides.provider,
        Some("GESTALT_PROVIDER"),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.provider.clone())),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.provider.clone())),
        "anthropic"
    );

    resolve!(
        "defaults.model",
        overrides.model,
        Some("GESTALT_MODEL"),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.model.clone())),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.model.clone())),
        Value::Null
    );

    resolve!(
        "defaults.mode",
        overrides.mode,
        Some("GESTALT_MODE"),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.mode)),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.mode)),
        "confirm"
    );

    resolve!(
        "defaults.max_turns",
        overrides.max_turns,
        Some("GESTALT_MAX_TURNS"),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.max_turns)),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.max_turns)),
        50
    );

    resolve!(
        "tools.bash_timeout_secs",
        None::<u64>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.bash_timeout_secs)),
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.bash_timeout_secs)),
        60
    );

    resolve!(
        "tools.max_output_tokens",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.max_output_tokens)),
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.max_output_tokens)),
        4000
    );

    resolve!(
        "tools.sandbox_type",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.sandbox_type)),
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.sandbox_type)),
        "none"
    );

    resolve!(
        "tools.ignore_patterns",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.ignore_patterns.clone())),
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.ignore_patterns.clone())),
        Value::Null
    );

    resolve!(
        "context.max_context_window",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.max_context_window)),
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.max_context_window)),
        120_000
    );

    resolve!(
        "context.reserved_output_tokens",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.reserved_output_tokens)),
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.reserved_output_tokens)),
        8000
    );

    resolve!(
        "context.workspace_file",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.workspace_file.clone())),
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.workspace_file.clone())),
        Value::Null
    );

    resolve!(
        "context.memory_file",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.memory_file.clone())),
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.memory_file.clone())),
        Value::Null
    );

    resolve!(
        "context.workspace.enabled",
        None::<bool>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.enabled)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.enabled)),
        true
    );

    {
        let mut active_source = "Default".to_string();
        let mut active_value = json!(".gestalt/workspace.md");

        if let Some(ref g) = global_cfg {
            if let Some(ref c) = g.context {
                if let Some(ref w) = c.workspace {
                    if let Some(ref path) = w.path {
                        active_source = "Global Config File".to_string();
                        active_value = json!(path);
                    }
                }
                if active_source == "Default" {
                    if let Some(ref path) = c.workspace_file {
                        active_source =
                            "Global Config File (via deprecated context.workspace_file)"
                                .to_string();
                        active_value = json!(path);
                    }
                }
            }
        }

        if let Some(ref w_cfg) = ws_cfg {
            if let Some(ref c) = w_cfg.context {
                let mut source_set = false;
                if let Some(ref w) = c.workspace {
                    if let Some(ref path) = w.path {
                        active_source = "Workspace Config File".to_string();
                        active_value = json!(path);
                        source_set = true;
                    }
                }
                if !source_set {
                    if let Some(ref path) = c.workspace_file {
                        active_source =
                            "Workspace Config File (via deprecated context.workspace_file)"
                                .to_string();
                        active_value = json!(path);
                    }
                }
            }
        }

        map.insert(
            "context.workspace.path".to_string(),
            ConfigSourceInfo {
                value: active_value,
                source: active_source,
            },
        );
    }

    resolve!(
        "context.workspace.required",
        None::<bool>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.required)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.required)),
        false
    );

    resolve!(
        "context.workspace.max_tokens",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.max_tokens)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.max_tokens)),
        12000
    );

    resolve!(
        "context.workspace.max_bytes",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.max_bytes)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.max_bytes)),
        131_072
    );

    resolve!(
        "context.workspace.snapshot",
        None::<ContextSnapshotMode>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.snapshot)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.workspace.as_ref())
            .and_then(|w| w.snapshot)),
        ContextSnapshotMode::Session
    );

    resolve!(
        "context.memory.enabled",
        None::<bool>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.enabled)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.enabled)),
        true
    );

    {
        let mut active_source = "Default".to_string();
        let mut active_value = json!(".gestalt/memory.md");

        if let Some(ref g) = global_cfg {
            if let Some(ref c) = g.context {
                if let Some(ref m) = c.memory {
                    if let Some(ref path) = m.path {
                        active_source = "Global Config File".to_string();
                        active_value = json!(path);
                    }
                }
                if active_source == "Default" {
                    if let Some(ref path) = c.memory_file {
                        active_source =
                            "Global Config File (via deprecated context.memory_file)".to_string();
                        active_value = json!(path);
                    }
                }
            }
        }

        if let Some(ref w_cfg) = ws_cfg {
            if let Some(ref c) = w_cfg.context {
                let mut source_set = false;
                if let Some(ref m) = c.memory {
                    if let Some(ref path) = m.path {
                        active_source = "Workspace Config File".to_string();
                        active_value = json!(path);
                        source_set = true;
                    }
                }
                if !source_set {
                    if let Some(ref path) = c.memory_file {
                        active_source =
                            "Workspace Config File (via deprecated context.memory_file)"
                                .to_string();
                        active_value = json!(path);
                    }
                }
            }
        }

        map.insert(
            "context.memory.path".to_string(),
            ConfigSourceInfo {
                value: active_value,
                source: active_source,
            },
        );
    }

    resolve!(
        "context.memory.required",
        None::<bool>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.required)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.required)),
        false
    );

    resolve!(
        "context.memory.strategy",
        None::<MemorySelectionStrategy>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.strategy)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.strategy)),
        MemorySelectionStrategy::Budgeted
    );

    resolve!(
        "context.memory.max_tokens",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.max_tokens)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.max_tokens)),
        8000
    );

    resolve!(
        "context.memory.max_bytes",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.max_bytes)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.max_bytes)),
        524_288
    );

    resolve!(
        "context.memory.pinned_section",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.pinned_section.clone())),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.pinned_section.clone())),
        "Facts"
    );

    resolve!(
        "context.memory.snapshot",
        None::<ContextSnapshotMode>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.snapshot)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.snapshot)),
        ContextSnapshotMode::Session
    );

    resolve!(
        "context.memory.write_mode",
        None::<MemoryWriteMode>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.write_mode)),
        (|c: &WorkspaceConfig| c
            .context
            .as_ref()
            .and_then(|d| d.memory.as_ref())
            .and_then(|m| m.write_mode)),
        MemoryWriteMode::Proposal
    );

    resolve!(
        "prompt.override",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.prompt.as_ref().and_then(|p| p.r#override.clone())),
        (|c: &WorkspaceConfig| c.prompt.as_ref().and_then(|p| p.r#override.clone())),
        Value::Null
    );

    resolve!(
        "prompt.override_file",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.prompt.as_ref().and_then(|p| p.override_file.clone())),
        (|c: &WorkspaceConfig| c.prompt.as_ref().and_then(|p| p.override_file.clone())),
        Value::Null
    );

    resolve!(
        "policies.paths.allow_read",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.paths.allow_read.clone())),
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.paths.allow_read.clone())),
        vec![".".to_string()]
    );

    resolve!(
        "policies.paths.allow_write",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .policies
            .as_ref()
            .and_then(|p| p.paths.allow_write.clone())),
        (|c: &WorkspaceConfig| c
            .policies
            .as_ref()
            .and_then(|p| p.paths.allow_write.clone())),
        vec!["docs/".to_string(), ".gestalt/".to_string()]
    );

    resolve!(
        "policies.paths.deny_write",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.paths.deny_write.clone())),
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.paths.deny_write.clone())),
        vec![
            ".git/".to_string(),
            "secrets/".to_string(),
            ".env".to_string(),
            "*.key".to_string()
        ]
    );

    resolve!(
        "policies.paths.deny_read",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.paths.deny_read.clone())),
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.paths.deny_read.clone())),
        vec![
            ".env".to_string(),
            ".env.*".to_string(),
            "*.key".to_string(),
            "*.pem".to_string(),
            "*secret*".to_string(),
            "*credential*".to_string(),
            ".git/".to_string(),
            "secrets/".to_string()
        ]
    );

    resolve!(
        "policies.bash.default",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.bash.default)),
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.bash.default)),
        "confirm"
    );

    resolve!(
        "policies.bash.yolo_allow",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.bash.yolo_allow.clone())),
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.bash.yolo_allow.clone())),
        vec![
            "cargo test".to_string(),
            "cargo check".to_string(),
            "cargo build".to_string(),
            "ls".to_string(),
            "grep".to_string(),
            "rg".to_string(),
            "find".to_string(),
            "cat".to_string()
        ]
    );

    resolve!(
        "policies.bash.always_confirm",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .policies
            .as_ref()
            .and_then(|p| p.bash.always_confirm.clone())),
        (|c: &WorkspaceConfig| c
            .policies
            .as_ref()
            .and_then(|p| p.bash.always_confirm.clone())),
        vec![
            "rm".to_string(),
            "sudo".to_string(),
            "docker".to_string(),
            "git push".to_string(),
            "git reset".to_string(),
            "ssh".to_string(),
            "curl".to_string(),
            "wget".to_string()
        ]
    );

    resolve!(
        "policies.bash.always_deny",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.bash.always_deny.clone())),
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.bash.always_deny.clone())),
        vec![
            "dd".to_string(),
            "mkfs".to_string(),
            "fdisk".to_string(),
            "chmod 777".to_string()
        ]
    );

    resolve!(
        "policies.network.default",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.network.default)),
        (|c: &WorkspaceConfig| c.policies.as_ref().and_then(|p| p.network.default)),
        "confirm"
    );

    resolve!(
        "policies.network.allow_domains",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .policies
            .as_ref()
            .and_then(|p| p.network.allow_domains.clone())),
        (|c: &WorkspaceConfig| c
            .policies
            .as_ref()
            .and_then(|p| p.network.allow_domains.clone())),
        Vec::<String>::new()
    );

    resolve!(
        "policies.network.deny_domains",
        None::<Vec<String>>,
        None::<&str>,
        (|c: &WorkspaceConfig| c
            .policies
            .as_ref()
            .and_then(|p| p.network.deny_domains.clone())),
        (|c: &WorkspaceConfig| c
            .policies
            .as_ref()
            .and_then(|p| p.network.deny_domains.clone())),
        Vec::<String>::new()
    );

    resolve!(
        "observe.run_log_dir",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.observe.as_ref().and_then(|d| d.run_log_dir.clone())),
        (|c: &WorkspaceConfig| c.observe.as_ref().and_then(|d| d.run_log_dir.clone())),
        ".gestalt/runs"
    );

    resolve!(
        "observe.log_format",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.observe.as_ref().and_then(|d| d.log_format)),
        (|c: &WorkspaceConfig| c.observe.as_ref().and_then(|d| d.log_format)),
        "jsonl"
    );

    resolve!(
        "tui.diagnostics.max_log_lines",
        None::<usize>,
        Some("GESTALT_TUI_MAX_LOG_LINES"),
        (|c: &WorkspaceConfig| c
            .tui
            .as_ref()
            .and_then(|t| t.diagnostics.as_ref())
            .and_then(|d| d.max_log_lines)),
        (|c: &WorkspaceConfig| c
            .tui
            .as_ref()
            .and_then(|t| t.diagnostics.as_ref())
            .and_then(|d| d.max_log_lines)),
        1000
    );

    resolve!(
        "mcp.discovery_threshold",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.mcp.as_ref().and_then(|m| m.discovery_threshold)),
        (|c: &WorkspaceConfig| c.mcp.as_ref().and_then(|m| m.discovery_threshold)),
        5
    );

    Ok(map)
}

fn resolve_variant(
    variants: &HashMap<String, ModelVariantConfig>,
    variant_name: &str,
    visited: &mut Vec<String>,
) -> Result<ModelOptionsConfig, HarnessError> {
    if visited.contains(&variant_name.to_string()) {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: "variants".to_string(),
            reason: format!("cycle detected in variant inheritance: {:?}", visited),
        }));
    }
    visited.push(variant_name.to_string());

    let variant_cfg = variants.get(variant_name).ok_or_else(|| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: "variant".to_string(),
            reason: format!("variant '{}' not found", variant_name),
        })
    })?;

    let mut base_options = if let Some(ref parent_name) = variant_cfg.extends {
        resolve_variant(variants, parent_name, visited)?
    } else {
        ModelOptionsConfig::default()
    };

    if let Some(ref options) = variant_cfg.options {
        base_options = merge_options(base_options, options.clone());
    }

    visited.pop();
    Ok(base_options)
}

fn merge_options(
    mut base: ModelOptionsConfig,
    override_opts: ModelOptionsConfig,
) -> ModelOptionsConfig {
    if override_opts.max_output_tokens.is_some() {
        base.max_output_tokens = override_opts.max_output_tokens;
    }
    if override_opts.temperature.is_some() {
        base.temperature = override_opts.temperature;
    }
    if override_opts.top_p.is_some() {
        base.top_p = override_opts.top_p;
    }
    if override_opts.reasoning_effort.is_some() {
        base.reasoning_effort = override_opts.reasoning_effort;
    }
    if override_opts.text_verbosity.is_some() {
        base.text_verbosity = override_opts.text_verbosity;
    }
    if override_opts.thinking.is_some() {
        base.thinking = override_opts.thinking;
    }
    if let Some(ref override_map) = override_opts.adapter_options {
        let mut base_map = base.adapter_options.unwrap_or_default();
        for (k, v) in override_map {
            base_map.insert(k.clone(), v.clone());
        }
        base.adapter_options = Some(base_map);
    }
    base
}
