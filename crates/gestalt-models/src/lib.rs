//! `gestalt-models` — Provider adapters and local model catalog.

pub mod anthropic;
pub mod auth;
pub mod catalog;
pub mod openai;
pub mod registry;
mod sse;

pub use anthropic::AnthropicProvider;
pub use auth::{
    ChainCredentialResolver, CredentialRef, CredentialResolver, CredentialSource,
    EnvironmentCredentialResolver, ProviderAuthConfig, ResolvedCredential,
};
pub use catalog::ModelCatalog;
pub use openai::OpenAiProvider;
