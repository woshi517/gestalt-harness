#![cfg(feature = "full")]

use std::{collections::HashMap, path::PathBuf};

use gestalt_app::config::ConfigSourceInfo;
use gestalt_cli::output::{
    CliErrorPayload, CliReport, ConfigValidateReport, JsonEnvelope, ModelsInspectReport,
    ModelsListReport, PolicyExplainReport, ProfileInfoEntry, ProfilesInspectReport,
    ProfilesListReport, ProvidersInspectReport, ProvidersListReport, ToolInfoEntry,
    ToolsInspectReport, ToolsListReport, WorkspaceInfoReport,
};
use gestalt_core::{
    event::PolicyStatus,
    model::ModelInfo,
    model::ModelInfoSource,
    policy::PolicyDecision,
    tool::RiskLevel,
    tool_descriptor::{AnnotationSource, ToolAnnotation, ToolAnnotations},
};
use serde::Serialize;
use serde_json::{json, Value};

fn success_json<T>(report: &T) -> Value
where
    T: CliReport + Serialize,
{
    serde_json::to_value(JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: report,
    })
    .expect("serialize success envelope")
}

fn error_json(code: &str, message: &str, retryable: bool) -> Value {
    serde_json::to_value(JsonEnvelope {
        schema_version: 1,
        kind: "error".to_string(),
        data: CliErrorPayload {
            code: code.to_string(),
            message: message.to_string(),
            retryable,
            details: None,
            correlation_id: None,
        },
    })
    .expect("serialize error envelope")
}

fn snapshot_string<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).expect("pretty JSON snapshot")
}

fn sample_model(qualified_id: &str, display_name: &str) -> ModelInfo {
    ModelInfo {
        qualified_id: qualified_id.to_string(),
        model_id: qualified_id.rsplit_once('/').map_or_else(
            || qualified_id.to_string(),
            |(_, model_id)| model_id.to_string(),
        ),
        display_name: display_name.to_string(),
        max_context_tokens: 128_000,
        max_output_tokens: 8192,
        supports_tools: true,
        supports_vision: false,
        supports_json_schema: true,
        supports_thinking: false,
        supports_prompt_caching: true,
        input_cost_per_million: Some(1.25),
        output_cost_per_million: Some(5.0),
        source: ModelInfoSource::BuiltIn,
        last_updated: None,
    }
}

#[test]
fn config_validate_snapshots() {
    let report = ConfigValidateReport {
        workspace_root: PathBuf::from("/workspace"),
    };

    insta::assert_snapshot!(
        "config_validate_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "config_validate_error",
        snapshot_string(&error_json(
            "UNSUPPORTED_LEGACY_CONFIG",
            "legacy TOML configuration is not supported",
            false,
        ))
    );
}

#[test]
fn config_explain_snapshots() {
    let mut explain_map = HashMap::new();
    explain_map.insert(
        "defaults.provider".to_string(),
        ConfigSourceInfo {
            value: json!("openrouter"),
            source: "Default".to_string(),
            winning_layer: "default".to_string(),
            source_location: None,
            defaulted: true,
            overridden: false,
            redacted: false,
        },
    );
    explain_map.insert(
        "providers.openai".to_string(),
        ConfigSourceInfo {
            value: json!({
                "id": "openai",
                "display_name": "OpenAI",
                "protocol": "openai",
                "api_format": "openai_responses",
                "base_url": "https://api.openai.com/v1",
                "default_model": "gpt-4o-mini",
                "api_key": "sk-test-secret",
                "api_key_env": "OPENAI_API_KEY",
                "auth_ref": "keychain:gestalt/openai",
                "headers": {
                    "Authorization": "Bearer secret",
                    "X-Trace": "trace-123"
                }
            }),
            source: "Workspace Config File".to_string(),
            winning_layer: "workspace".to_string(),
            source_location: Some("/workspace/gestalt.json".to_string()),
            defaulted: false,
            overridden: true,
            redacted: false,
        },
    );

    let report = gestalt_cli::output::ConfigExplainReport { explain_map };

    insta::assert_snapshot!(
        "config_explain_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "config_explain_error",
        snapshot_string(&error_json(
            "CONFIG_ERROR",
            "invalid configuration value",
            false,
        ))
    );
}

#[test]
fn workspace_info_snapshots() {
    let report = WorkspaceInfoReport {
        workspace_root: PathBuf::from("/workspace"),
        config_path: PathBuf::from("/workspace/gestalt.json"),
        workspace_md_path: PathBuf::from("/workspace/.gestalt/workspace.md"),
        memory_md_path: PathBuf::from("/workspace/.gestalt/memory.md"),
    };

    insta::assert_snapshot!(
        "workspace_info_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "workspace_info_error",
        snapshot_string(&error_json(
            "CONFIG_ERROR",
            "workspace configuration is invalid",
            false
        ))
    );
}

#[test]
fn providers_list_snapshots() {
    let report = ProvidersListReport {
        providers: vec![
            "anthropic".to_string(),
            "openai".to_string(),
            "openai-compatible".to_string(),
        ],
    };

    insta::assert_snapshot!(
        "providers_list_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "providers_list_error",
        snapshot_string(&error_json(
            "PROVIDER_ERROR",
            "provider catalog unavailable",
            true
        ))
    );
}

#[test]
fn providers_inspect_snapshots() {
    let report = ProvidersInspectReport {
        provider: "openai".to_string(),
        config: json!({
            "id": "openai",
            "display_name": "OpenAI",
            "protocol": "openai",
            "api_format": "openai_responses",
            "base_url": "https://api.openai.com/v1",
            "default_model": "gpt-4o-mini",
            "api_key": null,
            "api_key_env": "OPENAI_API_KEY",
            "auth_ref": "keychain:gestalt/openai",
            "models_endpoint": "https://api.openai.com/v1/models",
            "headers": {
                "X-Trace": "trace-123"
            }
        }),
    };

    insta::assert_snapshot!(
        "providers_inspect_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "providers_inspect_error",
        snapshot_string(&error_json(
            "PROVIDER_NOT_FOUND",
            "provider 'missing' not found",
            false
        ))
    );
}

#[test]
fn profiles_list_snapshots() {
    let report = ProfilesListReport {
        profiles: vec![
            ProfileInfoEntry {
                name: "default".to_string(),
                provider: "openai".to_string(),
                model: "gpt-4o-mini".to_string(),
                active: true,
            },
            ProfileInfoEntry {
                name: "research".to_string(),
                provider: "anthropic".to_string(),
                model: "claude-3-5-sonnet-20241022".to_string(),
                active: false,
            },
        ],
    };

    insta::assert_snapshot!(
        "profiles_list_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "profiles_list_error",
        snapshot_string(&error_json(
            "CONFIG_ERROR",
            "profile catalog unavailable",
            false
        ))
    );
}

#[test]
fn profiles_inspect_snapshots() {
    let report = ProfilesInspectReport {
        name: "default".to_string(),
        provider: "openai".to_string(),
        model: "gpt-4o-mini".to_string(),
        active: true,
        resolved_provider_kind: "openai".to_string(),
        resolved_base_url: Some("https://api.openai.com/v1".to_string()),
        resolved_auth_ref: Some("keychain:gestalt/openai".to_string()),
        resolved_api_key_env: None,
    };

    insta::assert_snapshot!(
        "profiles_inspect_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "profiles_inspect_error",
        snapshot_string(&error_json(
            "CONFIG_ERROR",
            "profile 'missing' not found",
            false
        ))
    );
}

#[test]
fn models_list_snapshots() {
    let report = ModelsListReport {
        models: vec![
            sample_model("openai/gpt-4o-mini", "GPT-4o Mini"),
            sample_model("anthropic/claude-3-5-sonnet-20241022", "Claude 3.5 Sonnet"),
        ],
    };

    insta::assert_snapshot!(
        "models_list_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "models_list_error",
        snapshot_string(&error_json(
            "PROVIDER_ERROR",
            "model catalog unavailable",
            true
        ))
    );
}

#[test]
fn models_inspect_snapshots() {
    let report = ModelsInspectReport {
        model: sample_model("openai/gpt-4o-mini", "GPT-4o Mini"),
    };

    insta::assert_snapshot!(
        "models_inspect_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "models_inspect_error",
        snapshot_string(&error_json(
            "CONFIG_ERROR",
            "unknown model: missing/model",
            false
        ))
    );
}

#[test]
fn policy_explain_snapshots() {
    let report = PolicyExplainReport {
        tool: "bash".to_string(),
        input: json!({"command": "echo hello"}),
        mode: "confirm".to_string(),
        risk: RiskLevel::Low,
        decision: PolicyDecision {
            status: PolicyStatus::Allowed,
            reason: Some("allowlisted command".to_string()),
            policy_source: "default allow".to_string(),
        },
    };

    insta::assert_snapshot!(
        "policy_explain_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "policy_explain_error",
        snapshot_string(&error_json(
            "POLICY_ERROR",
            "policy evaluation failed",
            false
        ))
    );
}

#[test]
fn tools_list_snapshots() {
    let report = ToolsListReport {
        tools: vec![
            ToolInfoEntry {
                name: "read".to_string(),
                description: "Read a file".to_string(),
                risk_type: "Low".to_string(),
            },
            ToolInfoEntry {
                name: "write".to_string(),
                description: "Write a file".to_string(),
                risk_type: "Medium".to_string(),
            },
        ],
    };

    insta::assert_snapshot!(
        "tools_list_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "tools_list_error",
        snapshot_string(&error_json("TOOL_ERROR", "tool catalog unavailable", true))
    );
}

#[test]
fn tools_inspect_snapshots() {
    let report = ToolsInspectReport {
        name: "read".to_string(),
        schema: json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"}
            }
        }),
        risk: RiskLevel::Low,
        annotations: ToolAnnotations::new(vec![ToolAnnotation {
            key: "trusted".to_string(),
            value: "true".to_string(),
            source: AnnotationSource::BuiltInTrusted,
        }]),
    };

    insta::assert_snapshot!(
        "tools_inspect_success",
        snapshot_string(&success_json(&report))
    );
    insta::assert_snapshot!(
        "tools_inspect_error",
        snapshot_string(&error_json(
            "TOOL_NOT_FOUND",
            "tool 'missing' not found",
            false
        ))
    );
}
