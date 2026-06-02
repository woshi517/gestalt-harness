use gestalt_core::HarnessError;
use gestalt_models::registry;

use crate::{auth::resolve_auth, config::EffectiveConfig, output::ProviderDoctorResult};

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

pub fn inspect_provider(
    config: &EffectiveConfig,
    provider: &str,
) -> Result<serde_json::Value, HarnessError> {
    Ok(config.provider_json(provider))
}

pub fn doctor_provider(
    config: &EffectiveConfig,
    provider: &str,
) -> Result<ProviderDoctorResult, HarnessError> {
    let auth = resolve_auth(config, provider)?;
    Ok(ProviderDoctorResult {
        provider: provider.to_string(),
        auth_variable: auth.variable,
        auth_status: auth.status,
    })
}
