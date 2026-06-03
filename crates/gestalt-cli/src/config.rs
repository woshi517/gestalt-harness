use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use gestalt_core::{ConfigError, ExecutionMode, HarnessError, ProviderError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuiConfig {
    #[serde(default)]
    pub diagnostics: Option<TuiDiagnosticsConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TuiDiagnosticsConfig {
    pub max_log_lines: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub max_turns: Option<usize>,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    pub provider: Option<String>,
    pub model: Option<String>,
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
    pub kind: Option<String>,
    pub models_endpoint: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CliOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub max_turns: Option<usize>,
    pub workspace: Option<PathBuf>,
    pub profile: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveConfig {
    pub workspace_root: PathBuf,
    pub defaults: DefaultsConfig,
    pub tools: ToolsConfig,
    pub context: ContextConfig,
    pub observe: ObserveConfig,
    pub providers: HashMap<String, ProviderConfig>,
    pub profiles: HashMap<String, ProfileConfig>,
    pub provider_override: Option<String>,
    pub model_override: Option<String>,
    pub tui: TuiConfig,
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
        if let Some(other_defaults) = other.defaults {
            let mut self_defaults = self.defaults.unwrap_or_default();
            self_defaults.provider = other_defaults.provider.or(self_defaults.provider);
            self_defaults.model = other_defaults.model.or(self_defaults.model);
            self_defaults.mode = other_defaults.mode.or(self_defaults.mode);
            self_defaults.max_turns = other_defaults.max_turns.or(self_defaults.max_turns);
            self_defaults.profile = other_defaults.profile.or(self_defaults.profile);
            self.defaults = Some(self_defaults);
        }

        if let Some(other_tools) = other.tools {
            let mut self_tools = self.tools.unwrap_or_default();
            self_tools.bash_timeout_secs = other_tools.bash_timeout_secs.or(self_tools.bash_timeout_secs);
            self_tools.max_output_tokens = other_tools.max_output_tokens.or(self_tools.max_output_tokens);
            self_tools.sandbox_type = other_tools.sandbox_type.or(self_tools.sandbox_type);
            self.tools = Some(self_tools);
        }

        if let Some(other_context) = other.context {
            let mut self_context = self.context.unwrap_or_default();
            self_context.max_context_window = other_context.max_context_window.or(self_context.max_context_window);
            self_context.reserved_output_tokens = other_context.reserved_output_tokens.or(self_context.reserved_output_tokens);
            self.context = Some(self_context);
        }

        if let Some(other_observe) = other.observe {
            let mut self_observe = self.observe.unwrap_or_default();
            self_observe.run_log_dir = other_observe.run_log_dir.or(self_observe.run_log_dir);
            self_observe.log_format = other_observe.log_format.or(self_observe.log_format);
            self.observe = Some(self_observe);
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

        self.providers.extend(other.providers);
        self.profiles.extend(other.profiles);
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
        defaults.mode = Some(mode);
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
        defaults.mode = Some(mode.clone());
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
        c.max_context_window = c.max_context_window.or(d.max_context_window);
        c.reserved_output_tokens = c.reserved_output_tokens.or(d.reserved_output_tokens);
        c
    };

    let observe = {
        let mut o = config.observe.unwrap_or_default();
        let d = ObserveConfig::default();
        o.run_log_dir = o.run_log_dir.or(d.run_log_dir);
        o.log_format = o.log_format.or(d.log_format);
        o
    };

    let provider_override = overrides.provider.clone().or_else(|| std::env::var("GESTALT_PROVIDER").ok());
    let model_override = overrides.model.clone().or_else(|| std::env::var("GESTALT_MODEL").ok());

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
    if max_lines < 100 || max_lines > 50000 {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: "tui.diagnostics.max_log_lines".to_string(),
            reason: format!("Value {max_lines} must be between 100 and 50,000"),
        }));
    }
    diagnostics.max_log_lines = Some(max_lines);
    tui.diagnostics = Some(diagnostics);

    Ok(EffectiveConfig {
        workspace_root,
        defaults: config.defaults.unwrap_or_default(),
        tools,
        context,
        observe,
        providers: config.providers,
        profiles: config.profiles,
        provider_override,
        model_override,
        tui,
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
        })
    }
}

impl EffectiveConfig {
    pub fn resolve_provider(&self) -> Result<ResolvedProvider, HarnessError> {
        let active_profile = self.defaults.profile.clone();
        let active_provider = self.defaults.provider.clone();

        let (profile_name, mut provider_name, mut model_override) = if let Some(p) = active_profile {
            if let Some(prof_cfg) = self.profiles.get(&p) {
                let prov = prof_cfg.provider.clone().unwrap_or_else(|| p.clone());
                let model = prof_cfg.model.clone();
                (Some(p), prov, model)
            } else {
                match p.as_str() {
                    "default" => (Some(p), "openrouter".to_string(), Some("openrouter/free".to_string())),
                    "openrouter" => (Some(p), "openrouter".to_string(), Some("openrouter/free".to_string())),
                    "anthropic" => (Some(p), "anthropic".to_string(), Some("claude-3-5-sonnet-20241022".to_string())),
                    "openai" => (Some(p), "openai".to_string(), Some("gpt-4o-mini".to_string())),
                    "ollama" => (Some(p), "ollama".to_string(), Some("llama3".to_string())),
                    "groq" => (Some(p), "groq".to_string(), Some("llama3-8b-8192".to_string())),
                    "together" => (Some(p), "together".to_string(), Some("mistralai/Mixtral-8x7B-Instruct-v0.1".to_string())),
                    _ => {
                        return Err(HarnessError::Config(ConfigError::InvalidValue {
                            field: "defaults.profile".to_string(),
                            reason: format!("profile '{p}' not found in configuration or built-ins"),
                        }));
                    }
                }
            }
        } else if let Some(prov) = active_provider {
            let model = self.defaults.model.clone();
            (None, prov, model)
        } else {
            let p = "default".to_string();
            (Some(p), "openrouter".to_string(), Some("openrouter/free".to_string()))
        };

        // Apply explicit CLI / Env overrides (which beat profiles and defaults)
        if let Some(ref prov_ovr) = self.provider_override {
            provider_name = prov_ovr.clone();
        }
        if let Some(ref model_ovr) = self.model_override {
            model_override = Some(model_ovr.clone());
        }

        let (kind, base_url, default_model, api_key_env, auth_ref, models_endpoint, headers) = 
            if let Some(prov_cfg) = self.providers.get(&provider_name) {
                let kind = prov_cfg.kind.clone().or_else(|| {
                    if provider_name == "anthropic" {
                        Some("anthropic".to_string())
                    } else if provider_name == "openai" {
                        Some("openai".to_string())
                    } else if gestalt_models::registry::registered().contains(&provider_name) {
                        Some(provider_name.clone())
                    } else {
                        Some("openai-compatible".to_string())
                    }
                }).unwrap();
                let base = prov_cfg.base_url.clone();
                let model = prov_cfg.default_model.clone();
                let env = prov_cfg.api_key_env.clone();
                let auth = prov_cfg.auth_ref.clone();
                let endpoint = prov_cfg.models_endpoint.clone();
                let hdrs = prov_cfg.headers.clone();
                (kind, base, model, env, auth, endpoint, hdrs)
            } else if let Some(builtin) = crate::provider_catalog::get_builtin_provider(&provider_name) {
                (
                    builtin.kind.clone().unwrap_or_else(|| "openai-compatible".to_string()),
                    builtin.base_url,
                    builtin.default_model,
                    builtin.api_key_env,
                    builtin.auth_ref,
                    builtin.models_endpoint,
                    builtin.headers,
                )
            } else if gestalt_models::registry::registered().contains(&provider_name) {
                (
                    provider_name.clone(),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            } else {
                return Err(HarnessError::Provider(ProviderError::UnknownProvider(
                    provider_name.clone(),
                )));
            };

        let model = model_override
            .or(default_model)
            .unwrap_or_else(|| {
                if kind == "anthropic" {
                    "claude-3-5-sonnet-20241022".to_string()
                } else if kind == "openai" {
                    "gpt-4o-mini".to_string()
                } else {
                    "openrouter/free".to_string()
                }
            });

        Ok(ResolvedProvider {
            profile_name,
            provider_name,
            kind,
            model,
            auth_ref,
            base_url,
            api_key_env,
            models_endpoint,
            headers,
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
        let mut base = if let Some(builtin) = crate::provider_catalog::get_builtin_provider(provider) {
            builtin
        } else {
            ProviderConfig::default()
        };

        if let Some(id) = configured.id { base.id = Some(id); }
        if let Some(display) = configured.display_name { base.display_name = Some(display); }
        if let Some(protocol) = configured.protocol { base.protocol = Some(protocol); }
        if let Some(base_url) = configured.base_url { base.base_url = Some(base_url); }
        if let Some(def_model) = configured.default_model { base.default_model = Some(def_model); }
        if let Some(api_key_env) = configured.api_key_env { base.api_key_env = Some(api_key_env); }
        if let Some(auth_ref) = configured.auth_ref { base.auth_ref = Some(auth_ref); }
        if let Some(kind) = configured.kind { base.kind = Some(kind); }
        if let Some(models_endpoint) = configured.models_endpoint { base.models_endpoint = Some(models_endpoint); }
        if let Some(headers) = configured.headers { base.headers = Some(headers); }

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
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.mode.clone())),
        (|c: &WorkspaceConfig| c.defaults.as_ref().and_then(|d| d.mode.clone())),
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
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.sandbox_type.clone())),
        (|c: &WorkspaceConfig| c.tools.as_ref().and_then(|d| d.sandbox_type.clone())),
        "none"
    );

    resolve!(
        "context.max_context_window",
        None::<usize>,
        None::<&str>,
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.max_context_window)),
        (|c: &WorkspaceConfig| c.context.as_ref().and_then(|d| d.max_context_window)),
        120000
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
        (|c: &WorkspaceConfig| c.observe.as_ref().and_then(|d| d.log_format.clone())),
        (|c: &WorkspaceConfig| c.observe.as_ref().and_then(|d| d.log_format.clone())),
        "jsonl"
    );

    resolve!(
        "tui.diagnostics.max_log_lines",
        None::<usize>,
        Some("GESTALT_TUI_MAX_LOG_LINES"),
        (|c: &WorkspaceConfig| c.tui.as_ref().and_then(|t| t.diagnostics.as_ref()).and_then(|d| d.max_log_lines)),
        (|c: &WorkspaceConfig| c.tui.as_ref().and_then(|t| t.diagnostics.as_ref()).and_then(|d| d.max_log_lines)),
        1000
    );

    Ok(map)
}
