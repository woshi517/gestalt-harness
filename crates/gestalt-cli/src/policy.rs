use crate::config::{load_effective_config, CliOverrides};
use crate::output::{PolicyExplainReport, PolicyTestReport, PolicyValidateReport};
use gestalt_core::policy::{PolicyEngine, PolicyRequest};
use gestalt_core::ToolCatalog;
use gestalt_core::{tool::RiskLevel, HarnessError};
use gestalt_policy::{MinimalPolicyEngine, PolicyConfig};
use serde_json::Value;

pub fn validate_policy(overrides: &CliOverrides) -> Result<PolicyValidateReport, HarnessError> {
    let config = load_effective_config(overrides)?;
    let policy_path = config.workspace_file("policies.toml");
    if !policy_path.exists() {
        return Ok(PolicyValidateReport {
            path: policy_path,
            valid: false,
            error: Some("policies.toml does not exist".to_string()),
        });
    }

    match PolicyConfig::from_file(&policy_path) {
        Ok(_) => Ok(PolicyValidateReport {
            path: policy_path,
            valid: true,
            error: None,
        }),
        Err(err) => Ok(PolicyValidateReport {
            path: policy_path,
            valid: false,
            error: Some(err.to_string()),
        }),
    }
}

fn get_tool_risk(tool_name: &str, input: &Value) -> RiskLevel {
    if tool_name == "bash" {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        gestalt_policy::classify_bash(command)
    } else if let Ok(registry) = gestalt_tools::default_registry() {
        if let Some(tool) = registry.get(tool_name) {
            tool.risk(input)
        } else {
            RiskLevel::Medium
        }
    } else {
        RiskLevel::Medium
    }
}

fn validate_tool_input(tool_name: &str, input: &Value) -> Result<(), Box<dyn std::error::Error>> {
    if tool_name == "bash" {
        if !input.is_object() {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Bash tool input must be a JSON object",
            )));
        }
        let command = input.get("command").and_then(Value::as_str);
        match command {
            Some(cmd) if !cmd.trim().is_empty() => {}
            _ => {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Bash tool input must contain a non-empty 'command' string",
                )));
            }
        }
    }
    Ok(())
}

pub async fn explain_policy(
    overrides: &CliOverrides,
    tool_name: &str,
    input_str: &str,
) -> Result<PolicyExplainReport, Box<dyn std::error::Error>> {
    let config = load_effective_config(overrides)?;
    let input: Value = serde_json::from_str(input_str)?;
    validate_tool_input(tool_name, &input)?;
    let mode = config.selected_mode()?;
    let risk = get_tool_risk(tool_name, &input);

    let policy_path = config.workspace_file("policies.toml");
    let policy_config = if policy_path.exists() {
        PolicyConfig::from_file(&policy_path)?
    } else {
        PolicyConfig::default()
    };

    let engine = MinimalPolicyEngine::new(policy_config);
    let request = PolicyRequest {
        tool_call_id: "explain-call-id".to_string(),
        tool_name: tool_name.to_string(),
        namespace: gestalt_core::tool_descriptor::ToolNamespace::BuiltIn,
        annotations: gestalt_core::tool_descriptor::ToolAnnotations::default(),
        input: input.clone(),
        risk,
        mode,
        working_dir: config.workspace_root.clone(),
        workspace_root: Some(config.workspace_root.clone()),
        user_approved: false,
    };

    let decision = engine.evaluate(request).await;

    Ok(PolicyExplainReport {
        tool: tool_name.to_string(),
        input,
        mode: format!("{:?}", mode).to_lowercase(),
        risk,
        decision,
    })
}

pub async fn test_policy(
    overrides: &CliOverrides,
    tool_name: &str,
    input_str: &str,
    override_mode: Option<&str>,
) -> Result<PolicyTestReport, Box<dyn std::error::Error>> {
    let config = load_effective_config(overrides)?;
    let input: Value = serde_json::from_str(input_str)?;
    validate_tool_input(tool_name, &input)?;
    let mode = if let Some(m) = override_mode {
        crate::config::mode_from_str(m)?
    } else {
        config.selected_mode()?
    };
    let risk = get_tool_risk(tool_name, &input);

    let policy_path = config.workspace_file("policies.toml");
    let policy_config = if policy_path.exists() {
        PolicyConfig::from_file(&policy_path)?
    } else {
        PolicyConfig::default()
    };

    let engine = MinimalPolicyEngine::new(policy_config);
    let request = PolicyRequest {
        tool_call_id: "test-call-id".to_string(),
        tool_name: tool_name.to_string(),
        namespace: gestalt_core::tool_descriptor::ToolNamespace::BuiltIn,
        annotations: gestalt_core::tool_descriptor::ToolAnnotations::default(),
        input: input.clone(),
        risk,
        mode,
        working_dir: config.workspace_root.clone(),
        workspace_root: Some(config.workspace_root.clone()),
        user_approved: false,
    };

    let decision = engine.evaluate(request).await;

    Ok(PolicyTestReport {
        tool: tool_name.to_string(),
        input,
        mode: format!("{:?}", mode).to_lowercase(),
        risk,
        decision,
    })
}
