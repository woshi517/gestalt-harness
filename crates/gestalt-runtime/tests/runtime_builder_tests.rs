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
use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig};
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
    fn id(&self) -> &str { "mock" }
    fn display_name(&self) -> &str { "Mock" }
    fn default_model(&self) -> &str { "mock-model" }
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
    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> { None }
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
    fn schemas(&self) -> Vec<ToolSchema> { Vec::new() }
    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> { None }
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
