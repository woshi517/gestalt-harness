use gestalt_core::{ConfigError, HarnessError};
use gestalt_models::{registry, AnthropicProvider, OpenAiProvider};

use crate::{config::EffectiveConfig, output::{AuthResolveReport, AuthDoctorReport, AuthDoctorEntry}};

pub fn resolve_auth(
    config: &EffectiveConfig,
    provider: &str,
) -> Result<AuthResolveReport, HarnessError> {
    let provider_config = config.provider_json(provider);

    let env_var = match provider {
        "anthropic" => AnthropicProvider::new(provider_config)?
            .auth_config()
            .api_key_env
            .clone(),
        "openai" | "openai-compatible" => OpenAiProvider::new(provider_config)?
            .auth_config()
            .api_key_env
            .clone(),
        other if registry::registered().iter().any(|name| name == other) => {
            OpenAiProvider::new(config.provider_json(other))?
                .auth_config()
                .api_key_env
                .clone()
        }
        _ => {
            return Err(HarnessError::Config(ConfigError::InvalidValue {
                field: "provider".to_string(),
                reason: format!("unknown provider: {provider}"),
            }))
        }
    };

    let status = if std::env::var(&env_var).is_ok() {
        "present".to_string()
    } else {
        "missing".to_string()
    };

    Ok(AuthResolveReport {
        provider: provider.to_string(),
        source: "env".to_string(),
        variable: env_var,
        status,
    })
}

pub fn auth_doctor(config: &EffectiveConfig) -> Result<AuthDoctorReport, HarnessError> {
    let mut entries = Vec::new();
    let mut checked_vars = std::collections::HashSet::new();

    let providers = crate::providers::list_providers(config);
    for provider in providers {
        if let Ok(auth_report) = resolve_auth(config, &provider) {
            let var_name = auth_report.variable;
            if checked_vars.insert(var_name.clone()) {
                let (status, value) = if let Ok(val) = std::env::var(&var_name) {
                    if val.is_empty() {
                        ("empty".to_string(), "[EMPTY]".to_string())
                    } else {
                        ("present".to_string(), "[REDACTED]".to_string())
                    }
                } else {
                    ("missing".to_string(), "[NOT SET]".to_string())
                };
                entries.push(AuthDoctorEntry {
                    variable: var_name,
                    status,
                    value,
                });
            }
        }
    }

    Ok(AuthDoctorReport { entries })
}
