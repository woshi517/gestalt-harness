use std::{collections::HashMap, sync::Arc};

use gestalt_core::{HarnessError, ProviderError};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::Value;

use crate::{
    auth::{CredentialResolver, ProviderAuthConfig},
    sse,
};

#[derive(Clone)]
pub struct CompletionsTransport {
    pub client: reqwest::Client,
    pub base_url: String,
    pub auth: ProviderAuthConfig,
    pub resolver: Arc<dyn CredentialResolver>,
    pub headers: HashMap<String, String>,
}

impl CompletionsTransport {
    pub fn new(
        base_url: String,
        auth: ProviderAuthConfig,
        resolver: Arc<dyn CredentialResolver>,
        headers: HashMap<String, String>,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            auth,
            resolver,
            headers,
        }
    }

    pub fn build_headers(&self) -> Result<HeaderMap, HarnessError> {
        let mut headers = HeaderMap::new();
        let credential_ref = self.auth.credential_ref();
        let has_auth = !matches!(credential_ref, crate::auth::CredentialRef::Session)
            || matches!(
                &self.auth.credential,
                crate::auth::ConfiguredCredential::Inline(_)
            );

        if has_auth {
            let credential = self.resolver.resolve(&self.auth)?;
            headers.insert(
                "authorization",
                HeaderValue::from_str(&format!("Bearer {}", credential.secret()))
                    .map_err(invalid)?,
            );
        }
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        for (k, v) in &self.headers {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes()).map_err(invalid)?;
            let value = HeaderValue::from_str(v).map_err(invalid)?;
            headers.insert(name, value);
        }
        Ok(headers)
    }
}

pub fn map_error(value: &Value, provider: &str) -> ProviderError {
    let error = value.get("error").unwrap_or(value);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("provider error");
    let lowered = message.to_ascii_lowercase();
    let sanitized = sse::sanitize_detail(message);

    if matches!(
        error.get("code").and_then(Value::as_str),
        Some("invalid_api_key")
    ) {
        return ProviderError::AuthFailed {
            provider: provider.to_string(),
        };
    }

    match error.get("type").and_then(Value::as_str) {
        Some("insufficient_quota") => ProviderError::RateLimit {
            retry_after_secs: None,
        },
        Some("invalid_request_error") if lowered.contains("context") => {
            ProviderError::ContextTooLong {
                tokens: 0,
                limit: 0,
            }
        }
        Some("invalid_request_error") if lowered.contains("model") => {
            ProviderError::InvalidModel { model: sanitized }
        }
        _ if lowered.contains("timed out") => ProviderError::Timeout,
        _ => ProviderError::UnexpectedResponse { details: sanitized },
    }
}

pub fn invalid(err: impl std::fmt::Display) -> HarnessError {
    HarnessError::Provider(ProviderError::UnexpectedResponse {
        details: sse::sanitize_detail(&err.to_string()),
    })
}

pub fn poisoned() -> HarnessError {
    HarnessError::Provider(ProviderError::UnexpectedResponse {
        details: "provider stream state poisoned".to_string(),
    })
}

pub fn u64_to_usize(value: u64) -> usize {
    usize::try_from(value).map_or(usize::MAX, |value| value)
}
