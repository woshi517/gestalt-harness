use std::fs;
use std::path::Path;
use gestalt_cli::config::WorkspaceConfig;
use schemars::schema_for;

#[test]
fn test_schema_drift() {
    let generated_schema = schema_for!(WorkspaceConfig);
    let generated_json = serde_json::to_string_pretty(&generated_schema).unwrap();

    let schema_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs/schemas/gestalt.schema.json");

    let existing_json = fs::read_to_string(&schema_path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    let normalized_generated = generated_json.replace("\r\n", "\n");

    if normalized_generated != existing_json {
        // Print diff/details
        println!("Generated Schema:\n{}", normalized_generated);
        panic!("Schema drift detected! Generated WorkspaceConfig schema does not match docs/schemas/gestalt.schema.json");
    }
}

#[test]
fn test_minimal_valid_config() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/config/v1/minimal_valid.json");
    let config = WorkspaceConfig::from_file(&fixture_path);
    assert!(config.is_ok(), "Failed to parse minimal_valid.json: {:?}", config.err());
}

#[test]
fn test_full_valid_config() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/config/v1/full_valid.json");
    let config = WorkspaceConfig::from_file(&fixture_path);
    assert!(config.is_ok(), "Failed to parse full_valid.json: {:?}", config.err());
}

#[test]
fn test_unknown_top_level_key_fails() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/config/v1/unknown_top_level_key.json");
    let config = WorkspaceConfig::from_file(&fixture_path);
    assert!(config.is_err(), "unknown_top_level_key.json should fail due to deny_unknown_fields");
}

#[test]
fn test_unknown_nested_key_fails() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/config/v1/unknown_nested_key.json");
    let config = WorkspaceConfig::from_file(&fixture_path);
    assert!(config.is_err(), "unknown_nested_key.json should fail due to deny_unknown_fields");
}

#[test]
fn test_invalid_version_fails() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/config/v1/invalid_version.json");
    let config = WorkspaceConfig::from_file(&fixture_path);
    assert!(config.is_err(), "invalid_version.json should fail because version != 1");
    if let Err(err) = config {
        let err_str = err.to_string();
        assert!(err_str.contains("version must be 1") || err_str.contains("version"), "Error did not mention version requirement: {}", err_str);
    }
}
