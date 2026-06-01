//! Smoke test to verify test fixture paths exist.
//! Prevents accidental deletion of fixture directories.

use std::path::Path;

#[test]
fn fixture_directories_exist() {
    // Cargo runs integration tests with the current working directory set to the crate's directory
    let fixtures = Path::new("../../tests/fixtures");
    assert!(fixtures.exists(), "tests/fixtures/ directory missing");

    let expected_dirs = [
        "provider-streams",
        "policy",
        "traces",
        "workspaces",
        "sources",
        "cli-golden",
    ];

    for dir in &expected_dirs {
        let path = fixtures.join(dir);
        assert!(
            path.exists(),
            "fixture directory missing: {}",
            path.display()
        );
    }
}

#[test]
fn minimal_workspace_fixture_exists() {
    let workspace = Path::new("../../tests/fixtures/workspaces/minimal/.gestalt");
    assert!(workspace.join("config.toml").exists());
    assert!(workspace.join("policies.toml").exists());
    assert!(workspace.join("workspace.md").exists());
    assert!(workspace.join("memory.md").exists());
}

#[test]
fn provider_and_trace_fixtures_are_populated() {
    let fixtures = Path::new("../../tests/fixtures");
    assert!(fixtures
        .join("provider-streams/openai-multiple-tools.sse")
        .exists());
    assert!(fixtures
        .join("provider-streams/anthropic-single-tool.sse")
        .exists());
    assert!(fixtures.join("traces/minimal-run.jsonl").exists());
    assert!(fixtures.join("cli-golden/replay-display.txt").exists());
}
