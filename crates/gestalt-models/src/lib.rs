//! `gestalt-models` — Provider adapters and local model catalog.

pub mod anthropic;
pub mod auth;
pub mod catalog;
pub mod openai;
pub mod registry;
mod sse;
pub mod strict_schema;
pub mod tool_schema_adapter;

pub use anthropic::AnthropicProvider;
pub use auth::{
    ChainCredentialResolver, ConfiguredCredential, CredentialRef, CredentialResolver,
    CredentialSource, EnvironmentCredentialResolver, InlineCredentialResolver, ProviderAuthConfig,
    ResolvedCredential,
};
pub use catalog::ModelCatalog;
pub use openai::{OpenAiChatCompletionsProvider, OpenAiResponsesProvider};
pub use registry::get_by_api_format_with_resolver;
pub use tool_schema_adapter::ToolSchemaAdapter;
