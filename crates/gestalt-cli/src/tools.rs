use gestalt_core::{error::HarnessError, ToolCatalog};
use gestalt_tools::default_registry;
use serde_json::Value;

use crate::config::CliOverrides;
use crate::output::{ToolInfoEntry, ToolsClassifyReport, ToolsInspectReport, ToolsListReport};

pub fn list_tools(
    _overrides: &CliOverrides,
) -> Result<ToolsListReport, Box<dyn std::error::Error>> {
    let registry = default_registry()?;
    let mut tools = Vec::new();

    for schema in registry.schemas() {
        let name = schema
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(tool) = registry.get(name) {
            let risk = tool.risk(&Value::Null);
            tools.push(ToolInfoEntry {
                name: name.to_string(),
                description: tool.description().to_string(),
                risk_type: format!("{:?}", risk),
            });
        }
    }

    Ok(ToolsListReport { tools })
}

pub fn inspect_tool(
    _overrides: &CliOverrides,
    tool_name: &str,
) -> Result<ToolsInspectReport, Box<dyn std::error::Error>> {
    let registry = default_registry()?;
    let tool = registry.get(tool_name).ok_or_else(|| {
        HarnessError::Tool(gestalt_core::error::ToolError::NotFound(
            tool_name.to_string(),
        ))
    })?;

    Ok(ToolsInspectReport {
        name: tool_name.to_string(),
        schema: tool.schema(),
    })
}

pub fn classify_bash(
    _overrides: &CliOverrides,
    command: &[String],
) -> Result<ToolsClassifyReport, Box<dyn std::error::Error>> {
    let full_command = command.join(" ");
    let risk = gestalt_policy::classify_bash(&full_command);

    Ok(ToolsClassifyReport {
        command: full_command,
        risk,
    })
}
