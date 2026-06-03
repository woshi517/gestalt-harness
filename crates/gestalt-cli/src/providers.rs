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
    if !list_providers(config).contains(&provider.to_string()) {
        return Err(HarnessError::Provider(gestalt_core::ProviderError::UnknownProvider(
            provider.to_string(),
        )));
    }
    Ok(config.provider_json(provider))
}

pub async fn probe_provider(config: &EffectiveConfig, provider: &str) -> Result<(), HarnessError> {
    let auth = resolve_auth(config, provider)?;
    if auth.status != "present" {
        return Err(HarnessError::Provider(gestalt_core::ProviderError::AuthFailed {
            provider: provider.to_string(),
        }));
    }
    let api_key = std::env::var(&auth.variable).map_err(|_| {
        HarnessError::Provider(gestalt_core::ProviderError::AuthFailed {
            provider: provider.to_string(),
        })
    })?;

    let provider_config = config.provider_json(provider);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| HarnessError::Provider(gestalt_core::ProviderError::Transport(std::io::Error::other(e))))?;

    if provider == "anthropic" {
        let base_url = provider_config
            .get("base_url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("https://api.anthropic.com")
            .trim_end_matches('/');
        let url = format!("{base_url}/v1/models");
        let resp = client
            .get(&url)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|e| HarnessError::Provider(gestalt_core::ProviderError::Transport(std::io::Error::other(e))))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(HarnessError::Provider(gestalt_core::ProviderError::UnexpectedResponse {
                details: format!("Anthropic API returned status {}: {}", status, body),
            }));
        }
    } else {
        // openai or openai-compatible
        let base_url = provider_config
            .get("base_url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/');
        let url = format!("{base_url}/models");
        let resp = client
            .get(&url)
            .bearer_auth(&api_key)
            .send()
            .await
            .map_err(|e| HarnessError::Provider(gestalt_core::ProviderError::Transport(std::io::Error::other(e))))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(HarnessError::Provider(gestalt_core::ProviderError::UnexpectedResponse {
                details: format!("OpenAI Compatible API returned status {}: {}", status, body),
            }));
        }
    }

    Ok(())
}

pub async fn doctor_provider(
    config: &EffectiveConfig,
    provider: &str,
    live: bool,
) -> Result<ProviderDoctorResult, HarnessError> {
    let auth = resolve_auth(config, provider)?;
    let mut auth_status = auth.status.clone();

    if live && auth_status == "present" {
        match probe_provider(config, provider).await {
            Ok(_) => {
                auth_status = "ready".to_string();
            }
            Err(err) => {
                auth_status = format!("error: {}", err);
            }
        }
    }

    Ok(ProviderDoctorResult {
        provider: provider.to_string(),
        auth_variable: auth.variable,
        auth_status,
    })
}
