use gestalt_app::config::WorkspaceConfig;
use schemars::schema_for;
use std::fs;
use std::path::Path;

#[test]
fn schema_matches_default_workspace_config() {
    let generated_schema = schema_for!(WorkspaceConfig);
    let generated_json = serde_json::to_string_pretty(&generated_schema).unwrap();
    let default_json = serde_json::to_value(WorkspaceConfig::default()).unwrap();
    let round_trip: WorkspaceConfig = serde_json::from_value(default_json.clone()).unwrap();

    assert_eq!(default_json["version"], 1);
    assert_eq!(
        serde_json::to_value(round_trip).unwrap(),
        default_json,
        "default config must round-trip through the v1 input model"
    );

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
        if std::env::var("UPDATE_SCHEMA").is_ok() {
            fs::write(&schema_path, &normalized_generated).unwrap();
            println!("Updated docs/schemas/gestalt.schema.json");
        } else {
            // Print diff/details
            println!("Generated Schema:\n{}", normalized_generated);
            panic!("Schema drift detected! Generated WorkspaceConfig schema does not match docs/schemas/gestalt.schema.json");
        }
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
    assert!(
        config.is_ok(),
        "Failed to parse minimal_valid.json: {:?}",
        config.err()
    );
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
    assert!(
        config.is_ok(),
        "Failed to parse full_valid.json: {:?}",
        config.err()
    );
}

#[test]
fn config_rejects_unknown_fields() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/config/v1/unknown_top_level_key.json");
    let config = WorkspaceConfig::from_file(&fixture_path);
    assert!(
        config.is_err(),
        "unknown_top_level_key.json should fail due to deny_unknown_fields"
    );
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
    assert!(
        config.is_err(),
        "unknown_nested_key.json should fail due to deny_unknown_fields"
    );
}

#[test]
fn config_rejects_unsupported_version() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/config/v1/invalid_version.json");
    let config = WorkspaceConfig::from_file(&fixture_path);
    assert!(
        config.is_err(),
        "invalid_version.json should fail because version != 1"
    );
    if let Err(err) = config {
        let err_str = err.to_string();
        assert!(
            err_str.contains("version must be 1") || err_str.contains("version"),
            "Error did not mention version requirement: {}",
            err_str
        );
    }
}

#[test]
fn config_rejects_missing_version() {
    let fixture = fixture("missing_version.json");
    let error = WorkspaceConfig::from_file(&fixture).expect_err("missing version must fail");

    assert!(matches!(
        error,
        gestalt_core::HarnessError::Config(gestalt_core::ConfigError::MissingVersion)
    ));
}

#[test]
fn config_rejects_non_integer_version() {
    let fixture = fixture("invalid_version_type.json");
    let error = WorkspaceConfig::from_file(&fixture).expect_err("invalid version must fail");

    assert!(matches!(
        error,
        gestalt_core::HarnessError::Config(gestalt_core::ConfigError::InvalidVersion)
    ));
}

#[test]
fn config_rejects_removed_aliases() {
    let removed_fields = [
        serde_json::json!({"version": 1, "context": {"workspace_file": "workspace.md"}}),
        serde_json::json!({"version": 1, "context": {"memory_file": "memory.md"}}),
        serde_json::json!({"version": 1, "policies": {"bash": {"yolo_allow": ["git"]}}}),
        serde_json::json!({"version": 1, "policies": {"bash": {"always_confirm": ["rm"]}}}),
        serde_json::json!({"version": 1, "policies": {"bash": {"always_deny": ["sudo"]}}}),
    ];

    for config in removed_fields {
        assert!(
            serde_json::from_value::<WorkspaceConfig>(config).is_err(),
            "removed aliases must remain outside the v1 input model"
        );
    }

    let schema = serde_json::to_string(&schema_for!(WorkspaceConfig)).unwrap();
    for removed in [
        "workspace_file",
        "memory_file",
        "yolo_allow",
        "always_confirm",
        "always_deny",
    ] {
        assert!(
            !schema.contains(&format!("\"{removed}\"")),
            "removed alias {removed} must not appear in the v1 schema"
        );
    }
}

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("tests/fixtures/config/v1")
        .join(name)
}
