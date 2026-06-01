use gestalt_core::{ModelInfo, ModelInfoSource};
use gestalt_models::ModelCatalog;

fn override_model() -> ModelInfo {
    ModelInfo {
        qualified_id: "openai/gpt-4o-mini".to_string(),
        model_id: "gpt-4o-mini".to_string(),
        display_name: "Workspace GPT-4o Mini".to_string(),
        max_context_tokens: 256_000,
        max_output_tokens: 16_384,
        supports_tools: true,
        supports_vision: true,
        supports_json_schema: true,
        supports_thinking: false,
        supports_prompt_caching: false,
        input_cost_per_million: Some(0.15),
        output_cost_per_million: Some(0.60),
        source: ModelInfoSource::WorkspaceOverride,
        last_updated: None,
    }
}

#[test]
fn catalog_resolves_qualified_and_unqualified_ids() {
    let catalog = ModelCatalog::new();
    assert!(catalog.get("openai/gpt-4o-mini").is_some());
    assert!(catalog.get("claude-sonnet-4-6").is_some());
}

#[test]
fn later_layers_override_built_ins() {
    let catalog = ModelCatalog::new().with_layer(vec![override_model()]);
    let info = catalog.get("openai/gpt-4o-mini").expect("model exists");
    assert_eq!(info.max_context_tokens, 256_000);
    assert_eq!(info.source, ModelInfoSource::WorkspaceOverride);
}
