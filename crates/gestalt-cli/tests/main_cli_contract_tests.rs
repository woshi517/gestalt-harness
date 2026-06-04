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

#[test]
fn test_cli_additional_json_contracts() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();
    let gestalt_dir = temp_root.join(".gestalt");
    let runs_dir = gestalt_dir.join("runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run_dir = runs_dir.join("20260603T140000Z-session-contract");
    fs::create_dir_all(&run_dir).unwrap();

    let trace_content = r#"{"v":1,"session_id":"session-contract","turn_id":1,"seq":1,"ts":"2026-06-03T14:00:00Z","event":{"type":"user_message","content":"hello"},"redacted":false}"#;
    fs::write(run_dir.join("trace.jsonl"), trace_content).unwrap();

    // 1. trace inspect format JSON
    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("trace")
        .arg("inspect")
        .arg("20260603T140000Z-session-contract")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "trace.inspect");

    // 2. trace validate format JSON
    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("trace")
        .arg("validate")
        .arg("20260603T140000Z-session-contract")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "trace.validate");

    // 3. policy explain format JSON
    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("policy")
        .arg("explain")
        .arg("--tool")
        .arg("bash")
        .arg("--input")
        .arg(r#"{"command":"echo hello"}"#)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "policy.explain");

    // 4. policy explain malformed JSON input
    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("policy")
        .arg("explain")
        .arg("--tool")
        .arg("bash")
        .arg("--input")
        .arg("{invalid")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "error");

    // 5. policy explain missing command bash input (shape-invalid)
    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("policy")
        .arg("explain")
        .arg("--tool")
        .arg("bash")
        .arg("--input")
        .arg("{}")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "error");

    // 6. providers inspect unknown provider (failure check)
    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("providers")
        .arg("inspect")
        .arg("non-existent-provider")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "error");

    // 7. export unsupported sharegpt (failure check)
    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("export")
        .arg("--format")
        .arg("sharegpt")
        .arg("20260603T140000Z-session-contract")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "error");

    // 8. doctor command format JSON
    let output = Command::new(get_bin())
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("doctor")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "doctor");

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_cli_provider_connection_and_profile_contracts() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    // 1. Test connect openrouter
    let output = Command::new(get_bin())
        .env("XDG_CONFIG_HOME", &temp_root)
        .env("XDG_CACHE_HOME", &temp_root)
        .env("GESTALT_USE_FAKE_KEYCHAIN", "1")
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("connect")
        .arg("openrouter")
        .arg("--api-key")
        .arg("sk-or-test-key")
        .arg("--set-default")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "connect");
    assert_eq!(json["data"]["provider"], "openrouter");
    assert_eq!(json["data"]["status"], "connected");
    assert_eq!(json["data"]["profile_created"].as_str(), Some("default"));
    assert_eq!(json["data"]["keychain_stored"].as_bool(), Some(true));

    // 2. Test profiles list
    let output = Command::new(get_bin())
        .env("XDG_CONFIG_HOME", &temp_root)
        .env("XDG_CACHE_HOME", &temp_root)
        .env("GESTALT_USE_FAKE_KEYCHAIN", "1")
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("profiles")
        .arg("list")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "profiles.list");
    let profiles = json["data"]["profiles"].as_array().unwrap();
    assert!(profiles.iter().any(|p| p["name"] == "default"));

    // 3. Test providers doctor
    let output = Command::new(get_bin())
        .env("XDG_CONFIG_HOME", &temp_root)
        .env("XDG_CACHE_HOME", &temp_root)
        .env("GESTALT_USE_FAKE_KEYCHAIN", "1")
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("providers")
        .arg("doctor")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "providers.doctor");
    let results = json["data"]["results"].as_array().unwrap();
    let openrouter_res = results.iter().find(|r| r["provider"] == "openrouter").unwrap();
    assert_eq!(openrouter_res["auth_status"], "present");
    assert!(openrouter_res["auth_source"].as_str().unwrap().contains("keychain"));

    // 4. Test models search
    let output = Command::new(get_bin())
        .env("XDG_CONFIG_HOME", &temp_root)
        .env("XDG_CACHE_HOME", &temp_root)
        .env("GESTALT_USE_FAKE_KEYCHAIN", "1")
        .arg("--workspace")
        .arg(&temp_root)
        .arg("--format")
        .arg("json")
        .arg("models")
        .arg("search")
        .arg("free")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["kind"], "models.search");
    let models = json["data"]["models"].as_array().unwrap();
    assert!(models.iter().any(|m| m["qualified_id"] == "openrouter/free"));

    let _ = fs::remove_dir_all(&temp_root);
}

