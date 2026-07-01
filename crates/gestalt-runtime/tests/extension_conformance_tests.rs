use gestalt_runtime::unstable::extension::{
    ComponentKind, ExtensionManifestV2, ResolvedExtensionPackage,
};
use gestalt_runtime::unstable::lifecycle::protocol::{
    negotiate_protocol_version, InitializeRequestV2, LifecycleInvokeRequestV2,
};

#[test]
fn conformance_manifest_examples_parse_and_normalize() {
    let manifests = [
        lifecycle_manifest(),
        command_tool_manifest(),
        mcp_manifest(),
        optional_component_manifest(),
    ];

    for manifest in manifests {
        let parsed = ExtensionManifestV2::parse(&manifest).unwrap();
        parsed.validate().unwrap();
        let resolved = ResolvedExtensionPackage::from_v2_manifest(parsed, "primary").unwrap();
        assert!(!resolved.components.is_empty());
    }
}

#[test]
fn conformance_protocol_v2_fixtures_parse() {
    let valid: InitializeRequestV2 =
        serde_json::from_str(include_str!("fixtures/protocol-v2/valid-initialize.json")).unwrap();
    assert_eq!(
        negotiate_protocol_version(&valid.supported_versions),
        Some("2.0".to_string())
    );

    let unsupported: InitializeRequestV2 = serde_json::from_str(include_str!(
        "fixtures/protocol-v2/unsupported-protocol.json"
    ))
    .unwrap();
    assert_eq!(
        negotiate_protocol_version(&unsupported.supported_versions),
        None
    );

    let invoke: LifecycleInvokeRequestV2 = serde_json::from_str(include_str!(
        "fixtures/protocol-v2/context-contribution.json"
    ))
    .unwrap();
    assert_eq!(
        invoke.component_id,
        "component:com.example.review:primary:lifecycle"
    );
}

#[test]
fn command_and_mcp_components_have_expected_kinds() {
    let command = ResolvedExtensionPackage::from_v2_manifest(
        ExtensionManifestV2::parse(&command_tool_manifest()).unwrap(),
        "primary",
    )
    .unwrap();
    let mcp = ResolvedExtensionPackage::from_v2_manifest(
        ExtensionManifestV2::parse(&mcp_manifest()).unwrap(),
        "primary",
    )
    .unwrap();

    assert_eq!(command.components[0].kind, ComponentKind::CommandTool);
    assert_eq!(mcp.components[0].kind, ComponentKind::McpServer);
}

fn lifecycle_manifest() -> String {
    r#"
manifest_version = 2

[package]
id = "com.example.lifecycle"
name = "Lifecycle"
version = "1.0.0"

[[components]]
id = "lifecycle"
kind = "gestalt-lifecycle"

[components.entrypoint]
command = "python"
args = ["-m", "example"]
"#
    .to_string()
}

fn command_tool_manifest() -> String {
    r#"
manifest_version = 2

[package]
id = "com.example.tools"
name = "Tools"
version = "1.0.0"

[[components]]
id = "echo"
kind = "command-tool"
description = "Echo JSON"
input_schema = { type = "object" }
risk = "Low"
read_only = true
idempotent = true

[components.entrypoint]
command = "/bin/cat"
"#
    .to_string()
}

fn mcp_manifest() -> String {
    r#"
manifest_version = 2

[package]
id = "com.example.mcp"
name = "MCP"
version = "1.0.0"

[[components]]
id = "server"
kind = "mcp-server"

[components.entrypoint]
command = "node"
args = ["server.js"]
"#
    .to_string()
}

fn optional_component_manifest() -> String {
    r#"
manifest_version = 2

[package]
id = "com.example.optional"
name = "Optional"
version = "1.0.0"

[[components]]
id = "optional-lifecycle"
kind = "gestalt-lifecycle"
optional = true

[components.entrypoint]
command = "python"
args = ["-m", "optional"]
"#
    .to_string()
}
