use std::{fmt, sync::Arc};

use gestalt_core::{ConfigError, HarnessError, ProviderError};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAuthConfig {
    pub provider_id: String,
    pub api_key_env: String,
    pub auth_ref: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedCredential {
    secret: Arc<str>,
    source: CredentialSource,
}

impl ResolvedCredential {
    #[must_use]
    pub fn new(secret: String, source: CredentialSource) -> Self {
        Self {
            secret: Arc::<str>::from(secret),
            source,
        }
    }

    #[must_use]
    pub fn secret(&self) -> &str {
        &self.secret
    }

    #[must_use]
    pub const fn source(&self) -> &CredentialSource {
        &self.source
    }
}

impl fmt::Debug for ResolvedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedCredential")
            .field("secret", &"[REDACTED]")
            .field("source", &self.source)
            .finish()
    }
}

impl fmt::Display for ResolvedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED]")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialSource {
    Environment { variable: String },
}

pub trait CredentialResolver: Send + Sync {
    fn resolve(&self, auth: &ProviderAuthConfig) -> Result<ResolvedCredential, HarnessError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EnvironmentCredentialResolver;

impl CredentialResolver for EnvironmentCredentialResolver {
    fn resolve(&self, auth: &ProviderAuthConfig) -> Result<ResolvedCredential, HarnessError> {
        let value = std::env::var(&auth.api_key_env).map_err(|_| {
            HarnessError::Provider(ProviderError::AuthFailed {
                provider: auth.provider_id.clone(),
            })
        })?;

        Ok(ResolvedCredential::new(
            value,
            CredentialSource::Environment {
                variable: auth.api_key_env.clone(),
            },
        ))
    }
}

pub fn provider_auth_config(
    config: &Value,
    provider_id: &str,
    default_env: &str,
) -> Result<ProviderAuthConfig, HarnessError> {
    if config.get("api_key").is_some() {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: format!("providers.{provider_id}.api_key"),
            reason: "inline secrets are not supported; use api_key_env instead".to_string(),
        }));
    }

    let api_key_env = config
        .get("api_key_env")
        .and_then(Value::as_str)
        .unwrap_or(default_env)
        .to_string();
    let auth_ref = config
        .get("auth_ref")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(ProviderAuthConfig {
        provider_id: provider_id.to_string(),
        api_key_env,
        auth_ref,
    })
}
