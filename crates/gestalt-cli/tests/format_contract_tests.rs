use gestalt_cli::output::{
    CliReport, ConfigValidateReport, JsonEnvelope, ModelsListReport, ProvidersListReport,
    ReplayReport, RunReport, ToolsListReport, ToolsInspectReport, ToolsClassifyReport,
    AuthDoctorReport, GlobalDoctorReport, AuthDoctorEntry, ToolInfoEntry,
};
use gestalt_core::model::{ModelInfo, ModelInfoSource};
use std::path::PathBuf;

#[test]
fn test_run_report_contract() {
    let report = RunReport {
        run_dir: PathBuf::from("runs/test-run-123"),
    };
    assert_eq!(report.kind(), "run");
    assert_eq!(report.render_text(), "run_dir=runs/test-run-123");

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""schema_version":1"#));
    assert!(serialized.contains(r#""kind":"run""#));
    assert!(serialized.contains(r#""run_dir":"runs/test-run-123""#));
}

#[test]
fn test_replay_report_contract() {
    let report = ReplayReport {
        rendered: "turn 1: user prompt\nturn 2: assistant response".to_string(),
    };
    assert_eq!(report.kind(), "replay");
    assert_eq!(
        report.render_text(),
        "turn 1: user prompt\nturn 2: assistant response"
    );

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"replay""#));
}

#[test]
fn test_config_validate_report_contract() {
    let report = ConfigValidateReport {
        workspace_root: PathBuf::from("/workspace"),
    };
    assert_eq!(report.kind(), "config.validate");
    assert_eq!(report.render_text(), "valid workspace=/workspace");

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"config.validate""#));
}

#[test]
fn test_providers_list_report_contract() {
    let report = ProvidersListReport {
        providers: vec!["anthropic".to_string(), "openai".to_string()],
    };
    assert_eq!(report.kind(), "providers.list");
    assert_eq!(report.render_text(), "anthropic\nopenai");
}

#[test]
fn test_models_list_report_contract() {
    let m1 = ModelInfo {
        qualified_id: "anthropic/claude-3".to_string(),
        model_id: "claude-3".to_string(),
        display_name: "Claude 3".to_string(),
        max_context_tokens: 100000,
        max_output_tokens: 4000,
        supports_tools: true,
        supports_vision: false,
        supports_json_schema: true,
        supports_thinking: false,
        supports_prompt_caching: false,
        input_cost_per_million: None,
        output_cost_per_million: None,
        source: ModelInfoSource::BuiltIn,
        last_updated: None,
    };
    let report = ModelsListReport { models: vec![m1] };
    assert_eq!(report.kind(), "models.list");
    let expected_text = vec![
        format!("{:<40} | {:<12} | {:<12} | {:<6} | {:<6} | {:<8} | {:<6}",
            "Qualified ID", "Input $/M", "Output $/M", "Vision", "Tools", "Thinking", "Cache"),
        "-".repeat(110),
        format!("{:<40} | {:<12} | {:<12} | {:<6} | {:<6} | {:<8} | {:<6}",
            "anthropic/claude-3", "N/A", "N/A", "no", "yes", "no", "no"),
    ].join("\n");
    assert_eq!(report.render_text(), expected_text);
}

#[test]
fn test_tools_reports_contracts() {
    use serde_json::json;

    // ToolsListReport
    let t_list = ToolsListReport {
        tools: vec![ToolInfoEntry {
            name: "mock-tool".to_string(),
            description: "a mock tool for tests".to_string(),
            risk_type: "Low".to_string(),
        }],
    };
    assert_eq!(t_list.kind(), "tools.list");
    assert!(t_list.render_text().contains("mock-tool"));

    // ToolsInspectReport
    let t_inspect = ToolsInspectReport {
        name: "mock-tool".to_string(),
        schema: json!({"type": "object"}),
    };
    assert_eq!(t_inspect.kind(), "tools.inspect");
    assert!(t_inspect.render_text().contains("object"));

    // ToolsClassifyReport
    let t_classify = ToolsClassifyReport {
        command: "rm -rf /".to_string(),
        risk: gestalt_core::tool::RiskLevel::Critical,
    };
    assert_eq!(t_classify.kind(), "tools.classify");
    assert!(t_classify.render_text().contains("rm -rf /"));
    assert!(t_classify.render_text().contains("Critical"));
}

#[test]
fn test_auth_doctor_report_contract() {
    let auth_doc = AuthDoctorReport {
        entries: vec![AuthDoctorEntry {
            variable: "ANTHROPIC_API_KEY".to_string(),
            status: "present".to_string(),
            value: "[REDACTED]".to_string(),
        }],
    };
    assert_eq!(auth_doc.kind(), "auth.doctor");
    assert!(auth_doc.render_text().contains("ANTHROPIC_API_KEY"));
    assert!(auth_doc.render_text().contains("present"));
}

#[test]
fn test_global_doctor_report_contract() {
    let mut auths = std::collections::HashMap::new();
    auths.insert("openai".to_string(), "present".to_string());

    let ws_doctor = gestalt_cli::output::WorkspaceDoctorReport {
        workspace_root: std::path::PathBuf::from("/workspace"),
        config_valid: true,
        config_error: None,
        policies_valid: true,
        policies_error: None,
        missing_files: vec![],
        auth_summary: auths,
        run_dir_exists: true,
        run_dir_writable: Some(true),
    };
    let global_doc = GlobalDoctorReport {
        workspace_doctor: ws_doctor,
        live: false,
    };
    assert_eq!(global_doc.kind(), "doctor");
    let text = global_doc.render_text();
    assert!(text.contains("Configuration: valid"));
    assert!(text.contains("Policies: syntax valid"));
    assert!(text.contains("openai"));
    assert!(text.contains("PASS:"));
}

