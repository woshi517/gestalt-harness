use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use gestalt_core::{ConfigError, ExecutionMode, HarnessError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub observe: ObserveConfig,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub max_turns: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsConfig {
    pub bash_timeout_secs: Option<u64>,
    pub max_output_tokens: Option<usize>,
    pub sandbox_type: Option<String>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            bash_timeout_secs: Some(60),
            max_output_tokens: Some(4_000),
            sandbox_type: Some("none".to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextConfig {
    pub max_context_window: Option<usize>,
    pub reserved_output_tokens: Option<usize>,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_context_window: Some(120_000),
            reserved_output_tokens: Some(8_000),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserveConfig {
    pub run_log_dir: Option<String>,
    pub log_format: Option<String>,
}

impl Default for ObserveConfig {
    fn default() -> Self {
        Self {
            run_log_dir: Some(".gestalt/runs".to_string()),
            log_format: Some("jsonl".to_string()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub protocol: Option<String>,
    pub base_url: Option<String>,
    pub default_model: Option<String>,
    pub api_key_env: Option<String>,
    pub auth_ref: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CliOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub max_turns: Option<usize>,
    pub workspace: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveConfig {
    pub workspace_root: PathBuf,
    pub defaults: DefaultsConfig,
    pub tools: ToolsConfig,
    pub context: ContextConfig,
    pub observe: ObserveConfig,
    pub providers: HashMap<String, ProviderConfig>,
}

impl WorkspaceConfig {
    pub fn from_file(path: &Path) -> Result<Self, HarnessError> {
        let input = fs::read_to_string(path).map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: path.display().to_string(),
                reason: err.to_string(),
            })
        })?;
        toml::from_str(&input).map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: path.display().to_string(),
                reason: err.to_string(),
            })
        })
    }

    fn merge(mut self, other: Self) -> Self {
        self.defaults.provider = other.defaults.provider.or(self.defaults.provider);
        self.defaults.model = other.defaults.model.or(self.defaults.model);
        self.defaults.mode = other.defaults.mode.or(self.defaults.mode);
        self.defaults.max_turns = other.defaults.max_turns.or(self.defaults.max_turns);

        self.tools.bash_timeout_secs = other
            .tools
            .bash_timeout_secs
            .or(self.tools.bash_timeout_secs);
        self.tools.max_output_tokens = other
            .tools
            .max_output_tokens
            .or(self.tools.max_output_tokens);
        self.tools.sandbox_type = other.tools.sandbox_type.or(self.tools.sandbox_type);

        self.context.max_context_window = other
            .context
            .max_context_window
            .or(self.context.max_context_window);
        self.context.reserved_output_tokens = other
            .context
            .reserved_output_tokens
            .or(self.context.reserved_output_tokens);

        self.observe.run_log_dir = other.observe.run_log_dir.or(self.observe.run_log_dir);
        self.observe.log_format = other.observe.log_format.or(self.observe.log_format);

        self.providers.extend(other.providers);
        self
    }
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
    let global_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gestalt/config.toml");
    let workspace_path = workspace_root.join(".gestalt/config.toml");

    let mut config = WorkspaceConfig::default();
    if global_path.exists() {
        config = config.merge(WorkspaceConfig::from_file(&global_path)?);
    }
    if workspace_path.exists() {
        config = config.merge(WorkspaceConfig::from_file(&workspace_path)?);
    }

    if let Ok(provider) = std::env::var("GESTALT_PROVIDER") {
        config.defaults.provider = Some(provider);
    }
    if let Ok(model) = std::env::var("GESTALT_MODEL") {
        config.defaults.model = Some(model);
    }
    if let Ok(mode) = std::env::var("GESTALT_MODE") {
        config.defaults.mode = Some(mode);
    }
    if let Ok(max_turns) = std::env::var("GESTALT_MAX_TURNS") {
        if let Ok(max_turns) = max_turns.parse::<usize>() {
            config.defaults.max_turns = Some(max_turns);
        }
    }

    if let Some(provider) = &overrides.provider {
        config.defaults.provider = Some(provider.clone());
    }
    if let Some(model) = &overrides.model {
        config.defaults.model = Some(model.clone());
    }
    if let Some(mode) = &overrides.mode {
        config.defaults.mode = Some(mode.clone());
    }
    if let Some(max_turns) = overrides.max_turns {
        config.defaults.max_turns = Some(max_turns);
    }

    Ok(EffectiveConfig {
        workspace_root,
        defaults: config.defaults,
        tools: config.tools,
        context: config.context,
        observe: config.observe,
        providers: config.providers,
    })
}

impl EffectiveConfig {
    pub fn selected_provider(&self) -> Result<String, HarnessError> {
        Ok(self
            .defaults
            .provider
            .clone()
            .unwrap_or_else(|| "anthropic".to_string()))
    }

    pub fn selected_model(&self) -> Option<String> {
        self.defaults.model.clone()
    }

    pub fn selected_mode(&self) -> Result<ExecutionMode, HarnessError> {
        mode_from_str(self.defaults.mode.as_deref().unwrap_or("confirm"))
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

    pub fn provider_json(&self, provider: &str) -> Value {
        let configured = self.providers.get(provider).cloned().unwrap_or_default();
        json!({
            "id": configured.id.unwrap_or_else(|| provider.to_string()),
            "display_name": configured.display_name,
            "protocol": configured.protocol,
            "base_url": configured.base_url,
            "default_model": configured.default_model.or_else(|| self.selected_model()),
            "api_key_env": configured.api_key_env,
            "auth_ref": configured.auth_ref,
        })
    }

    pub fn workspace_file(&self, name: &str) -> PathBuf {
        self.workspace_root.join(".gestalt").join(name)
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

pub fn explain_config(overrides: &CliOverrides) -> Result<HashMap<String, ConfigSourceInfo>, HarnessError> {
    let workspace_root = overrides
        .workspace
        .clone()
        .unwrap_or(std::env::current_dir().map_err(|err| {
            HarnessError::Config(ConfigError::InvalidValue {
                field: "workspace".to_string(),
                reason: err.to_string(),
            })
        })?);
    let global_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gestalt/config.toml");
    let workspace_path = workspace_root.join(".gestalt/config.toml");

    let mut global_cfg = None;
    if global_path.exists() {
        global_cfg = Some(WorkspaceConfig::from_file(&global_path)?);
    }
    let mut ws_cfg = None;
    if workspace_path.exists() {
        ws_cfg = Some(WorkspaceConfig::from_file(&workspace_path)?);
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

            map.insert($key.to_string(), ConfigSourceInfo {
                value: active_value,
                source: active_source,
            });
        };
    }

    resolve!(
        "defaults.provider",
        overrides.provider,
        Some("GESTALT_PROVIDER"),
        (|c: &WorkspaceConfig| c.defaults.provider.clone()),
        (|c: &WorkspaceConfig| c.defaults.provider.clone()),
        "anthropic"
    );

    resolve!(
        "defaults.model",
        overrides.model,
        Some("GESTALT_MODEL"),
        (|c: &WorkspaceConfig| c.defaults.model.clone()),
        (|c: &WorkspaceConfig| c.defaults.model.clone()),
        Value::Null
    );

    resolve!(
        "defaults.mode",
        overrides.mode,
        Some("GESTALT_MODE"),
        (|c: &WorkspaceConfig| c.defaults.mode.clone()),
        (|c: &WorkspaceConfig| c.defaults.mode.clone()),
        "confirm"
    );

    resolve!(
        "defaults.max_turns",
        overrides.max_turns,
        Some("GESTALT_MAX_TURNS"),
        (|c: &WorkspaceConfig| c.defaults.max_turns),
        (|c: &WorkspaceConfig| c.defaults.max_turns),
        50
    );

    resolve!(
        "tools.bash_timeout_secs",
        None::<u64>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.tools.bash_timeout_secs),
        (|c: &WorkspaceConfig| c.tools.bash_timeout_secs),
        60
    );

    resolve!(
        "tools.max_output_tokens",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.tools.max_output_tokens),
        (|c: &WorkspaceConfig| c.tools.max_output_tokens),
        4000
    );

    resolve!(
        "tools.sandbox_type",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.tools.sandbox_type.clone()),
        (|c: &WorkspaceConfig| c.tools.sandbox_type.clone()),
        "none"
    );

    resolve!(
        "context.max_context_window",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.context.max_context_window),
        (|c: &WorkspaceConfig| c.context.max_context_window),
        120000
    );

    resolve!(
        "context.reserved_output_tokens",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.context.reserved_output_tokens),
        (|c: &WorkspaceConfig| c.context.reserved_output_tokens),
        8000
    );

    resolve!(
        "observe.run_log_dir",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.observe.run_log_dir.clone()),
        (|c: &WorkspaceConfig| c.observe.run_log_dir.clone()),
        ".gestalt/runs"
    );

    resolve!(
        "observe.log_format",
        None::<String>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.observe.log_format.clone()),
        (|c: &WorkspaceConfig| c.observe.log_format.clone()),
        "jsonl"
    );

    Ok(map)
}
