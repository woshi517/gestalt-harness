use std::{fmt, sync::Arc};

use gestalt_core::{HarnessError, ProviderError};
use serde_json::Value;

#[derive(Clone, PartialEq, Eq)]
pub enum ConfiguredCredential {
    None,
    Environment(String),
    Keychain(String),
    Inline(Arc<str>),
}

impl fmt::Debug for ConfiguredCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Environment(v) => write!(f, "Environment({})", v),
            Self::Keychain(v) => write!(f, "Keychain({})", v),
            Self::Inline(_) => write!(f, "Inline([REDACTED])"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAuthConfig {
    pub provider_id: String,
    pub credential: ConfiguredCredential,
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
    Session,
    Environment { variable: String },
    Keychain { account: String },
    Inline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialRef {
    Session,
    Environment(String),
    Keychain(String),
    Inline(Arc<str>),
}

impl ProviderAuthConfig {
    pub fn credential_ref(&self) -> CredentialRef {
        match &self.credential {
            ConfiguredCredential::None => CredentialRef::Session,
            ConfiguredCredential::Environment(var) => CredentialRef::Environment(var.clone()),
            ConfiguredCredential::Keychain(acc) => CredentialRef::Keychain(acc.clone()),
            ConfiguredCredential::Inline(val) => CredentialRef::Inline(val.clone()),
        }
    }
}

pub trait CredentialResolver: Send + Sync {
    fn resolve(&self, auth: &ProviderAuthConfig) -> Result<ResolvedCredential, HarnessError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EnvironmentCredentialResolver;

impl CredentialResolver for EnvironmentCredentialResolver {
    fn resolve(&self, auth: &ProviderAuthConfig) -> Result<ResolvedCredential, HarnessError> {
        if let ConfiguredCredential::Environment(ref var) = auth.credential {
            let value = std::env::var(var).map_err(|_| {
                HarnessError::Provider(ProviderError::AuthFailed {
                    provider: auth.provider_id.clone(),
                })
            })?;

            Ok(ResolvedCredential::new(
                value,
                CredentialSource::Environment {
                    variable: var.clone(),
                },
            ))
        } else {
            Err(HarnessError::Provider(ProviderError::AuthFailed {
                provider: auth.provider_id.clone(),
            }))
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct InlineCredentialResolver;

impl CredentialResolver for InlineCredentialResolver {
    fn resolve(&self, auth: &ProviderAuthConfig) -> Result<ResolvedCredential, HarnessError> {
        if let ConfiguredCredential::Inline(ref val) = auth.credential {
            Ok(ResolvedCredential::new(
                val.to_string(),
                CredentialSource::Inline,
            ))
        } else {
            Err(HarnessError::Provider(ProviderError::AuthFailed {
                provider: auth.provider_id.clone(),
            }))
        }
    }
}

#[derive(Clone)]
pub struct ChainCredentialResolver {
    resolvers: Vec<Arc<dyn CredentialResolver>>,
}

impl ChainCredentialResolver {
    pub fn new(resolvers: Vec<Arc<dyn CredentialResolver>>) -> Self {
        Self { resolvers }
    }
}

impl CredentialResolver for ChainCredentialResolver {
    fn resolve(&self, auth: &ProviderAuthConfig) -> Result<ResolvedCredential, HarnessError> {
        let mut last_err = None;
        for resolver in &self.resolvers {
            match resolver.resolve(auth) {
                Ok(cred) => return Ok(cred),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            HarnessError::Provider(ProviderError::AuthFailed {
                provider: auth.provider_id.clone(),
            })
        }))
    }
}

pub fn provider_auth_config(
    config: &Value,
    provider_id: &str,
    default_env: &str,
) -> Result<ProviderAuthConfig, HarnessError> {
    let credential = if let Some(api_key_val) = config.get("api_key").and_then(Value::as_str) {
        if let Some(env_var) = api_key_val.strip_prefix('$') {
            ConfiguredCredential::Environment(env_var.to_string())
        } else {
            ConfiguredCredential::Inline(Arc::from(api_key_val))
        }
    } else if let Some(api_key_env) = config.get("api_key_env").and_then(Value::as_str) {
        ConfiguredCredential::Environment(api_key_env.to_string())
    } else if let Some(auth_ref) = config.get("auth_ref").and_then(Value::as_str) {
        if let Some(key) = auth_ref.strip_prefix("keychain:") {
            ConfiguredCredential::Keychain(key.to_string())
        } else if let Some(key) = auth_ref.strip_prefix("secret:") {
            ConfiguredCredential::Keychain(key.to_string())
        } else {
            ConfiguredCredential::Keychain(auth_ref.to_string())
        }
    } else {
        ConfiguredCredential::Environment(default_env.to_string())
    };

    Ok(ProviderAuthConfig {
        provider_id: provider_id.to_string(),
        credential,
    })
}
