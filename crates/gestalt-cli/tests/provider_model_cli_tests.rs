use std::path::PathBuf;

use gestalt_cli::{
    config::{load_effective_config, CliOverrides},
    models::inspect_model,
    providers::list_providers,
};

fn config() -> gestalt_cli::config::EffectiveConfig {
    load_effective_config(&CliOverrides {
        workspace: Some(PathBuf::from("../../tests/fixtures/workspaces/minimal")),
        ..CliOverrides::default()
    })
    .expect("config loads")
}

#[test]
fn provider_list_includes_openai_compatible() {
    let providers = list_providers(&config());
    assert!(providers
        .iter()
        .any(|provider| provider == "openai-compatible"));
}

#[test]
fn model_inspect_reads_catalog_metadata() {
    let model = inspect_model(&config(), "openai/gpt-4o-mini").expect("model exists");
    assert_eq!(model.display_name, "GPT-4o Mini");
}
