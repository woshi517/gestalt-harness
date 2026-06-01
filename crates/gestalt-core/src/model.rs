use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Fully qualified model reference, e.g. "anthropic/claude-sonnet-4-6".
    pub qualified_id: String,

    /// Provider-local model ID, e.g. "claude-sonnet-4-6".
    pub model_id: String,

    /// Human-readable name.
    pub display_name: String,

    /// Maximum input context window.
    pub max_context_tokens: usize,

    /// Maximum generation length.
    pub max_output_tokens: usize,

    /// Whether this model supports tool use.
    pub supports_tools: bool,

    /// Whether this model supports vision inputs.
    pub supports_vision: bool,

    /// Whether this model supports structured JSON/schema output.
    pub supports_json_schema: bool,

    /// Whether this model supports reasoning/thinking mode.
    pub supports_thinking: bool,

    /// Whether this model supports prompt caching.
    pub supports_prompt_caching: bool,

    /// Optional input price per million tokens.
    pub input_cost_per_million: Option<f64>,

    /// Optional output price per million tokens.
    pub output_cost_per_million: Option<f64>,

    /// Source of this metadata: built-in, refreshed catalog, provider API,
    /// user config, or workspace override.
    pub source: ModelInfoSource,

    /// ISO-8601 date or timestamp when this metadata was last refreshed.
    pub last_updated: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelInfoSource {
    BuiltIn,
    RefreshedCatalog,
    ProviderDiscovered,
    UserDefined,
    WorkspaceOverride,
    CliSelected,
}
