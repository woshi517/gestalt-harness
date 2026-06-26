#![allow(deprecated)]

use gestalt_core::session::ExecutionMode;
use gestalt_core::{
    approval::AutoApprovalProvider,
    context::TokenBudget,
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolSchema},
};
use gestalt_runtime::{AgentRuntimeBuilder, ExtensionManifest, RuntimeConfig};
use std::sync::Arc;

#[test]
fn test_runtime_config_defaults() {
    let config = RuntimeConfig::default();
    assert_eq!(config.execution_mode, ExecutionMode::Confirm);
    assert!(config.max_turns > 0);
}

#[test]
fn test_builder_missing_dependencies() {
    let builder = AgentRuntimeBuilder::new();
    let res = builder.build();
    assert!(res.is_err());
    let err_str = format!("{:?}", res.err().unwrap());
    assert!(err_str.contains("Missing provider") || err_str.contains("Builder"));
}

#[test]
fn test_builder_zero_max_turns() {
    let mut config = RuntimeConfig::default();
    config.max_turns = 0;
    let builder = AgentRuntimeBuilder::new().config(config);
    let res = builder.build();
    assert!(res.is_err());
    let err_str = format!("{:?}", res.err().unwrap());
    assert!(err_str.contains("max_turns must be positive"));
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
            supports_tools: false,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: false,
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

struct MockToolCatalog;
impl ToolCatalog for MockToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }
    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        None
    }
}

struct InspectToolCatalog {
    tool: Arc<dyn gestalt_core::tool::Tool>,
}

impl ToolCatalog for InspectToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        vec![self.tool.schema()]
    }

    fn get(&self, name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        if name == self.tool.name() {
            Some(self.tool.clone())
        } else {
            None
        }
    }
}

struct SnapshotTool;

#[async_trait::async_trait]
impl gestalt_core::tool::Tool for SnapshotTool {
    fn name(&self) -> &str {
        "snapshot-tool"
    }

    fn description(&self) -> &str {
        "snapshot tool"
    }

    fn schema(&self) -> ToolSchema {
        serde_json::from_value(serde_json::json!({
            "name": "snapshot-tool",
            "description": "snapshot tool",
            "input_schema": { "type": "object", "properties": {} }
        }))
        .unwrap()
    }

    fn risk(&self, _input: &serde_json::Value) -> gestalt_core::tool::RiskLevel {
        gestalt_core::tool::RiskLevel::Low
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &gestalt_core::tool::ToolContext,
    ) -> Result<gestalt_core::tool::ToolOutput, gestalt_core::error::ToolError> {
        Ok(gestalt_core::tool::ToolOutput::Text {
            content: "ok".to_string(),
        })
    }
}

struct MockPolicyEngine;
#[async_trait::async_trait]
impl PolicyEngine for MockPolicyEngine {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

struct LegacyContextPipeline;
impl gestalt_core::ContextPipeline for LegacyContextPipeline {
    fn process(
        &self,
        _history: &[gestalt_core::SessionMessage],
        _budget: &TokenBudget,
    ) -> Vec<Message> {
        Vec::new()
    }

    fn version(&self) -> &str {
        "legacy-v1"
    }
}

#[test]
fn test_builder_rejects_legacy_pipeline_without_assembler() {
    let res = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .middleware(Arc::new(LegacyContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .build();

    let err = match res {
        Ok(_) => panic!("legacy-only pipelines should fail during builder validation"),
        Err(err) => err,
    };
    assert!(err
        .to_string()
        .contains("runtime requires an assembler-backed context pipeline"));
}

#[test]
fn test_builder_publishes_registry_snapshot() {
    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .build()
        .unwrap();

    assert!(runtime.registry_snapshot.tools.contains_key("search_tools"));
    assert_eq!(
        runtime.registry_snapshot.fingerprint,
        runtime.registry.snapshot().fingerprint
    );
    assert_eq!(
        runtime.extension_manager.current_generation(),
        gestalt_runtime::extension::RuntimeGeneration(0)
    );
    assert_eq!(
        runtime.extension_manager.active_snapshot().fingerprint,
        runtime.registry_snapshot.fingerprint
    );
}

#[test]
fn test_inspect_reads_tool_catalog_from_pinned_snapshot() {
    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(InspectToolCatalog {
            tool: Arc::new(SnapshotTool),
        }))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .build()
        .unwrap();

    let mut runtime = runtime;
    runtime.tools = Arc::new(MockToolCatalog);

    let inspect = runtime.inspect();
    assert!(inspect
        .tools
        .iter()
        .any(|tool| tool.name == "snapshot-tool"));
}

#[test]
fn test_sync_builder_rejects_pending_process_extensions() {
    let manifest = mock_extension_manifest();
    let res = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .process_extension(manifest, "hash-a".to_string(), true)
        .build();

    let err = match res {
        Ok(_) => panic!("sync build should reject external process activation"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("build_async"));
}

#[tokio::test]
async fn test_async_builder_launches_process_extensions_through_manager() {
    let manifest = mock_extension_manifest();
    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .process_extension(manifest, "hash-a".to_string(), true)
        .build_async()
        .await
        .unwrap();

    assert!(runtime
        .extension_snapshot
        .registry_snapshot
        .extensions
        .contains(&"mock-ext@mock-ext".to_string()));
    assert_eq!(
        runtime.extension_manager.process_instances()[0].state(),
        gestalt_runtime::extension::ExtensionProcessState::Ready
    );
    assert!(runtime
        .extension_snapshot
        .resolved_packages
        .iter()
        .any(|pkg| pkg.descriptor.id == "mock-ext"));
}

#[test]
fn test_sync_builder_activates_v1_extension_packages_through_pipeline() {
    let manifest = mock_extension_manifest();
    let package =
        gestalt_runtime::extension::ResolvedExtensionPackage::from_v1_manifest(manifest).unwrap();

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .extension_package(package)
        .config(RuntimeConfig::default())
        .build()
        .unwrap();

    assert!(runtime
        .extension_snapshot
        .resolved_packages
        .iter()
        .any(|pkg| pkg.descriptor.id == "mock-ext"));
    assert_eq!(runtime.extension_manager.process_instances().len(), 1);
    assert_eq!(
        runtime.extension_manager.process_instances()[0].state(),
        gestalt_runtime::extension::ExtensionProcessState::Ready
    );
}

fn mock_extension_manifest() -> ExtensionManifest {
    let manifest_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/extensions/mock-ext/gestalt.extension.toml");
    let content = std::fs::read_to_string(manifest_path).unwrap();
    ExtensionManifest::parse(&content).unwrap()
}
