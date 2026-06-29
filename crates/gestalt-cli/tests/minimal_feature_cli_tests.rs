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
}
