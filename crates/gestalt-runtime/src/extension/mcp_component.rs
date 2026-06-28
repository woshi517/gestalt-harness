use std::collections::HashMap;
use std::hash::BuildHasher;

use crate::legacy_mcp::{McpLifecycleMode, McpServerConfig, McpTransportConfig};

use crate::error::{Result, RuntimeError};

use super::{ComponentKind, ResolvedExtensionPackage};

pub fn package_mcp_server_name(package_id: &str, instance_id: &str, component_id: &str) -> String {
    format!("package.{package_id}.{instance_id}.{component_id}")
}

pub fn package_mcp_servers(
    package: &ResolvedExtensionPackage,
) -> Result<HashMap<String, McpServerConfig>> {
    let mut servers = HashMap::new();
    for component in &package.components {
        if component.kind != ComponentKind::McpServer {
            continue;
        }
        let name = package_mcp_server_name(
            &component.id.package_id,
            &component.id.instance_id,
            &component.id.component_id,
        );
        let config = McpServerConfig {
            name: name.clone(),
            enabled: true,
            transport: McpTransportConfig::Stdio {
                command: component.entrypoint.command.clone(),
                args: component.entrypoint.args.clone(),
                cwd: None,
                env: HashMap::new(),
            },
            lifecycle: McpLifecycleMode::Lazy,
            trust_level: None,
            allow_sampling: false,
            env: HashMap::new(),
            tool_annotations: HashMap::new(),
            timeouts: None,
            display_name: Some(package.descriptor.name.clone()),
        };
        if servers.insert(name.clone(), config).is_some() {
            return Err(RuntimeError::Extension(format!(
                "Duplicate package MCP server '{name}'"
            )));
        }
    }
    Ok(servers)
}

pub fn merge_mcp_server_configs<S: BuildHasher>(
    direct: HashMap<String, McpServerConfig, S>,
    packages: &[ResolvedExtensionPackage],
) -> Result<HashMap<String, McpServerConfig>> {
    let mut merged: HashMap<String, McpServerConfig> = direct.into_iter().collect();
    for package in packages {
        for (name, config) in package_mcp_servers(package)? {
            if merged.contains_key(&name) {
                return Err(RuntimeError::Extension(format!(
                    "MCP server name collision for '{name}'"
                )));
            }
            merged.insert(name, config);
        }
    }
    Ok(merged)
}
