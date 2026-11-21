#![forbid(unsafe_code)]
//! Composable host-only `CredentialStore` supporting API keys, OAuth tokens,
//! custom header sets, extensible `Custom` schemes, and pluggable
//! `CredentialResolver`s with Unix `0600` file enforcement and stream redaction.

pub mod redactor;

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::RuntimeError;
use redactor::MIN_REDACT_LEN;
pub use redactor::{REDACTED_MARKER, StreamRedactor};

/// Extensible credential payload stored in [`CredentialStore`] or returned by a
/// [`CredentialResolver`].
///
/// Supports flat API keys, OAuth 2.0 tokens with refresh/expiry metadata,
/// arbitrary header maps, and provider-defined `Custom` schemes without
/// modifying `gestalt-runtime`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialRecord {
    /// Static API key or bearer secret.
    ApiKey {
        /// Secret key material.
        key: String,
    },
    /// OAuth 2.0 / PKCE token bundle.
    OAuth {
        /// Active access token.
        access_token: String,
        /// Optional refresh token for renewal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        /// Optional expiration timestamp in milliseconds since Unix epoch.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at_ms: Option<u64>,
        /// Optional token type (defaults to `"Bearer"`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_type: Option<String>,
        /// Provider-specific OAuth metadata (account ID, scopes, tenant, etc.).
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        extra: BTreeMap<String, Value>,
    },
    /// Pre-formatted HTTP authentication headers (e.g., custom gateway headers).
    Headers {
        /// Header name -> value map.
        headers: BTreeMap<String, String>,
    },
    /// Extensible provider-defined credential scheme (e.g., cloud SigV4, mTLS,
    /// service account JSON) declaring which `fields` contain secrets to redact.
    Custom {
        /// Scheme identifier (e.g., `"sigv4"`, `"mtls"`, `"service_account"`).
        scheme: String,
        /// Arbitrary structured credential fields.
        #[serde(default)]
        fields: BTreeMap<String, Value>,
        /// Keys inside `fields` whose string values MUST be scrubbed by [`StreamRedactor`].
        #[serde(default)]
        secret_fields: Vec<String>,
    },
}

impl CredentialRecord {
    /// Return the primary secret string for API-key or OAuth access-token flows.
    #[must_use]
    pub fn primary_secret(&self) -> Option<&str> {
        match self {
            Self::ApiKey { key } if !key.is_empty() => Some(key.as_str()),
            Self::OAuth { access_token, .. } if !access_token.is_empty() => {
                Some(access_token.as_str())
            }
            Self::ApiKey { .. }
            | Self::OAuth { .. }
            | Self::Headers { .. }
            | Self::Custom { .. } => None,
        }
    }

    /// Return `true` if this is an OAuth record whose `expires_at_ms` has passed
    /// (within `skew_ms` safety margin).
    #[must_use]
    pub fn is_expired(&self, now_ms: u64, skew_ms: u64) -> bool {
        match self {
            Self::OAuth {
                expires_at_ms: Some(exp),
                ..
            } => now_ms.saturating_add(skew_ms) >= *exp,
            _ => false,
        }
    }

    /// Build HTTP auth headers for standard API-key, OAuth, or header-map records.
    #[must_use]
    pub fn to_auth_headers(
        &self,
        api_key_header: &str,
        bearer_prefix_for_api_key: bool,
    ) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        match self {
            Self::ApiKey { key } => {
                let val = if bearer_prefix_for_api_key {
                    format!("Bearer {key}")
                } else {
                    key.clone()
                };
                out.insert(api_key_header.to_string(), val);
            }
            Self::OAuth {
                access_token,
                token_type,
                ..
            } => {
                let scheme = token_type.as_deref().unwrap_or("Bearer");
                out.insert(
                    "Authorization".to_string(),
                    format!("{scheme} {access_token}"),
                );
            }
            Self::Headers { headers } => {
                out.extend(headers.clone());
            }
            Self::Custom { .. } => {}
        }
        out
    }

    /// Extract all secret string values from this record for [`StreamRedactor`].
    #[must_use]
    pub fn secret_values(&self) -> Vec<String> {
        let mut out = Vec::new();
        match self {
            Self::ApiKey { key } => {
                if key.len() >= MIN_REDACT_LEN {
                    out.push(key.clone());
                }
            }
            Self::OAuth {
                access_token,
                refresh_token,
                ..
            } => {
                if access_token.len() >= MIN_REDACT_LEN {
                    out.push(access_token.clone());
                }
                if let Some(rt) = refresh_token
                    && rt.len() >= MIN_REDACT_LEN
                {
                    out.push(rt.clone());
                }
            }
            Self::Headers { headers } => {
                for val in headers.values() {
                    if val.len() >= MIN_REDACT_LEN {
                        out.push(val.clone());
                    }
                    if let Some(stripped) = val.strip_prefix("Bearer ")
                        && stripped.len() >= MIN_REDACT_LEN
                    {
                        out.push(stripped.to_string());
                    }
                }
            }
            Self::Custom {
                fields,
                secret_fields,
                ..
            } => {
                for key in secret_fields {
                    if let Some(val) = fields.get(key).and_then(Value::as_str)
                        && val.len() >= MIN_REDACT_LEN
                    {
                        out.push(val.to_string());
                    }
                }
            }
        }
        out
    }
}

/// Backward-compatible TOML entry allowing either a flat string
/// (`prov = "sk-..."`) or a structured [`CredentialRecord`] table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum StoredCredentialEntry {
    Structured(CredentialRecord),
    PlainApiKey(String),
}

impl From<StoredCredentialEntry> for CredentialRecord {
    fn from(entry: StoredCredentialEntry) -> Self {
        match entry {
            StoredCredentialEntry::Structured(rec) => rec,
            StoredCredentialEntry::PlainApiKey(key) => Self::ApiKey { key },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CredentialsFile {
    #[serde(default)]
    credentials: BTreeMap<String, StoredCredentialEntry>,
}

/// Lookup request passed to [`CredentialStore::resolve`] and custom
/// [`CredentialResolver`] implementations.
#[derive(Debug, Clone, Default)]
pub struct CredentialRequest {
    /// Provider or alias identifier.
    pub provider: String,
    /// Optional explicit environment variable override from `gestalt.toml`.
    pub env_key_override: Option<String>,
    /// Optional provider-specific hints (e.g., OAuth profile, cloud region, scopes).
    pub hints: BTreeMap<String, Value>,
}

impl CredentialRequest {
    /// Construct a [`CredentialRequest`] for `provider`.
    #[must_use]
    pub fn new(provider: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            env_key_override: None,
            hints: BTreeMap::new(),
        }
    }

    /// Attach an explicit environment variable name override.
    #[must_use]
    pub fn with_env_key(mut self, env_key: Option<String>) -> Self {
        self.env_key_override = env_key;
        self
    }
}

/// Pluggable async credential resolver (e.g., OAuth token refresher, OS
/// keychain, cloud STS, or custom auth broker) composable inside [`CredentialStore`].
pub trait CredentialResolver: Send + Sync {
    /// Unique resolver identifier.
    fn id(&self) -> &str;

    /// Attempt to resolve a [`CredentialRecord`] for `req`. Returning `Ok(None)`
    /// delegates to the next resolver in the chain.
    fn resolve<'a>(
        &'a self,
        req: &'a CredentialRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Option<CredentialRecord>, RuntimeError>> + Send + 'a>>;

    /// Return any known secret strings held by this resolver for [`StreamRedactor`].
    fn known_secrets(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Host-side composable credential store resolving secrets from custom
/// [`CredentialResolver`]s, runtime overrides, environment variables, or
/// `~/.config/gestalt/credentials.toml` (enforcing `0600` permissions on Unix).
///
/// Secrets stored here MUST never be serialized into `SessionTree`, `EventStream`,
/// `tracing` logs, or sandboxed child process environments.
#[derive(Clone, Default)]
pub struct CredentialStore {
    file_path: Option<PathBuf>,
    file_records: BTreeMap<String, CredentialRecord>,
    runtime_overrides: BTreeMap<String, CredentialRecord>,
    resolvers: Vec<Arc<dyn CredentialResolver>>,
}

impl std::fmt::Debug for CredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialStore")
            .field("file_path", &self.file_path)
            .field(
                "file_record_keys",
                &self.file_records.keys().collect::<Vec<_>>(),
            )
            .field(
                "runtime_override_keys",
                &self.runtime_overrides.keys().collect::<Vec<_>>(),
            )
            .field("resolver_count", &self.resolvers.len())
            .finish()
    }
}

impl CredentialStore {
    /// Resolve the user's Gestalt configuration directory, checking `XDG_CONFIG_HOME`
    /// first across all platforms before falling back to `$HOME/.config/gestalt`.
    #[must_use]
    pub fn resolve_config_dir() -> Option<PathBuf> {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.is_empty()
        {
            return Some(PathBuf::from(xdg).join("gestalt"));
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.is_empty()
        {
            return Some(PathBuf::from(home).join(".config").join("gestalt"));
        }
        if let Ok(profile) = std::env::var("USERPROFILE")
            && !profile.is_empty()
        {
            return Some(PathBuf::from(profile).join(".config").join("gestalt"));
        }
        None
    }

    /// Load credentials from `~/.config/gestalt/credentials.toml`, if present.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InsecureCredentialPermissions`] if the file exists
    /// with group/world permissions on Unix, or [`RuntimeError::TomlDe`] on invalid TOML.
    pub fn load_default() -> Result<Self, RuntimeError> {
        let path = Self::resolve_config_dir().map(|d| d.join("credentials.toml"));
        match path {
            Some(p) => Self::load_from_path(p),
            None => Ok(Self::default()),
        }
    }

    /// Load credentials from an explicit `credentials.toml` path.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InsecureCredentialPermissions`] if Unix permissions
    /// allow group or other access (`mode & 0o077 != 0`).
    pub fn load_from_path(path: PathBuf) -> Result<Self, RuntimeError> {
        if !path.exists() {
            return Ok(Self {
                file_path: Some(path),
                file_records: BTreeMap::new(),
                runtime_overrides: BTreeMap::new(),
                resolvers: Vec::new(),
            });
        }

        enforce_0600_permissions(&path)?;
        let raw = fs::read_to_string(&path)?;
        let parsed: CredentialsFile = toml::from_str(&raw)?;
        let file_records = parsed
            .credentials
            .into_iter()
            .map(|(k, v)| (k, CredentialRecord::from(v)))
            .collect();
        Ok(Self {
            file_path: Some(path),
            file_records,
            runtime_overrides: BTreeMap::new(),
            resolvers: Vec::new(),
        })
    }

    /// Construct an in-memory [`CredentialStore`] from flat API keys.
    #[must_use]
    pub fn from_in_memory(secrets: BTreeMap<String, String>) -> Self {
        let file_records = secrets
            .into_iter()
            .map(|(k, key)| (k, CredentialRecord::ApiKey { key }))
            .collect();
        Self {
            file_path: None,
            file_records,
            runtime_overrides: BTreeMap::new(),
            resolvers: Vec::new(),
        }
    }

    /// Register a custom [`CredentialResolver`] (evaluated before default env/file
    /// fallbacks in registration order).
    pub fn register_resolver(&mut self, resolver: Arc<dyn CredentialResolver>) {
        self.resolvers.push(resolver);
    }

    /// Register an explicit in-memory override secret string for `provider`.
    pub fn insert_runtime_secret(
        &mut self,
        provider: impl Into<String>,
        secret: impl Into<String>,
    ) {
        self.runtime_overrides.insert(
            provider.into(),
            CredentialRecord::ApiKey { key: secret.into() },
        );
    }

    /// Register an explicit in-memory [`CredentialRecord`] override for `provider`.
    pub fn insert_runtime_record(&mut self, provider: impl Into<String>, record: CredentialRecord) {
        self.runtime_overrides.insert(provider.into(), record);
    }

    /// Asynchronously resolve a [`CredentialRecord`] for `req`, checking:
    /// 1. Custom registered [`CredentialResolver`]s in order
    /// 2. Explicit in-memory runtime overrides
    /// 3. Environment variables (`env_key_override`, then `<PROVIDER_UPPER>_API_KEY`)
    /// 4. `~/.config/gestalt/credentials.toml`
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if a custom resolver fails.
    pub async fn resolve(
        &self,
        req: &CredentialRequest,
    ) -> Result<Option<CredentialRecord>, RuntimeError> {
        for resolver in &self.resolvers {
            if let Some(record) = resolver.resolve(req).await? {
                return Ok(Some(record));
            }
        }
        Ok(self.get_record(&req.provider, req.env_key_override.as_deref()))
    }

    /// Synchronously look up a [`CredentialRecord`] from runtime overrides, env
    /// vars, or `credentials.toml`.
    #[must_use]
    pub fn get_record(
        &self,
        provider: &str,
        env_key_override: Option<&str>,
    ) -> Option<CredentialRecord> {
        if let Some(rec) = self.runtime_overrides.get(provider) {
            return Some(rec.clone());
        }
        if let Some(env_name) = env_key_override
            && let Ok(val) = std::env::var(env_name)
            && !val.is_empty()
        {
            return Some(CredentialRecord::ApiKey { key: val });
        }
        let default_env = format!("{}_API_KEY", provider.to_ascii_uppercase());
        if let Ok(val) = std::env::var(&default_env)
            && !val.is_empty()
        {
            return Some(CredentialRecord::ApiKey { key: val });
        }
        self.file_records.get(provider).cloned()
    }

    /// Convenience helper returning the primary API key / access token string
    /// for `provider`.
    #[must_use]
    pub fn get(&self, provider: &str, env_key_override: Option<&str>) -> Option<String> {
        self.get_record(provider, env_key_override)
            .and_then(|rec| rec.primary_secret().map(str::to_string))
    }

    /// Persist a structured [`CredentialRecord`] for `provider` into
    /// `credentials.toml` with `0600` permissions.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if serialization or private file write fails.
    pub fn set_record(
        &mut self,
        provider: impl Into<String>,
        record: CredentialRecord,
    ) -> Result<(), RuntimeError> {
        self.file_records.insert(provider.into(), record);
        self.persist_file()
    }

    /// Persist a plain API key for `provider` into `credentials.toml` with
    /// `0600` permissions.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if serialization or private file write fails.
    pub fn set_credential(
        &mut self,
        provider: impl Into<String>,
        secret: String,
    ) -> Result<(), RuntimeError> {
        self.set_record(provider, CredentialRecord::ApiKey { key: secret })
    }

    /// Remove stored credentials for `provider` from memory and `credentials.toml`.
    ///
    /// Returns `true` if a credential record or runtime override existed and was removed.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if persisting the updated `credentials.toml` fails.
    pub fn remove_credential(&mut self, provider: &str) -> Result<bool, RuntimeError> {
        let removed_file = self.file_records.remove(provider).is_some();
        let removed_override = self.runtime_overrides.remove(provider).is_some();
        if removed_file {
            self.persist_file()?;
        }
        Ok(removed_file || removed_override)
    }

    /// Return all known non-empty secret strings across stored records,
    /// registered resolvers, and host secret env vars for redaction.
    #[must_use]
    pub fn known_secrets(&self) -> Vec<String> {
        let mut secrets = Vec::new();
        for rec in self
            .file_records
            .values()
            .chain(self.runtime_overrides.values())
        {
            secrets.extend(rec.secret_values());
        }
        for resolver in &self.resolvers {
            secrets.extend(
                resolver
                    .known_secrets()
                    .into_iter()
                    .filter(|s| s.len() >= MIN_REDACT_LEN),
            );
        }
        for (k, v) in std::env::vars() {
            if (k.ends_with("_API_KEY") || k.ends_with("_TOKEN") || k.ends_with("_SECRET"))
                && v.len() >= MIN_REDACT_LEN
            {
                secrets.push(v);
            }
        }
        secrets.sort();
        secrets.dedup();
        secrets
    }

    /// Build a [`StreamRedactor`] initialized with all currently known secrets.
    #[must_use]
    pub fn redactor(&self) -> StreamRedactor {
        StreamRedactor::from_secrets(self.known_secrets())
    }

    fn persist_file(&self) -> Result<(), RuntimeError> {
        let Some(ref path) = self.file_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let entries = self
            .file_records
            .iter()
            .map(|(k, rec)| (k.clone(), StoredCredentialEntry::Structured(rec.clone())))
            .collect();
        let payload = CredentialsFile {
            credentials: entries,
        };
        let encoded = toml::to_string_pretty(&payload)?;
        write_private_file(path, encoded.as_bytes())?;
        Ok(())
    }
}

#[cfg(unix)]
fn enforce_0600_permissions(path: &Path) -> Result<(), RuntimeError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path)?;
    let mode = meta.permissions().mode() & 0o777;
    if (mode & 0o077) != 0 {
        return Err(RuntimeError::InsecureCredentialPermissions {
            path: path.display().to_string(),
            mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn enforce_0600_permissions(_path: &Path) -> Result<(), RuntimeError> {
    Ok(())
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct CustomOAuthResolver;
    impl CredentialResolver for CustomOAuthResolver {
        fn id(&self) -> &str {
            "custom_oauth"
        }
        fn resolve<'a>(
            &'a self,
            req: &'a CredentialRequest,
        ) -> Pin<Box<dyn Future<Output = Result<Option<CredentialRecord>, RuntimeError>> + Send + 'a>>
        {
            let provider = req.provider.clone();
            Box::pin(async move {
                if provider == "oauth_prov" {
                    Ok(Some(CredentialRecord::OAuth {
                        access_token: "oa_live_token_999".to_string(),
                        refresh_token: Some("oa_refresh_888".to_string()),
                        expires_at_ms: Some(9_999_999_999),
                        token_type: Some("Bearer".to_string()),
                        extra: BTreeMap::new(),
                    }))
                } else {
                    Ok(None)
                }
            })
        }
        fn known_secrets(&self) -> Vec<String> {
            vec!["oa_live_token_999".to_string()]
        }
    }

    #[tokio::test]
    async fn composable_credential_records_and_custom_resolvers() {
        let dir = std::env::temp_dir().join(format!("gestalt_cred_comp_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let cred_path = dir.join("credentials.toml");

        let mut store = CredentialStore::load_from_path(cred_path.clone()).expect("empty store");
        store.register_resolver(Arc::new(CustomOAuthResolver));

        // Persist a Custom scheme credential and reload from disk
        let mut fields = BTreeMap::new();
        fields.insert("region".to_string(), json!("us-east-1"));
        fields.insert("secret_key".to_string(), json!("cloud_secret_7777"));
        store
            .set_record(
                "cloud_prov",
                CredentialRecord::Custom {
                    scheme: "sigv4".to_string(),
                    fields,
                    secret_fields: vec!["secret_key".to_string()],
                },
            )
            .expect("save custom record");

        let mut reloaded = CredentialStore::load_from_path(cred_path).expect("reload");
        reloaded.register_resolver(Arc::new(CustomOAuthResolver));

        let oauth = reloaded
            .resolve(&CredentialRequest::new("oauth_prov"))
            .await
            .expect("resolve oauth")
            .expect("some oauth");
        assert_eq!(oauth.primary_secret(), Some("oa_live_token_999"));
        assert_eq!(
            oauth
                .to_auth_headers("x-api-key", false)
                .get("Authorization")
                .map(String::as_str),
            Some("Bearer oa_live_token_999")
        );

        let secrets = reloaded.known_secrets();
        assert!(secrets.contains(&"cloud_secret_7777".to_string()));
        assert!(secrets.contains(&"oa_live_token_999".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn credential_store_rejects_world_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("gestalt_cred_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let cred_path = dir.join("credentials.toml");
        fs::write(&cred_path, "[credentials]\nprov = \"secret-value\"\n").expect("write");
        fs::set_permissions(&cred_path, fs::Permissions::from_mode(0o644)).expect("chmod");

        let res = CredentialStore::load_from_path(cred_path.clone());
        assert!(matches!(
            res,
            Err(RuntimeError::InsecureCredentialPermissions { mode: 0o644, .. })
        ));

        fs::set_permissions(&cred_path, fs::Permissions::from_mode(0o600)).expect("chmod 600");
        let store = CredentialStore::load_from_path(cred_path).expect("load 0600");
        assert_eq!(store.get("prov", None).as_deref(), Some("secret-value"));

        let _ = fs::remove_dir_all(&dir);
    }
}
