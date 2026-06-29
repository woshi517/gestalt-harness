use gestalt_core::{ConfigError, HarnessError, ProviderError};
use gestalt_runtime::auth::{
    ChainCredentialResolver, CredentialResolver, CredentialSource, EnvironmentCredentialResolver,
    ProviderAuthConfig, ResolvedCredential,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::{
    config::EffectiveConfig,
    reports::{AuthDoctorEntry, AuthDoctorReport, AuthResolveReport},
};

static KEYCHAIN_FAKE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static USE_FAKE_KEYCHAIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn get_file_fake(account: &str) -> Option<String> {
    if let Ok(config_dir) = std::env::var("XDG_CONFIG_HOME") {
        let path = std::path::PathBuf::from(config_dir).join("gestalt/fake_keychain.json");
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&content) {
                    return map.get(account).cloned();
                }
            }
        }
    }
    None
}

fn set_file_fake(account: &str, secret: &str) {
    if let Ok(config_dir) = std::env::var("XDG_CONFIG_HOME") {
        let path = std::path::PathBuf::from(config_dir).join("gestalt/fake_keychain.json");
        let mut map = HashMap::new();
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(m) = serde_json::from_str::<HashMap<String, String>>(&content) {
                    map = m;
                }
            }
        }
        map.insert(account.to_string(), secret.to_string());
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(content) = serde_json::to_string(&map) {
            let _ = std::fs::write(&path, content);
        }
    }
}

fn delete_file_fake(account: &str) {
    if let Ok(config_dir) = std::env::var("XDG_CONFIG_HOME") {
        let path = std::path::PathBuf::from(config_dir).join("gestalt/fake_keychain.json");
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(mut map) = serde_json::from_str::<HashMap<String, String>>(&content) {
                    map.remove(account);
                    if let Ok(content) = serde_json::to_string(&map) {
                        let _ = std::fs::write(&path, content);
                    }
                }
            }
        }
    }
}

pub fn set_use_fake_keychain(use_fake: bool) {
    USE_FAKE_KEYCHAIN.store(use_fake, std::sync::atomic::Ordering::SeqCst);
}

pub fn get_keychain_secret(account: &str) -> Result<String, String> {
    if USE_FAKE_KEYCHAIN.load(std::sync::atomic::Ordering::SeqCst)
        || std::env::var("GESTALT_USE_FAKE_KEYCHAIN").is_ok()
    {
        if let Some(secret) = get_file_fake(account) {
            return Ok(secret);
        }
        let fake = KEYCHAIN_FAKE.get_or_init(|| Mutex::new(HashMap::new()));
        let map = fake.lock().unwrap();
        if let Some(secret) = map.get(account) {
            Ok(secret.clone())
        } else {
            Err("Fake keychain entry not found".to_string())
        }
    } else {
        #[cfg(not(test))]
        {
            let entry =
                keyring::Entry::new("gestalt-harness", account).map_err(|err| err.to_string())?;
            entry.get_password().map_err(|err| err.to_string())
        }
        #[cfg(test)]
        {
            if let Some(secret) = get_file_fake(account) {
                return Ok(secret);
            }
            let fake = KEYCHAIN_FAKE.get_or_init(|| Mutex::new(HashMap::new()));
            let map = fake.lock().unwrap();
            if let Some(secret) = map.get(account) {
                Ok(secret.clone())
            } else {
                Err("Real keychain disabled in tests".to_string())
            }
        }
    }
}

pub fn set_keychain_secret(account: &str, secret: &str) -> Result<(), String> {
    if USE_FAKE_KEYCHAIN.load(std::sync::atomic::Ordering::SeqCst)
        || std::env::var("GESTALT_USE_FAKE_KEYCHAIN").is_ok()
    {
        set_file_fake(account, secret);
        let fake = KEYCHAIN_FAKE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut map = fake.lock().unwrap();
        map.insert(account.to_string(), secret.to_string());
        Ok(())
    } else {
        #[cfg(not(test))]
        {
            let entry =
                keyring::Entry::new("gestalt-harness", account).map_err(|err| err.to_string())?;
            entry.set_password(secret).map_err(|err| err.to_string())
        }
        #[cfg(test)]
        {
            set_file_fake(account, secret);
            let fake = KEYCHAIN_FAKE.get_or_init(|| Mutex::new(HashMap::new()));
            let mut map = fake.lock().unwrap();
            map.insert(account.to_string(), secret.to_string());
            Ok(())
        }
    }
}

pub fn delete_keychain_secret(account: &str) -> Result<(), String> {
    if USE_FAKE_KEYCHAIN.load(std::sync::atomic::Ordering::SeqCst)
        || std::env::var("GESTALT_USE_FAKE_KEYCHAIN").is_ok()
    {
        delete_file_fake(account);
        let fake = KEYCHAIN_FAKE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut map = fake.lock().unwrap();
        map.remove(account);
        Ok(())
    } else {
        #[cfg(not(test))]
        {
            let entry =
                keyring::Entry::new("gestalt-harness", account).map_err(|err| err.to_string())?;
            match entry.delete_password() {
                Ok(_) => Ok(()),
                Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(err.to_string()),
            }
        }
        #[cfg(test)]
        {
            delete_file_fake(account);
            let fake = KEYCHAIN_FAKE.get_or_init(|| Mutex::new(HashMap::new()));
            let mut map = fake.lock().unwrap();
            map.remove(account);
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionCredentialResolver {
    pub api_key: Option<String>,
}

impl CredentialResolver for SessionCredentialResolver {
    fn resolve(&self, auth: &ProviderAuthConfig) -> Result<ResolvedCredential, HarnessError> {
        if let Some(ref key) = self.api_key {
            return Ok(ResolvedCredential::new(
                key.clone(),
                CredentialSource::Session,
            ));
        }
        Err(HarnessError::Provider(ProviderError::AuthFailed {
            provider: auth.provider_id.clone(),
        }))
    }
}

#[derive(Debug, Clone)]
pub struct KeychainCredentialResolver;

impl CredentialResolver for KeychainCredentialResolver {
    fn resolve(&self, auth: &ProviderAuthConfig) -> Result<ResolvedCredential, HarnessError> {
        if let gestalt_runtime::auth::ConfiguredCredential::Keychain(ref account) = auth.credential
        {
            if let Ok(password) = get_keychain_secret(account) {
                return Ok(ResolvedCredential::new(
                    password,
                    CredentialSource::Keychain {
                        account: account.clone(),
                    },
                ));
            }
        }
        Err(HarnessError::Provider(ProviderError::AuthFailed {
            provider: auth.provider_id.clone(),
        }))
    }
}

#[derive(Clone)]
pub struct PromptCredentialResolver {
    pub interaction: Arc<dyn crate::InteractionProvider>,
}

impl std::fmt::Debug for PromptCredentialResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptCredentialResolver").finish()
    }
}

impl CredentialResolver for PromptCredentialResolver {
    fn resolve(&self, auth: &ProviderAuthConfig) -> Result<ResolvedCredential, HarnessError> {
        let prompt = format!("Enter API key for provider '{}':", auth.provider_id);
        if let Some(key) = self.interaction.prompt_password(&prompt) {
            return Ok(ResolvedCredential::new(key, CredentialSource::Session));
        }
        Err(HarnessError::Provider(ProviderError::AuthFailed {
            provider: auth.provider_id.clone(),
        }))
    }
}

pub fn build_credential_resolver(
    api_key_override: Option<String>,
    interaction: Option<Arc<dyn crate::InteractionProvider>>,
) -> Arc<dyn CredentialResolver> {
    let mut resolvers: Vec<Arc<dyn CredentialResolver>> = Vec::new();
    if let Some(key) = api_key_override {
        resolvers.push(Arc::new(SessionCredentialResolver { api_key: Some(key) }));
    }
    resolvers.push(Arc::new(gestalt_runtime::auth::InlineCredentialResolver));
    resolvers.push(Arc::new(EnvironmentCredentialResolver));
    resolvers.push(Arc::new(KeychainCredentialResolver));
    if let Some(inter) = interaction {
        resolvers.push(Arc::new(PromptCredentialResolver { interaction: inter }));
    }
    Arc::new(ChainCredentialResolver::new(resolvers))
}

pub fn resolve_auth(
    config: &EffectiveConfig,
    provider: &str,
) -> Result<AuthResolveReport, HarnessError> {
    // We check if provider matches a resolved provider name or if it's one of the configured providers
    // If provider is a legacy/direct provider, config.resolve_provider() will handle it

    // Create a temporary config with the targeted provider
    let resolved = if let Ok(c) = config.resolve_provider() {
        if c.provider_name() == provider {
            c
        } else {
            // resolve with provider override
            let mut temp_cfg = config.clone();
            temp_cfg.defaults.provider = Some(provider.to_string());
            temp_cfg.defaults.profile = None; // clear profile to force provider override
            temp_cfg.resolve_provider()?
        }
    } else {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: "provider".to_string(),
            reason: format!("unknown provider: {provider}"),
        }));
    };

    let auth_config = resolved.auth.clone();
    let cred_resolver = build_credential_resolver(None, None);

    let (source, status, variable) = if let Ok(cred) = cred_resolver.resolve(&auth_config) {
        let src = match cred.source() {
            CredentialSource::Session => "session".to_string(),
            CredentialSource::Environment { variable } => format!("env ({variable})"),
            CredentialSource::Keychain { account } => format!("keychain ({account})"),
            CredentialSource::Inline => "inline".to_string(),
        };
        let var = match &auth_config.credential {
            gestalt_runtime::auth::ConfiguredCredential::Environment(v) => v.clone(),
            _ => String::new(),
        };
        (src, "present".to_string(), var)
    } else {
        let var = match &auth_config.credential {
            gestalt_runtime::auth::ConfiguredCredential::Environment(v) => v.clone(),
            _ => String::new(),
        };
        ("missing".to_string(), "missing".to_string(), var)
    };

    Ok(AuthResolveReport {
        provider: provider.to_string(),
        source,
        variable,
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
            if var_name.is_empty() || checked_vars.insert(var_name.clone()) {
                let status = auth_report.status.clone();
                let source = auth_report.source.clone();
                let value = if status == "present" {
                    format!("[PRESENT] via {source}")
                } else {
                    "[NOT SET]".to_string()
                };
                let display_var = if var_name.is_empty() {
                    format!("({})", auth_report.provider)
                } else {
                    var_name
                };
                entries.push(AuthDoctorEntry {
                    variable: display_var,
                    status,
                    value,
                });
            }
        }
    }

    Ok(AuthDoctorReport { entries })
}
