use gestalt_cli::output::{
    CliReport, ConfigValidateReport, JsonEnvelope, ModelsListReport, ProvidersListReport,
    ReplayReport, RunReport,
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
    assert_eq!(report.render_text(), "anthropic/claude-3");
}
