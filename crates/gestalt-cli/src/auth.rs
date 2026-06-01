use gestalt_core::{ConfigError, HarnessError};
use gestalt_models::{AnthropicProvider, OpenAiProvider, registry};

use crate::config::EffectiveConfig;

pub fn resolve_auth(config: &EffectiveConfig, provider: &str) -> Result<String, HarnessError> {
    let provider_config = config.provider_json(provider);

    let env_var = match provider {
        "anthropic" => AnthropicProvider::new(provider_config)?.auth_config().api_key_env.clone(),
        "openai" | "openai-compatible" => {
            OpenAiProvider::new(provider_config)?.auth_config().api_key_env.clone()
        }
        other if registry::registered().iter().any(|name| name == other) => {
            OpenAiProvider::new(config.provider_json(other))?.auth_config().api_key_env.clone()
        }
        _ => {
            return Err(HarnessError::Config(ConfigError::InvalidValue {
                field: "provider".to_string(),
                reason: format!("unknown provider: {provider}"),
            }))
        }
    };

    let status = if std::env::var(&env_var).is_ok() {
        "present"
    } else {
        "missing"
    };

    Ok(format!("provider={provider} source=env variable={env_var} status={status}"))
}
