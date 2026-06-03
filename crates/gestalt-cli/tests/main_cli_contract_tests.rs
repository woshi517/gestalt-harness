use std::process::Command;
use std::fs;
use gestalt_cli::workspace::init_workspace;

fn get_bin() -> &'static str {
    env!("CARGO_BIN_EXE_gestalt")
}

fn create_temp_workspace() -> std::path::PathBuf {
    let temp = std::env::temp_dir().join(format!("gestalt-bin-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[test]
fn test_cli_format_json_envelope() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("status")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout_str = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout_str).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "workspace.status");
    assert!(json["data"]["config_valid"].as_bool().unwrap());

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_cli_invalid_format_exits_non_zero() {
    let output = Command::new(get_bin())
        .arg("--format")
        .arg("xml")
        .arg("status")
        .output()
        .unwrap();

    assert!(!output.status.success());
}

#[test]
fn test_cli_quiet_config_validate_still_prints() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("text")
        .arg("--quiet")
        .arg("config")
        .arg("validate")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout_str = String::from_utf8(output.stdout).unwrap();
    assert!(stdout_str.contains("valid workspace"));

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_cli_invalid_provider_json_error() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--provider")
        .arg("invalid_provider")
        .arg("--format")
        .arg("json")
        .arg("run")
        .arg("hello")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr_str = String::from_utf8(output.stderr).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stderr_str).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "error");
    assert_eq!(json["data"]["code"], "PROVIDER_ERROR");

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_cli_runs_delete_without_yes_exits_non_zero() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("runs")
        .arg("delete")
        .arg("20260602T100000Z-session-1")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr_str = String::from_utf8(output.stderr).unwrap();
    assert!(stderr_str.contains("non-interactive execution requires") || stderr_str.contains("error:"));

    let _ = fs::remove_dir_all(&temp_root);
}
