#![cfg(not(feature = "full"))]

use std::process::Command;

#[test]
fn disabled_command_should_remain_visible_and_return_typed_error() {
    let binary = env!("CARGO_BIN_EXE_gestalt");
    let help = Command::new(binary)
        .arg("--help")
        .output()
        .expect("run help");
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("run") && stdout.contains("verify") && stdout.contains("skill"));

    let output = Command::new(binary)
        .args(["run", "hello"])
        .output()
        .expect("run disabled command");
    assert!(String::from_utf8_lossy(&output.stderr).contains("FEATURE_DISABLED"));

    let json_output = Command::new(binary)
        .args(["--format", "json", "run", "hello"])
        .output()
        .expect("run disabled command in JSON mode");
    assert_eq!(json_output.status.code(), Some(7));
    assert!(json_output.stdout.is_empty());

    let error: serde_json::Value = serde_json::from_slice(&json_output.stderr).unwrap();
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["status"], "error");
    assert_eq!(error["data"], serde_json::Value::Null);
    assert_eq!(error["error"]["code"], "FEATURE_DISABLED");
    assert_eq!(error["error"]["retryable"], false);
    assert_eq!(error["warnings"], serde_json::json!([]));
}

#[test]
fn json_usage_errors_remain_machine_readable() {
    let binary = env!("CARGO_BIN_EXE_gestalt");
    let output = Command::new(binary)
        .args(["--format", "json", "--definitely-invalid"])
        .output()
        .expect("run invalid command");

    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["status"], "error");
    assert_eq!(error["error"]["code"], "USAGE");
    assert_eq!(error["error"]["retryable"], false);
    assert_eq!(error["warnings"], serde_json::json!([]));
}
