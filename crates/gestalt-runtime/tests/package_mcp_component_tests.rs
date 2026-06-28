use std::collections::HashMap;

use gestalt_runtime::{McpLifecycleMode, McpServerConfig, McpTransportConfig};
use gestalt_runtime::extension::{
    merge_mcp_server_configs, package_mcp_server_name, package_mcp_servers, ExtensionManifestV2,
    ResolvedExtensionPackage,
};

#[test]
fn package_mcp_component_normalizes_to_canonical_server_name() {
    let package = mcp_package("server", "node", &["server.js"]);
    let servers = package_mcp_servers(&package).unwrap();
    let name = package_mcp_server_name("com.example.mcp", "primary", "server");
    let config = servers.get(&name).unwrap();

    assert_eq!(config.name, name);
    assert_eq!(config.lifecycle, McpLifecycleMode::Lazy);
    assert_eq!(
        config.transport,
        McpTransportConfig::Stdio {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            cwd: None,
            env: HashMap::new()
        }
    );
}

#[test]
fn direct_and_package_mcp_configs_coexist() {
    let package = mcp_package("server", "node", &["server.js"]);
    let direct = HashMap::from([("direct".to_string(), direct_mcp_config("direct"))]);

    let merged = merge_mcp_server_configs(direct, &[package]).unwrap();

    assert!(merged.contains_key("direct"));
    assert!(merged.contains_key(&package_mcp_server_name(
        "com.example.mcp",
        "primary",
        "server"
    )));
}

#[test]
fn mcp_server_name_collisions_fail_candidate_construction() {
    let package = mcp_package("server", "node", &["server.js"]);
    let name = package_mcp_server_name("com.example.mcp", "primary", "server");
    let direct = HashMap::from([(name.clone(), direct_mcp_config(&name))]);

    let err = merge_mcp_server_configs(direct, &[package]).unwrap_err();

    assert!(err.to_string().contains("collision"));
}

fn mcp_package(component_id: &str, command: &str, args: &[&str]) -> ResolvedExtensionPackage {
    let args_toml = args
        .iter()
        .map(|arg| format!("\"{arg}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let manifest = ExtensionManifestV2::parse(&format!(
        r#"
manifest_version = 2

[package]
id = "com.example.mcp"
name = "Example MCP"
version = "1.0.0"

[[components]]
id = "{component_id}"
kind = "mcp-server"

[components.entrypoint]
command = "{command}"
args = [{args_toml}]
"#
    ))
    .unwrap();
    ResolvedExtensionPackage::from_v2_manifest(manifest, "primary").unwrap()
}

fn direct_mcp_config(name: &str) -> McpServerConfig {
    McpServerConfig {
        name: name.to_string(),
        enabled: true,
        transport: McpTransportConfig::Stdio {
            command: "node".to_string(),
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
        },
        lifecycle: McpLifecycleMode::Lazy,
        trust_level: None,
        allow_sampling: false,
        env: HashMap::new(),
        tool_annotations: HashMap::new(),
        timeouts: None,
        display_name: None,
    }
}
