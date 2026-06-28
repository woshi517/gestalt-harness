use gestalt_core::HarnessError;

use crate::{auth::resolve_auth, config::EffectiveConfig, reports::ProviderDoctorResult};

pub fn list_providers(config: &EffectiveConfig) -> Vec<String> {
    let mut providers = gestalt_models::registered();
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
    let auth_config = resolved.auth.clone();

    let cred_resolver = crate::auth::build_credential_resolver(None, None);
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

    let (url, is_anthropic) = if let Some(ref ep) = resolved.models_endpoint {
        (
            ep.clone(),
            resolved.api_format() == gestalt_core::ApiFormat::AnthropicMessages,
        )
    } else if resolved.api_format() == gestalt_core::ApiFormat::AnthropicMessages {
        let base_url = if resolved.base_url.is_empty() {
            "https://api.anthropic.com"
        } else {
            &resolved.base_url
        };
        let base_url = base_url.trim_end_matches('/');
        (format!("{base_url}/v1/models"), true)
    } else {
        let base_url = if resolved.base_url.is_empty() {
            "https://api.openai.com/v1"
        } else {
            &resolved.base_url
        };
        let base_url = base_url.trim_end_matches('/');
        (format!("{base_url}/models"), false)
    };

    let mut req_builder = if is_anthropic {
        client
            .get(&url)
            .header("x-api-key", &api_key)
            .header("anthropic-version", "2023-06-01")
    } else {
        client.get(&url).bearer_auth(&api_key)
    };

    for (k, v) in &resolved.headers {
        req_builder = req_builder.header(k, v);
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
