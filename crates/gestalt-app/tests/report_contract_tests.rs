use gestalt_app::reports::{
    AppDiagnosticV1, AppErrorProjectionV1, DiagnosticSeverityV1, ServiceReportV1,
};
use serde_json::json;

#[test]
fn app_report_success_contains_value_no_error() {
    let diagnostics = vec![AppDiagnosticV1 {
        severity: DiagnosticSeverityV1::Warning,
        code: "deprecated_alias".to_string(),
        message: "legacy option".to_string(),
        correlation_id: Some("corr-1".to_string()),
        details: Some(json!({ "field": "context.workspace_file" })),
    }];

    let report = ServiceReportV1::new(String::from("ok"))
        .with_diagnostics(diagnostics.clone())
        .with_correlation_id("corr-root");

    assert_eq!(report.value.as_deref(), Some("ok"));
    assert_eq!(report.diagnostics, diagnostics);
    assert_eq!(report.correlation_id.as_deref(), Some("corr-root"));
    assert!(report.error.is_none());

    let serialized = serde_json::to_string(&report).expect("serialize report");
    let round_trip: ServiceReportV1<String> =
        serde_json::from_str(&serialized).expect("deserialize report");
    assert_eq!(round_trip, report);
}

#[test]
fn app_report_failure_contains_error_no_value() {
    let failed = ServiceReportV1::<String>::failure(AppErrorProjectionV1 {
        code: "unavailable".to_string(),
        message: "provider unavailable".to_string(),
        retryable: true,
        details: None,
    });
    assert!(failed.value.is_none());
    assert_eq!(
        failed.error.as_ref().map(|error| error.code.as_str()),
        Some("unavailable")
    );
    assert!(failed.value.is_none());
}

#[test]
fn app_runtime_factory_does_not_print_to_stdout() {
    fn check(directory: &std::path::Path) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                check(&path);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                for presentation_macro in ["print!(", "println!(", "eprint!(", "eprintln!("] {
                    assert!(
                        !source.contains(presentation_macro),
                        "{} contains {presentation_macro}",
                        path.display()
                    );
                }
            }
        }
    }

    check(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}

#[test]
fn app_errors_project_to_stable_codes() {
    let cases = [
        (
            gestalt_core::HarnessError::Provider(gestalt_core::ProviderError::AuthFailed {
                provider: "test".to_string(),
            }),
            "AUTH_FAILED",
        ),
        (
            gestalt_core::HarnessError::Policy(gestalt_core::PolicyError::Denied(
                "blocked".to_string(),
            )),
            "POLICY_DENIED",
        ),
        (
            gestalt_core::HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
                field: "skills.active".to_string(),
                reason: "untrusted".to_string(),
            }),
            "SKILL_CONFIGURATION_ERROR",
        ),
        (
            gestalt_core::HarnessError::Tool(gestalt_core::ToolError::PathNotAllowed(
                "outside workspace".to_string(),
            )),
            "TOOL_PERMISSION_DENIED",
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(
            AppErrorProjectionV1::from_harness_error(&error).code,
            expected
        );
    }
}

#[cfg(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
))]
#[tokio::test]
async fn app_runtime_factory_reports_provider_warnings() {
    use gestalt_app::config::{EffectiveConfig, ProviderConfig, SecretString};
    use gestalt_app::runtime_factory::build_app_runtime_with_report;

    let workspace =
        std::env::temp_dir().join(format!("gestalt-app-report-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&workspace).unwrap();
    let mut config = EffectiveConfig::default();
    config.workspace_root = workspace.clone();
    config.config_path = workspace.join("gestalt.json");
    config.defaults.provider = Some("openai".to_string());
    config.defaults.model = Some("unknown-model".to_string());
    config.providers.insert(
        "openai".to_string(),
        ProviderConfig {
            api_key: Some(SecretString("inline-test-key".to_string())),
            ..ProviderConfig::default()
        },
    );

    let report = build_app_runtime_with_report(&config, None, None, None, None).await;

    assert!(report.value.is_some(), "{:?}", report.error);
    assert!(report.error.is_none());
    assert!(report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "provider_resolution_warning"));
    assert!(report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "auth_resolution_warning"));
    std::fs::remove_dir_all(workspace).unwrap();
}

#[cfg(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
))]
#[tokio::test]
async fn app_runtime_factory_reports_skill_and_extension_diagnostics() {
    use gestalt_app::config::EffectiveConfig;
    use gestalt_app::runtime_factory::build_app_runtime_with_report;

    fn config(workspace: &std::path::Path) -> EffectiveConfig {
        let mut config = EffectiveConfig::default();
        config.workspace_root = workspace.to_path_buf();
        config.config_path = workspace.join("gestalt.json");
        config.defaults.provider = Some("openai".to_string());
        config.defaults.model = Some("unknown-model".to_string());
        config
    }

    let skill_workspace =
        std::env::temp_dir().join(format!("gestalt-app-skill-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&skill_workspace).unwrap();
    let mut skill_config = config(&skill_workspace);
    skill_config.skills.active.push("missing-skill".to_string());
    let skill_report = build_app_runtime_with_report(
        &skill_config,
        Some("test-key".to_string()),
        None,
        None,
        None,
    )
    .await;
    assert!(skill_report.value.is_none());
    assert!(skill_report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "skill_configuration_error"));
    assert_eq!(
        skill_report.error.as_ref().map(|error| error.code.as_str()),
        Some("SKILL_CONFIGURATION_ERROR")
    );

    let extension_workspace =
        std::env::temp_dir().join(format!("gestalt-app-extension-{}", uuid::Uuid::new_v4()));
    let extension_dir = extension_workspace.join("extension");
    std::fs::create_dir_all(&extension_dir).unwrap();
    std::fs::write(
        extension_dir.join("gestalt.extension.toml"),
        r#"
manifest_version = 2

[package]
id = "com.example.report"
name = "Report"
version = "1.0.0"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"
optional = false

[components.entrypoint]
command = "unused"
args = []
"#,
    )
    .unwrap();
    let mut extension_config = config(&extension_workspace);
    extension_config
        .extensions
        .explicit_loads
        .push(extension_dir.display().to_string());
    let extension_report = build_app_runtime_with_report(
        &extension_config,
        Some("test-key".to_string()),
        None,
        None,
        None,
    )
    .await;
    assert!(
        extension_report.value.is_some(),
        "{:?}",
        extension_report.error
    );
    assert!(extension_report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "extension_rejected"));

    std::fs::remove_dir_all(skill_workspace).unwrap();
    std::fs::remove_dir_all(extension_workspace).unwrap();
}
