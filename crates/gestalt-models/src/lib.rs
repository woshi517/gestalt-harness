//! `gestalt-models` — Provider adapters and local model catalog.

pub mod anthropic;
pub mod auth;
pub mod catalog;
pub mod openai;
pub mod registry;
pub mod strict_schema;
pub mod tool_schema_adapter;
mod sse;

pub use anthropic::AnthropicProvider;
pub use auth::{
    ChainCredentialResolver, CredentialRef, CredentialResolver, CredentialSource,
    EnvironmentCredentialResolver, ProviderAuthConfig, ResolvedCredential,
};
pub use catalog::ModelCatalog;
pub use openai::OpenAiProvider;
pub use tool_schema_adapter::ToolSchemaAdapter;
