use std::path::PathBuf;

use gestalt_cli::{
    config::{load_effective_config, CliOverrides},
    models::inspect_model,
    providers::list_providers,
};

fn config() -> gestalt_cli::config::EffectiveConfig {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
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

#[tokio::test]
async fn test_models_filtering_and_refresh() {
    use gestalt_cli::models::{list_models, refresh_models};

    let cfg = config();
    // filter models list by provider
    let all = list_models(&cfg, None);
    let openai_only = list_models(&cfg, Some("openai"));
    assert!(openai_only.len() < all.len());
    assert!(openai_only
        .iter()
        .all(|m| m.qualified_id.starts_with("openai/")));

    // refresh offline
    let refresh_offline = refresh_models(&cfg, false).await.unwrap();
    assert_eq!(refresh_offline.status, "offline");

    // refresh live
    let refresh_live = refresh_models(&cfg, true).await.unwrap();
    assert!(
        refresh_live.status == "offline"
            || refresh_live.status == "unsupported"
            || refresh_live.status == "live requested"
            || refresh_live.status == "live performed"
    );
}
