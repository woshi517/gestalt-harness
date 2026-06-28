use std::path::Path;
use std::sync::Arc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolSchema},
    ContextStability,
};
use gestalt_runtime as gestalt_context;
use gestalt_runtime::{
    AfterContextBuildCtx, AfterToolResultCtx, AgentRuntimeBuilder, BeforeContextBuildCtx,
    BeforeToolPolicyCtx, CompositionHooks, ContextContributor, HookOutcome, OnEventCtx,
    PrepareNextTurnCtx, RuntimeConfig,
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

struct TestContextContributor;

#[async_trait::async_trait]
impl ContextContributor for TestContextContributor {
    fn name(&self) -> &str {
        "test_context"
    }

    fn stability(&self) -> ContextStability {
        ContextStability::TurnDynamic
    }

    async fn contribute(&self, _workspace_root: &Path) -> gestalt_runtime::Result<Message> {
        Ok(Message::System {
            content: "test context".to_string(),
        })
    }
}

struct NoopCompositionHooks;

#[async_trait::async_trait]
impl CompositionHooks for NoopCompositionHooks {
    async fn before_context_build(
        &self,
        _context: &BeforeContextBuildCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn after_context_build(
        &self,
        _context: &AfterContextBuildCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn before_tool_policy(
        &self,
        _context: &BeforeToolPolicyCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn after_tool_result(
        &self,
        _context: &AfterToolResultCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn prepare_next_turn(
        &self,
        _context: &PrepareNextTurnCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn on_event(&self, _context: &OnEventCtx) -> gestalt_runtime::Result<()> {
        Ok(())
    }
}

#[test]
fn runtime_snapshot_contains_typed_context_and_policy_plans() {
    let mut builder = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .composition_hooks(Arc::new(NoopCompositionHooks));

    builder
        .registry
        .register_context_contributor("test_context".to_string(), Arc::new(TestContextContributor))
        .unwrap();

    let runtime = builder.build().unwrap();
    let context_sources = runtime
        .extension_snapshot
        .context_plan
        .registrations
        .iter()
        .map(|registration| registration.source.as_str())
        .collect::<Vec<_>>();

    assert!(context_sources.contains(&"test_context"));
    assert!(context_sources.contains(&"native-composition-hooks"));
    assert_eq!(
        runtime
            .extension_snapshot
            .policy_plan
            .registrations
            .first()
            .unwrap()
            .descriptor
            .component_id,
        "native:composition_hooks:before_tool_policy"
    );
}
#[allow(unused_imports)]
use gestalt_runtime as gestalt_trace;
