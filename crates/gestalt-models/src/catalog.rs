use std::{collections::HashMap, sync::OnceLock};

use gestalt_core::{
    message::Message,
    model::{ModelInfo, ModelInfoSource},
};

#[derive(Debug, Clone)]
pub struct ModelCatalog {
    layers: Vec<Vec<ModelInfo>>,
}

impl Default for ModelCatalog {
    fn default() -> Self {
        Self {
            layers: vec![built_in_models().clone()],
        }
    }
}

impl ModelCatalog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn built_in() -> &'static Self {
        static BUILT_IN: OnceLock<ModelCatalog> = OnceLock::new();
        BUILT_IN.get_or_init(Self::default)
    }

    #[must_use]
    pub fn with_layer(mut self, models: Vec<ModelInfo>) -> Self {
        if !models.is_empty() {
            self.layers.push(models);
        }
        self
    }

    #[must_use]
    pub fn list(&self) -> Vec<ModelInfo> {
        let mut merged = HashMap::<String, ModelInfo>::new();
        for layer in &self.layers {
            for model in layer {
                merged.insert(model.qualified_id.clone(), model.clone());
            }
        }

        let mut models = merged.into_values().collect::<Vec<_>>();
        models.sort_by(|left, right| left.qualified_id.cmp(&right.qualified_id));
        models
    }

    #[must_use]
    pub fn get(&self, query: &str) -> Option<ModelInfo> {
        if query.contains('/') {
            return self.get_qualified(query);
        }

        let mut matches = self
            .list()
            .into_iter()
            .filter(|model| model.model_id == query)
            .collect::<Vec<_>>();

        if matches.len() == 1 {
            matches.pop()
        } else {
            None
        }
    }

    #[must_use]
    pub fn get_qualified(&self, qualified_id: &str) -> Option<ModelInfo> {
        self.list()
            .into_iter()
            .find(|model| model.qualified_id == qualified_id)
    }

    #[must_use]
    pub fn get_provider_model(&self, provider: &str, model_id: &str) -> Option<ModelInfo> {
        self.get_qualified(&format!("{provider}/{model_id}"))
    }

    #[must_use]
    pub fn provider_default(&self, provider: &str) -> Option<ModelInfo> {
        let prefix = format!("{provider}/");
        self.list()
            .into_iter()
            .find(|model| model.qualified_id.starts_with(&prefix))
    }

    #[must_use]
    pub fn estimate_cost(&self, model: &str, input_tokens: usize, output_tokens: usize) -> Option<f64> {
        let info = self.get(model)?;
        let input_rate = info.input_cost_per_million?;
        let output_rate = info.output_cost_per_million?;

        Some(token_cost(input_tokens, input_rate) + token_cost(output_tokens, output_rate))
    }

    #[must_use]
    pub fn count_tokens(messages: &[Message]) -> usize {
        let chars = messages.iter().map(message_chars).sum::<usize>();
        chars.saturating_add(3) / 4 + messages.len().saturating_mul(4)
    }
}

fn built_in_models() -> &'static Vec<ModelInfo> {
    static BUILT_INS: OnceLock<Vec<ModelInfo>> = OnceLock::new();
    BUILT_INS.get_or_init(|| {
        vec![
            ModelInfo {
                qualified_id: "anthropic/claude-3-5-sonnet-20241022".to_string(),
                model_id: "claude-3-5-sonnet-20241022".to_string(),
                display_name: "Claude 3.5 Sonnet".to_string(),
                max_context_tokens: 200_000,
                max_output_tokens: 8_192,
                supports_tools: true,
                supports_vision: true,
                supports_json_schema: true,
                supports_thinking: true,
                supports_prompt_caching: true,
                input_cost_per_million: Some(3.0),
                output_cost_per_million: Some(15.0),
                source: ModelInfoSource::BuiltIn,
                last_updated: Some("2026-05-31".to_string()),
            },
            ModelInfo {
                qualified_id: "anthropic/claude-sonnet-4-6".to_string(),
                model_id: "claude-sonnet-4-6".to_string(),
                display_name: "Claude Sonnet 4.6".to_string(),
                max_context_tokens: 200_000,
                max_output_tokens: 8_192,
                supports_tools: true,
                supports_vision: true,
                supports_json_schema: true,
                supports_thinking: true,
                supports_prompt_caching: true,
                input_cost_per_million: Some(3.0),
                output_cost_per_million: Some(15.0),
                source: ModelInfoSource::BuiltIn,
                last_updated: Some("2026-05-31".to_string()),
            },
            ModelInfo {
                qualified_id: "openai/gpt-4o-mini".to_string(),
                model_id: "gpt-4o-mini".to_string(),
                display_name: "GPT-4o Mini".to_string(),
                max_context_tokens: 128_000,
                max_output_tokens: 16_384,
                supports_tools: true,
                supports_vision: true,
                supports_json_schema: true,
                supports_thinking: false,
                supports_prompt_caching: false,
                input_cost_per_million: Some(0.15),
                output_cost_per_million: Some(0.60),
                source: ModelInfoSource::BuiltIn,
                last_updated: Some("2026-05-31".to_string()),
            },
            ModelInfo {
                qualified_id: "openai-compatible/gpt-4o-mini".to_string(),
                model_id: "gpt-4o-mini".to_string(),
                display_name: "GPT-4o Mini (Compatible)".to_string(),
                max_context_tokens: 128_000,
                max_output_tokens: 16_384,
                supports_tools: true,
                supports_vision: true,
                supports_json_schema: true,
                supports_thinking: false,
                supports_prompt_caching: false,
                input_cost_per_million: Some(0.15),
                output_cost_per_million: Some(0.60),
                source: ModelInfoSource::BuiltIn,
                last_updated: Some("2026-05-31".to_string()),
            },
        ]
    })
}

fn message_chars(message: &Message) -> usize {
    match message {
        Message::System { content } | Message::ToolResult { content, .. } => content.len(),
        Message::User { content } | Message::Assistant { content } => content
            .iter()
            .map(|block| serde_json::to_string(block).map_or(0, |text| text.len()))
            .sum(),
    }
}

#[expect(clippy::cast_precision_loss, reason = "approximate token pricing is reported in USD")]
fn token_cost(tokens: usize, rate_per_million: f64) -> f64 {
    (tokens as f64 / 1_000_000.0) * rate_per_million
}
