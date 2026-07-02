#![cfg(feature = "full")]

use std::path::PathBuf;

use gestalt_app::reports::{AppDiagnosticV1, DiagnosticSeverityV1};
use gestalt_cli::output::{
    CliErrorPayload, CliReport, ConfigValidateReport, JsonEnvelope, STABLE_COMMANDS_V1,
};
use serde::Serialize;
use serde_json::json;

#[derive(Serialize)]
struct DiagnosticReport {
    value: &'static str,
}

impl CliReport for DiagnosticReport {
    fn kind(&self) -> &'static str {
        "diagnostic.test"
    }

    fn render_text(&self) -> String {
        self.value.to_string()
    }

    fn diagnostics(&self) -> Vec<AppDiagnosticV1> {
        vec![AppDiagnosticV1 {
            severity: DiagnosticSeverityV1::Warning,
            code: "provider_resolution".to_string(),
            message: "using conservative model capabilities".to_string(),
            correlation_id: Some("correlation-1".to_string()),
            details: Some(json!({"provider": "custom"})),
        }]
    }
}

#[test]
fn cli_json_success_envelope_snapshot() {
    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: "config.validate".to_string(),
        data: json!({"workspace_root": "/workspace"}),
    };

    assert_eq!(
        serde_json::to_value(envelope).unwrap(),
        json!({
            "schema_version": 1,
            "status": "success",
            "kind": "config.validate",
            "data": {"workspace_root": "/workspace"},
            "error": null,
            "warnings": []
        })
    );
}

#[test]
fn cli_json_error_envelope_snapshot() {
    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: "error".to_string(),
        data: CliErrorPayload {
            code: "CONFIG_ERROR".to_string(),
            message: "invalid configuration".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        },
    };

    assert_eq!(
        serde_json::to_value(envelope).unwrap(),
        json!({
            "schema_version": 1,
            "status": "error",
            "kind": "error",
            "data": null,
            "error": {
                "code": "CONFIG_ERROR",
                "message": "invalid configuration",
                "retryable": false,
                "details": null,
                "correlation_id": null
            },
            "warnings": []
        })
    );
}

#[test]
fn cli_json_warnings_include_app_diagnostics() {
    let report = DiagnosticReport { value: "ok" };
    let warnings = report.diagnostics();
    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: report,
    }
    .with_warnings(warnings);
    let json = serde_json::to_value(envelope).unwrap();

    assert_eq!(json["warnings"][0]["severity"], "warning");
    assert_eq!(json["warnings"][0]["code"], "provider_resolution");
    assert_eq!(
        json["warnings"][0]["message"],
        "using conservative model capabilities"
    );
    assert_eq!(json["warnings"][0]["correlation_id"], "correlation-1");
    assert_eq!(json["warnings"][0]["details"]["provider"], "custom");

    let workspace =
        std::env::temp_dir().join(format!("gestalt-cli-warning-{}", uuid::Uuid::new_v4()));
    let run_id = "20260702T120000Z-warning-contract";
    let run_dir = workspace.join(".gestalt/runs").join(run_id);
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(
        run_dir.join("trace.jsonl"),
        r#"{"v":1,"session_id":"warning-contract","turn_id":1,"seq":1,"ts":"2026-07-02T12:00:00Z","event":{"type":"artifact_created","path":"artifacts/missing.txt","size_bytes":1,"mime_type":"text/plain","hash":"abc"},"redacted":false}"#,
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .env("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir")
        .args(["--format", "json", "--workspace"])
        .arg(&workspace)
        .args(["trace", "validate", run_id])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["warnings"][0]["severity"], "warning");
    assert_eq!(json["warnings"][0]["code"], "trace_validate");
    assert!(json["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .contains("referenced artifact does not exist"));

    std::fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn cli_text_output_remains_human_readable() {
    let report = ConfigValidateReport {
        workspace_root: PathBuf::from("/workspace"),
    };

    assert_eq!(report.render_text(), "valid workspace=/workspace");
    assert!(!report.render_text().starts_with('{'));
}

#[test]
fn cli_stable_commands_have_json_contract() {
    let expected = [
        ("config validate", "config.validate"),
        ("config show", "config.show"),
        ("config explain", "config.explain"),
        ("workspace info", "workspace.info"),
        ("providers list", "providers.list"),
        ("providers inspect", "providers.inspect"),
        ("models list", "models.list"),
        ("models inspect", "models.inspect"),
        ("profiles list", "profiles.list"),
        ("profiles inspect", "profiles.inspect"),
        ("policy explain", "policy.explain"),
        ("tools list", "tools.list"),
        ("tools inspect", "tools.inspect"),
        ("connect", "connect"),
        ("runs list", "runs.list"),
        ("runs inspect", "runs.inspect"),
        ("trace inspect", "trace.inspect"),
        ("context explain", "context.explain"),
        ("extension validate", "extension.validate"),
    ];
    let actual = STABLE_COMMANDS_V1
        .iter()
        .map(|entry| (entry.command, entry.kind))
        .collect::<Vec<_>>();

    assert_eq!(actual, expected);
    assert!(actual.iter().all(|(_, kind)| !kind.is_empty()));

    let temp = std::env::temp_dir().join(format!(
        "gestalt-extension-validate-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let manifest = temp.join("gestalt.extension.toml");
    std::fs::write(
        &manifest,
        r#"
manifest_version = 2

[package]
id = "com.example.contract"
name = "Contract"
version = "1.0.0"

[compatibility]
gestalt = ">=0.1"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "true"
"#,
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .args(["--format", "json", "extension", "validate"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["kind"], "extension.validate");
    assert_eq!(json["status"], "success");

    std::fs::remove_dir_all(temp).unwrap();
}

#[test]
fn cli_unstable_commands_are_not_documented_as_stable() {
    let docs = std::fs::read_to_string(cli_contract_path()).unwrap();
    let stable = docs
        .split("## Stable Commands")
        .nth(1)
        .unwrap()
        .split("## Experimental Commands")
        .next()
        .unwrap();

    for command in [
        "`run`",
        "`chat`",
        "`replay`",
        "`runtime inspect`",
        "`doctor`",
    ] {
        assert!(
            !stable.contains(command),
            "{command} must not appear in the stable command table"
        );
    }
    for entry in STABLE_COMMANDS_V1 {
        assert!(
            stable.contains(&format!("`{}`", entry.command))
                || stable.contains(&combined_table_entry(entry.command)),
            "stable docs missing {}",
            entry.command
        );
    }
}

fn combined_table_entry(command: &str) -> String {
    let (group, action) = command.split_once(' ').unwrap_or((command, ""));
    format!("`{group} list`, `{group} {action}`")
}

fn cli_contract_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs/v0.1/cli-automation.md")
}
