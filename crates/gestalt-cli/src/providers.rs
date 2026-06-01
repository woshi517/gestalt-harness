use gestalt_core::{ConfigError, HarnessError};
use gestalt_models::registry;

use crate::{auth::resolve_auth, config::EffectiveConfig};

pub fn list_providers(config: &EffectiveConfig) -> Vec<String> {
    let mut providers = registry::registered();
    for provider in config.providers.keys() {
        if !providers.contains(provider) {
            providers.push(provider.clone());
        }
    }
    providers.sort();
    providers
}

pub fn inspect_provider(config: &EffectiveConfig, provider: &str) -> Result<String, HarnessError> {
    let value = config.provider_json(provider);
    serde_json::to_string_pretty(&value).map_err(|err| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: provider.to_string(),
            reason: err.to_string(),
        })
    })
}

pub fn doctor_provider(config: &EffectiveConfig, provider: &str) -> Result<String, HarnessError> {
    let auth = resolve_auth(config, provider)?;
    Ok(format!("provider={provider}\n{auth}"))
}
