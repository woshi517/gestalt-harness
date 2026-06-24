use std::sync::Arc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolSchema},
};
use gestalt_runtime::{
    AgentRuntimeBuilder, ReloadExtensionsRequest, RuntimeConfig, RuntimeControl,
};

struct EmptyToolCatalog;

impl ToolCatalog for EmptyToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }

    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
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

struct MockPolicyEngine;

#[async_trait::async_trait]
impl PolicyEngine for MockPolicyEngine {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

#[tokio::test]
async fn successful_reload_increments_generation_and_changes_fingerprint() {
    let runtime = runtime();
    let before = runtime.extension_manager.active_snapshot();

    let report = runtime
        .reload_extensions(ReloadExtensionsRequest::default())
        .await
        .unwrap();
    let after = runtime.extension_manager.active_snapshot();

    assert_eq!(report.previous_generation, before.generation);
    assert_eq!(after.generation.0, before.generation.0 + 1);
    assert_ne!(after.fingerprint, before.fingerprint);
}

#[tokio::test]
async fn dry_run_reload_publishes_no_generation() {
    let runtime = runtime();
    let before = runtime.extension_manager.active_snapshot();

    let report = runtime
        .reload_extensions(ReloadExtensionsRequest {
            dry_run: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let after = runtime.extension_manager.active_snapshot();

    assert!(!report.published);
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.fingerprint, before.fingerprint);
}

fn runtime() -> gestalt_runtime::AgentRuntime {
    AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .build()
        .unwrap()
}
