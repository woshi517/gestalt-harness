use gestalt_app::reports::{
    AppDiagnosticV1, AppErrorProjectionV1, DiagnosticSeverityV1, ServiceReportV1,
};
use serde_json::json;

#[test]
fn app_diagnostics_and_reports_preserve_order_and_payloads() {
    let diagnostics = vec![
        AppDiagnosticV1 {
            severity: DiagnosticSeverityV1::Warning,
            code: "deprecated_alias".to_string(),
            message: "legacy option".to_string(),
            correlation_id: Some("corr-1".to_string()),
            details: Some(json!({ "field": "context.workspace_file" })),
        },
        AppDiagnosticV1 {
            severity: DiagnosticSeverityV1::Error,
            code: "invalid_value".to_string(),
            message: "bad input".to_string(),
            correlation_id: None,
            details: None,
        },
    ];

    let report = ServiceReportV1 {
        value: Some(String::from("ok")),
        diagnostics: diagnostics.clone(),
        error: Some(AppErrorProjectionV1 {
            code: "invalid_value".to_string(),
            message: "bad input".to_string(),
            retryable: false,
            details: Some(json!({ "field": "provider" })),
        }),
        correlation_id: Some("corr-root".to_string()),
    };

    assert_eq!(report.value.as_deref(), Some("ok"));
    assert_eq!(report.diagnostics, diagnostics);
    assert_eq!(report.correlation_id.as_deref(), Some("corr-root"));
    assert_eq!(
        report.error.as_ref().map(|error| error.code.as_str()),
        Some("invalid_value")
    );

    let serialized = serde_json::to_string(&report.diagnostics).expect("serialize diagnostics");
    let round_trip: Vec<AppDiagnosticV1> =
        serde_json::from_str(&serialized).expect("deserialize diagnostics");
    assert_eq!(round_trip, diagnostics);

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
}

#[test]
fn reusable_app_sources_have_no_presentation_writes() {
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
