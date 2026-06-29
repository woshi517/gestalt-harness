use crate::auth::{delete_keychain_secret, set_keychain_secret};
use crate::config::{
    global_config_path, legacy_global_config_path, mutate_workspace_config_file,
    write_workspace_config_file, EffectiveConfig, ProfileConfig, ProviderConfig, WorkspaceConfig,
};
use crate::reports::{ConnectReport, DisconnectReport};
use gestalt_core::{ApiFormat, ConfigError, HarnessError};
use std::collections::HashMap;

pub fn connect_provider(
    _config: &EffectiveConfig,
    provider: &str,
    api_key: Option<String>,
    no_keychain: bool,
    set_default: bool,
    name_opt: Option<String>,
    base_url_opt: Option<String>,
    default_model_opt: Option<String>,
    api_key_env_opt: Option<String>,
    interaction: Option<&dyn crate::InteractionProvider>,
) -> Result<ConnectReport, HarnessError> {
    let key_val = if let Some(key) = api_key {
        let trimmed = key.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    } else if !no_keychain && provider == "openrouter" {
        interaction.and_then(|i| i.prompt_password("Enter OpenRouter API key:"))
    } else if !no_keychain && provider == "openai-compatible" {
        interaction
            .and_then(|i| i.prompt_password("Enter API key (optional, press Enter to skip):"))
    } else {
        None
    };

    if no_keychain && key_val.is_some() {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: "no_keychain".to_string(),
            reason: "API key cannot be provided when --no-keychain is set, as gestalt does not store raw secrets in config files. Please set the environment variable instead.".to_string(),
        }));
    }

    let (conn_name, api_format, base_url, default_model, api_key_env, headers, models_endpoint) =
        match provider {
            "openrouter" => {
                let builtin = crate::catalog::get_builtin_provider("openrouter").unwrap();
                (
                    "openrouter".to_string(),
                    ApiFormat::OpenAiChatCompletions,
                    builtin.base_url.unwrap(),
                    builtin.default_model.unwrap(),
                    builtin.api_key_env,
                    builtin.headers,
                    builtin.models_endpoint,
                )
            }
            "openai-compatible" => {
                let conn_name = name_opt.ok_or_else(|| {
                    HarnessError::Config(ConfigError::InvalidValue {
                        field: "name".to_string(),
                        reason: "connection name is required for openai-compatible provider"
                            .to_string(),
                    })
                })?;
                let base_url = base_url_opt.ok_or_else(|| {
                    HarnessError::Config(ConfigError::InvalidValue {
                        field: "base_url".to_string(),
                        reason: "base URL is required for openai-compatible provider".to_string(),
                    })
                })?;
                let default_model = default_model_opt.ok_or_else(|| {
                    HarnessError::Config(ConfigError::InvalidValue {
                        field: "default_model".to_string(),
                        reason: "default model is required for openai-compatible provider"
                            .to_string(),
                    })
                })?;
                let env_val = api_key_env_opt.or_else(|| {
                    if key_val.is_none() {
                        Some("none".to_string())
                    } else {
                        Some(format!(
                            "{}_API_KEY",
                            conn_name.to_uppercase().replace('-', "_")
                        ))
                    }
                });
                (
                    conn_name,
                    ApiFormat::OpenAiChatCompletions,
                    base_url,
                    default_model,
                    env_val,
                    None,
                    None,
                )
            }
            _ => {
                if let Some(builtin) = crate::catalog::get_builtin_provider(provider) {
                    (
                        provider.to_string(),
                        builtin
                            .api_format
                            .unwrap_or(ApiFormat::OpenAiChatCompletions),
                        builtin.base_url.unwrap_or_default(),
                        builtin.default_model.unwrap_or_default(),
                        builtin.api_key_env.or_else(|| {
                            Some(format!(
                                "{}_API_KEY",
                                provider.to_uppercase().replace('-', "_")
                            ))
                        }),
                        builtin.headers,
                        builtin.models_endpoint,
                    )
                } else {
                    return Err(HarnessError::Config(ConfigError::InvalidValue {
                        field: "provider".to_string(),
                        reason: format!("unknown provider connection type: '{provider}'"),
                    }));
                }
            }
        };

    if no_keychain {
        let env_var = api_key_env.as_deref().unwrap_or("OPENROUTER_API_KEY");
        if env_var != "none" && std::env::var(env_var).is_err() {
            return Err(HarnessError::Config(ConfigError::InvalidValue {
                field: "api_key".to_string(),
                reason: format!("Environment variable '{}' is not set. When --no-keychain is active, you must set the environment variable containing the API key before connecting.", env_var),
            }));
        }
    }

    if key_val.is_none() && provider == "openrouter" && !no_keychain {
        let env_var = api_key_env.as_deref().unwrap_or("OPENROUTER_API_KEY");
        if std::env::var(env_var).is_err() {
            return Err(HarnessError::Config(ConfigError::InvalidValue {
                field: "api_key".to_string(),
                reason: "API key is required for OpenRouter connection".to_string(),
            }));
        }
    }

    let mut auth_ref = None;
    let mut keychain_stored = false;

    if let Some(ref secret) = key_val {
        if !no_keychain {
            let account = format!("gestalt/{conn_name}");
            match set_keychain_secret(&account, secret) {
                Ok(_) => {
                    auth_ref = Some(format!("keychain:{account}"));
                    keychain_stored = true;
                }
                Err(err) => {
                    return Err(HarnessError::Config(ConfigError::InvalidValue {
                        field: "keychain".to_string(),
                        reason: format!("failed to store secret in keychain: {err}"),
                    }));
                }
            }
        }
    }

    let global_path = global_config_path();
    let mut profile_created = None;
    mutate_workspace_config_file(&global_path, |ws_cfg| {
        let prov_config = ProviderConfig {
            id: Some(conn_name.clone()),
            display_name: Some(conn_name.clone()),
            protocol: None,
            api_format: Some(api_format),
            base_url: Some(base_url),
            default_model: Some(default_model),
            api_key_env: if auth_ref.is_some() {
                None
            } else {
                api_key_env.clone()
            },
            auth_ref,
            api_key: None,
            request_path: None,
            request: None,
            capabilities: None,
            models: HashMap::new(),
            models_endpoint,
            headers,
            kind: None,
        };
        ws_cfg.providers.insert(conn_name.clone(), prov_config);

        if set_default {
            let profile_name = if conn_name == "openrouter" {
                "default".to_string()
            } else {
                conn_name.clone()
            };
            let mut defaults = ws_cfg.defaults.clone().unwrap_or_default();
            defaults.profile = Some(profile_name.clone());
            ws_cfg.defaults = Some(defaults);

            let profile_cfg = ProfileConfig {
                provider: Some(conn_name.clone()),
                model: None,
                ..ProfileConfig::default()
            };
            ws_cfg.profiles.insert(profile_name.clone(), profile_cfg);
            profile_created = Some(profile_name);
        }
    })?;

    Ok(ConnectReport {
        provider: conn_name,
        status: "connected".to_string(),
        profile_created,
        keychain_stored,
    })
}

pub fn disconnect_provider(
    config: &EffectiveConfig,
    provider: &str,
    force: bool,
) -> Result<DisconnectReport, HarnessError> {
    let mut referenced_by = Vec::new();
    for (name, prof) in &config.profiles {
        if prof.provider.as_deref() == Some(provider) {
            referenced_by.push(name.clone());
        }
    }

    if !referenced_by.is_empty() && !force {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: "provider".to_string(),
            reason: format!(
                "provider connection '{}' is still referenced by profiles: {}. Use --force to disconnect anyway.",
                provider,
                referenced_by.join(", ")
            ),
        }));
    }

    let global_path = global_config_path();
    let legacy_global_path = legacy_global_config_path();
    if !global_path.exists() && !legacy_global_path.exists() {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: "global_config".to_string(),
            reason: "global configuration file does not exist".to_string(),
        }));
    }

    let mut ws_cfg = if global_path.exists() {
        WorkspaceConfig::from_file(&global_path)?
    } else {
        WorkspaceConfig::from_file(&legacy_global_path)?
    };
    ws_cfg.providers.remove(provider);

    let mut profile_removed = None;
    let mut keys_to_remove = Vec::new();
    for (name, prof) in &ws_cfg.profiles {
        if prof.provider.as_deref() == Some(provider) {
            keys_to_remove.push(name.clone());
        }
    }
    for k in keys_to_remove {
        ws_cfg.profiles.remove(&k);
        profile_removed = Some(k);
    }

    if let Some(ref mut defaults) = ws_cfg.defaults {
        if let Some(ref active_p) = defaults.profile {
            if active_p == provider || profile_removed.as_ref() == Some(active_p) {
                defaults.profile = None;
            }
        }
    }

    let account = format!("gestalt/{provider}");
    let keychain_cleared = delete_keychain_secret(&account).is_ok();

    write_workspace_config_file(&global_path, &ws_cfg)?;

    Ok(DisconnectReport {
        provider: provider.to_string(),
        profile_removed,
        keychain_cleared,
    })
}
