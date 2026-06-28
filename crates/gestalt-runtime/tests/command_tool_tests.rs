use std::sync::Arc;
use std::time::Duration;

use gestalt_core::{
    approval::AutoApprovalProvider,
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{Tool, ToolCatalog, ToolContext, ToolOutput, ToolSchema},
};
use gestalt_runtime::extension::{CommandTool, ExtensionManifestV2, ResolvedExtensionPackage};
use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig};

struct EmptyToolCatalog;

impl ToolCatalog for EmptyToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }

    fn get(&self, _name: &str) -> Option<Arc<dyn Tool>> {
        None
    }
}

struct MockProvider;

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }

    fn display_name(&self) -> &str {
        "Mock"
    }

    fn default_model(&self) -> &str {
        "mock-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        static CAP: ProviderCapabilities = ProviderCapabilities {
            supports_tools: true,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: true,
            supports_prompt_caching: false,
            supports_usage_reporting: false,
            supports_streaming: true,
            supports_strict_schema: false,
        };
        &CAP
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(0)
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

struct MockPolicyEngine;

#[async_trait::async_trait]
impl PolicyEngine for MockPolicyEngine {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

#[tokio::test]
async fn command_tool_returns_structured_json() {
    let package = command_tool_package("/bin/cat", &[]);
    let tool = CommandTool::from_component(
        &package.components[0],
        std::path::PathBuf::from("."),
        gestalt_runtime::event_bus::RuntimeEventBus::new(),
    )
    .unwrap();

    let output = tool
        .execute(serde_json::json!({ "message": "hello" }), &tool_context())
        .await
        .unwrap();

    assert_eq!(
        output,
        ToolOutput::Json {
            value: serde_json::json!({ "message": "hello" })
        }
    );
    assert_eq!(
        tool.descriptor().id.to_string(),
        "extension:com.example.tools@primary:echo"
    );
}

#[tokio::test]
async fn command_tool_invalid_json_output_is_execution_error() {
    let package = command_tool_package("/bin/echo", &["not-json"]);
    let tool = CommandTool::from_component(
        &package.components[0],
        std::path::PathBuf::from("."),
        gestalt_runtime::event_bus::RuntimeEventBus::new(),
    )
    .unwrap();

    let err = tool
        .execute(serde_json::json!({ "message": "hello" }), &tool_context())
        .await
        .unwrap_err();

    assert!(err.to_string().contains("expected ident"));
}

#[test]
fn builder_registers_command_tool_components_as_tools() {
    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_runtime::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .extension_package(command_tool_package("/bin/cat", &[]))
        .build()
        .unwrap();

    assert!(runtime.tools.get("primary__echo").is_some());
    assert!(runtime
        .registry_snapshot
        .tools
        .contains_key("primary__echo"));
}

#[test]
fn builder_keeps_same_component_names_unique_across_instances() {
    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_runtime::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .extension_package(command_tool_package("/bin/cat", &[]))
        .extension_package(command_tool_package_with_instance(
            "secondary",
            "/bin/cat",
            &[],
        ))
        .build()
        .unwrap();

    let primary = runtime
        .tools
        .get("extension:com.example.tools@primary:echo")
        .unwrap();
    let secondary = runtime
        .tools
        .get("extension:com.example.tools@secondary:echo")
        .unwrap();

    assert_ne!(
        primary.descriptor().id.to_string(),
        secondary.descriptor().id.to_string()
    );
    assert!(runtime
        .registry_snapshot
        .tools
        .contains_key("primary__echo"));
    assert!(runtime
        .registry_snapshot
        .tools
        .contains_key("secondary__echo"));
}

fn command_tool_package(command: &str, args: &[&str]) -> ResolvedExtensionPackage {
    command_tool_package_with_instance("primary", command, args)
}

fn command_tool_package_with_instance(
    instance_id: &str,
    command: &str,
    args: &[&str],
) -> ResolvedExtensionPackage {
    let args_toml = args
        .iter()
        .map(|arg| format!("\"{arg}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let manifest = ExtensionManifestV2::parse(&format!(
        r#"
manifest_version = 2

[package]
id = "com.example.tools"
name = "Example Tools"
version = "1.0.0"

[[components]]
id = "echo"
kind = "command-tool"
description = "Echo JSON"
input_schema = {{ type = "object" }}
risk = "Low"
read_only = true
idempotent = true

[components.entrypoint]
command = "{command}"
args = [{args_toml}]

[components.permissions]
allow_shell = true
"#
    ))
    .unwrap();
    let grants = gestalt_runtime::extension::ExtensionGrantConfig {
        shell: true,
        ..Default::default()
    };
    ResolvedExtensionPackage::from_v2_manifest(manifest, instance_id)
        .unwrap()
        .with_instance(instance_id, serde_json::Value::Null, grants)
}

fn tool_context() -> ToolContext {
    ToolContext {
        working_dir: std::env::current_dir().unwrap(),
        workspace_root: None,
        timeout: Duration::from_secs(1),
        allow_network: false,
        environment: Default::default(),
        max_output_bytes: 4096,
        artifact_dir: None,
        current_tool_call_id: None,
        ignore_patterns: Vec::new(),
    }
}
