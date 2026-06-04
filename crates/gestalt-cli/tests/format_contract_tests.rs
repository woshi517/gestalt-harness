use gestalt_cli::output::{
    CliReport, ConfigValidateReport, JsonEnvelope, ModelsListReport, ProvidersListReport,
    ReplayReport, RunReport, ToolsListReport, ToolsInspectReport, ToolsClassifyReport,
    AuthDoctorReport, GlobalDoctorReport, AuthDoctorEntry, ToolInfoEntry,
    ConnectReport, ProfilesListReport, ProvidersDoctorReport, ModelsSearchReport,
    ProfileInfoEntry, ProviderDoctorResult, RuntimeInspectReport,
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
        selected_model: Some("openai/gpt-4".to_string()),
        model_valid: true,
        model_error: None,
    };
    let global_doc = GlobalDoctorReport {
        workspace_doctor: ws_doctor,
        live: false,
    };
    assert_eq!(global_doc.kind(), "doctor");
    let text = global_doc.render_text();
    assert!(text.contains("Configuration: valid"));
    assert!(text.contains("Policies: syntax valid"));
    assert!(text.contains("Selected model 'openai/gpt-4': exists in catalog"));
    assert!(text.contains("openai"));
    assert!(text.contains("PASS:"));
}

#[test]
fn test_connect_report_contract() {
    let report = ConnectReport {
        provider: "openrouter".to_string(),
        status: "connected".to_string(),
        profile_created: Some("default".to_string()),
        keychain_stored: true,
    };
    assert_eq!(report.kind(), "connect");
    let text = report.render_text();
    assert!(text.contains("Connected to provider 'openrouter'"));
    assert!(text.contains("status=connected"));
    assert!(text.contains("profile_created=default"));
    assert!(text.contains("keychain_stored=true"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"connect""#));
}

#[test]
fn test_profiles_list_report_contract() {
    let entry = ProfileInfoEntry {
        name: "test-profile".to_string(),
        provider: "openrouter".to_string(),
        model: "openrouter/free".to_string(),
        active: true,
    };
    let report = ProfilesListReport {
        profiles: vec![entry],
    };
    assert_eq!(report.kind(), "profiles.list");
    let text = report.render_text();
    assert!(text.contains("test-profile"));
    assert!(text.contains("openrouter"));
    assert!(text.contains("openrouter/free"));
    assert!(text.contains("yes"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"profiles.list""#));
}

#[test]
fn test_providers_doctor_report_contract() {
    let result = ProviderDoctorResult {
        provider: "openrouter".to_string(),
        auth_variable: "OPENROUTER_API_KEY".to_string(),
        auth_status: "present".to_string(),
        auth_source: "env".to_string(),
    };
    let report = ProvidersDoctorReport {
        results: vec![result],
    };
    assert_eq!(report.kind(), "providers.doctor");
    let text = report.render_text();
    assert!(text.contains("provider=openrouter"));
    assert!(text.contains("status=present"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"providers.doctor""#));
}

#[test]
fn test_models_search_report_contract() {
    let m = ModelInfo {
        qualified_id: "openrouter/free".to_string(),
        model_id: "free".to_string(),
        display_name: "Google: Gemini 2.5 Flash (free)".to_string(),
        max_context_tokens: 1048576,
        max_output_tokens: 8192,
        supports_tools: true,
        supports_vision: true,
        supports_json_schema: true,
        supports_thinking: false,
        supports_prompt_caching: false,
        input_cost_per_million: Some(0.0),
        output_cost_per_million: Some(0.0),
        source: ModelInfoSource::BuiltIn,
        last_updated: None,
    };
    let report = ModelsSearchReport {
        models: vec![m],
    };
    assert_eq!(report.kind(), "models.search");
    let text = report.render_text();
    assert!(text.contains("openrouter/free"));
    assert!(text.contains("Gemini 2.5 Flash"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"models.search""#));
}

#[test]
fn test_runtime_inspect_report_contract() {
    let inspect = gestalt_runtime::RuntimeInspect {
        provider_name: "anthropic".to_string(),
        provider_model: "claude-3-5-sonnet".to_string(),
        execution_mode: "Confirm".to_string(),
        max_turns: 15,
        context_pipeline_version: "pipeline-v1".to_string(),
        tools: vec![gestalt_runtime::ToolInspectInfo {
            name: "bash".to_string(),
            schema_hash: "abc123hash".to_string(),
        }],
        tool_schema_hash: "def456hash".to_string(),
        policy_fingerprint: Some("policy789hash".to_string()),
        policy_source_path: Some("/policies.toml".to_string()),
        hooks: vec!["VerificationToolHook".to_string()],
        hook_contract_hash: "hookhash123".to_string(),
        verifiers: vec!["FileExistsVerifier".to_string()],
        extensions: vec!["MockExtension".to_string()],
        trace_sink_kind: Some("JsonlTraceSink".to_string()),
        trace_run_dir: None,
        workspace_root: "/workspace".to_string(),
        enabled_cli_features: vec!["tui".to_string()],
    };

    let report = RuntimeInspectReport { inspect };
    assert_eq!(report.kind(), "runtime.inspect");
    let text = report.render_text();
    assert!(text.contains("Provider Connection: anthropic"));
    assert!(text.contains("Provider Model:      claude-3-5-sonnet"));
    assert!(text.contains("Execution Mode:      Confirm"));
    assert!(text.contains("Max Turns Limit:     15"));
    assert!(text.contains("Workspace Root:      /workspace"));
    assert!(text.contains("Policy Source:       /policies.toml"));
    assert!(text.contains("MockExtension"));
    assert!(text.contains("FileExistsVerifier"));
    assert!(text.contains("VerificationToolHook"));
    assert!(text.contains("bash"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"runtime.inspect""#));
    assert!(serialized.contains(r#""provider_name":"anthropic""#));
    assert!(serialized.contains(r#""provider_model":"claude-3-5-sonnet""#));
}

