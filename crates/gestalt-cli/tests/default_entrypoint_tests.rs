#![cfg(feature = "full")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write script");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod");
}

#[test]
fn bare_cli_should_delegate_to_configured_tui_binary() {
    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-default-entry-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).expect("mkdir");

    let log_path = temp_dir.join("launch.log");
    let tui_path = temp_dir.join("fake-tui.sh");
    let script = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" > \"{}\"\n",
        log_path.display()
    );
    write_executable(&tui_path, &script);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .env("GESTALT_TUI_BIN", &tui_path)
        .arg("--workspace")
        .arg("tests/fixtures/workspaces/minimal")
        .output()
        .expect("run gestalt");

    assert!(
        output.status.success(),
        "bare gestalt should delegate successfully: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let logged = fs::read_to_string(&log_path).expect("delegation log");
    assert!(
        logged.contains("--workspace tests/fixtures/workspaces/minimal"),
        "delegated process should receive workspace args, got: {logged}"
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn bare_cli_should_report_missing_tui_binary() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .env(
            "GESTALT_TUI_BIN",
            "/definitely/missing/gestalt-tui-binary-for-test",
        )
        .output()
        .expect("run gestalt");

    assert!(!output.status.success(), "missing TUI binary should fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("gestalt-tui is not installed; run `cargo install gestalt-tui`"),
        "expected actionable error, got: {stderr}"
    );
}
