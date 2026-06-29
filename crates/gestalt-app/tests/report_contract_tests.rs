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
        value: String::from("ok"),
        diagnostics: diagnostics.clone(),
        error: Some(AppErrorProjectionV1 {
            code: "invalid_value".to_string(),
            message: "bad input".to_string(),
            retryable: false,
            details: Some(json!({ "field": "provider" })),
        }),
        correlation_id: Some("corr-root".to_string()),
    };

    assert_eq!(report.value, "ok");
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
}
