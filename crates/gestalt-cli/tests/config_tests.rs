use std::path::PathBuf;

use gestalt_cli::config::{CliOverrides, validate_workspace_config};

#[test]
fn validate_workspace_fixture_config() {
    let config = validate_workspace_config(&CliOverrides {
        workspace: Some(PathBuf::from("../../tests/fixtures/workspaces/minimal")),
        ..CliOverrides::default()
    })
    .expect("config validates");

    assert_eq!(config.selected_provider().expect("provider"), "anthropic");
    assert_eq!(config.selected_model().as_deref(), Some("claude-sonnet-4-6"));
}
