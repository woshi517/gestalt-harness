#![forbid(unsafe_code)]
//! Concrete vendor [`ModelProvider`] adapters (`anthropic`, `openai`, `jev`)
//! registering into `gestalt-runtime`'s [`ProviderRegistry`].

pub mod anthropic;
pub mod jev;
pub mod openai;

use std::collections::BTreeMap;
use std::sync::Arc;

use gestalt_runtime::{
    CredentialRequest, ModelRequest, ProviderContext, ProviderErrorKind, ProviderRegistry,
    RuntimeError,
};

pub use anthropic::AnthropicProvider;
pub use jev::JevProvider;
pub use openai::OpenAiProvider;

/// Register all 3 standard vendor [`gestalt_runtime::ModelProvider`] adapters
/// (`anthropic`, `openai`, `jev`) into [`ProviderRegistry`].
pub fn register_std_providers(registry: &mut ProviderRegistry) {
    registry.register_provider(Arc::new(AnthropicProvider));
    registry.register_provider(Arc::new(OpenAiProvider));
    registry.register_provider(Arc::new(JevProvider));
}

/// Resolve host credentials into HTTP authorization headers, failing with
/// [`ProviderErrorKind::AuthError`] if no credential is configured.
pub(crate) async fn resolve_auth_headers(
    req: &ModelRequest,
    ctx: &ProviderContext<'_>,
    api_key_header: &str,
    bearer_prefix_for_api_key: bool,
) -> Result<BTreeMap<String, String>, RuntimeError> {
    let cred_req = CredentialRequest::new(&req.provider_id)
        .with_env_key(req.config.as_ref().and_then(|c| c.env_key.clone()));
    let Some(record) = ctx.credentials.resolve(&cred_req).await? else {
        return Err(RuntimeError::ProviderError {
            kind: ProviderErrorKind::AuthError,
            message: format!("missing credential for provider '{}'", req.provider_id),
        });
    };
    Ok(record.to_auth_headers(api_key_header, bearer_prefix_for_api_key))
}

/// Pure safe Rust standard Base64 encoder (RFC 4648) for multimodal media blobs.
#[must_use]
pub(crate) fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}
