use gestalt_app::auth::resolve_auth;
use gestalt_app::config::{load_effective_config, CliOverrides};
use gestalt_app::run::run_prompt;
use gestalt_app::runtime_factory::build_cli_runtime;
use gestalt_core::cancel::CancelToken;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workspace =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/workspaces/minimal");
    let overrides = CliOverrides {
        workspace: Some(workspace),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides)?;
    let provider = config.resolve_provider()?;
    let _auth = resolve_auth(&config, provider.provider_name())?;
    let _runtime = build_cli_runtime(&config, None, None, None, None).await?;
    let _ = run_prompt(
        &config,
        "hello from embed_app",
        None,
        CancelToken::new(),
        None,
        None,
        None,
        None,
    )
    .await;
    Ok(())
}
