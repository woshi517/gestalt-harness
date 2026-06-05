use gestalt_core::HarnessError;
use gestalt_models::registry;

use crate::{auth::resolve_auth, config::EffectiveConfig, output::ProviderDoctorResult};

pub fn list_providers(config: &EffectiveConfig) -> Vec<String> {
    let mut providers = registry::registered();
    let builtins = vec![
        "openrouter".to_string(),
        "ollama".to_string(),
        "groq".to_string(),
        "together".to_string(),
    ];
    for b in builtins {
        if !providers.contains(&b) {
            providers.push(b);
        }
    }
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
        return Err(HarnessError::Provider(
            gestalt_core::ProviderError::UnknownProvider(provider.to_string()),
        ));
    }
    Ok(config.provider_json(provider))
}

pub async fn probe_provider(config: &EffectiveConfig, provider: &str) -> Result<(), HarnessError> {
    let mut temp_cfg = config.clone();
    temp_cfg.defaults.provider = Some(provider.to_string());
    temp_cfg.defaults.profile = None;
    let resolved = temp_cfg.resolve_provider()?;
    let provider_config = resolved.provider_json();
    let auth_config = gestalt_models::auth::provider_auth_config(
        &provider_config,
        &resolved.provider_name,
        "DUMMY_KEY",
    )?;

    let cred_resolver = crate::auth::build_credential_resolver(None, false);
    let credential = cred_resolver.resolve(&auth_config).map_err(|_| {
        HarnessError::Provider(gestalt_core::ProviderError::AuthFailed {
            provider: provider.to_string(),
        })
    })?;
    let api_key = credential.secret().to_string();

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| {
            HarnessError::Provider(gestalt_core::ProviderError::Transport(
                std::io::Error::other(e),
            ))
        })?;

    let mut req_builder = if resolved.kind == "anthropic" {
        let base_url = resolved
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com")
            .trim_end_matches('/');
        let url = format!("{base_url}/v1/models");
        client
            .get(&url)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
    } else {
        let base_url = resolved
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/');
        let url = format!("{base_url}/models");
        client.get(&url).bearer_auth(&api_key)
    };

    if let Some(ref hdrs) = resolved.headers {
        for (k, v) in hdrs {
            req_builder = req_builder.header(k, v);
        }
    }

    let resp = req_builder.send().await.map_err(|e| {
        HarnessError::Provider(gestalt_core::ProviderError::Transport(
            std::io::Error::other(e),
        ))
    })?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(HarnessError::Provider(
            gestalt_core::ProviderError::UnexpectedResponse {
                details: format!("API returned status {}: {}", status, body),
            },
        ));
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
        auth_source: auth.source,
    })
}
