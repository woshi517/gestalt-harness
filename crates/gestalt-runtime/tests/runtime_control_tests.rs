use std::sync::Arc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolSchema},
};
use gestalt_runtime::{
    AgentRuntimeBuilder, HostControl, ReloadExtensionsRequest, RuntimeConfig, RuntimeControl,
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
async fn dry_run_reload_does_not_publish_generation() {
    let runtime = runtime();

    let report = runtime
        .reload_extensions(ReloadExtensionsRequest {
            dry_run: true,
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(!report.published);
    assert_eq!(report.previous_generation.0, 0);
    assert_eq!(report.candidate_generation.0, 1);
    assert_eq!(runtime.current_generation().0, 0);
}

#[tokio::test]
async fn reload_publishes_next_generation_and_inspect_reports_it() {
    let runtime = runtime();

    let report = runtime
        .reload_extensions(ReloadExtensionsRequest::default())
        .await
        .unwrap();

    assert!(report.published);
    assert_eq!(runtime.current_generation(), report.candidate_generation);
    assert_eq!(
        runtime.inspect_runtime().await.runtime_generation,
        report.candidate_generation.0
    );
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

#[tokio::test]
async fn test_host_owns_workspace_and_generation_lineage() {
    let builder = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig {
            workspace_root: std::path::PathBuf::from("/test/host/workspace"),
            ..Default::default()
        });
    let host = Arc::new(gestalt_runtime::RuntimeHost::new(
        builder,
        Arc::new(gestalt_runtime::InMemoryArtifactStore::new()),
    ));

    assert_eq!(
        host.workspace_root,
        std::path::PathBuf::from("/test/host/workspace")
    );
    assert_eq!(host.current_generation().0, 0);

    let session_id = host.spawn_session("test-session", None).await.unwrap();
    let registry = host.session_registry.lock().unwrap();
    let session_runtime = registry.get(&session_id).unwrap();
    assert_eq!(
        session_runtime.config.workspace_root,
        std::path::PathBuf::from("/test/host/workspace")
    );
    assert_eq!(session_runtime.current_generation().0, 0);
}

#[tokio::test]
async fn test_per_session_override_cannot_mutate_critical_inputs() {
    let builder = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig {
            workspace_root: std::path::PathBuf::from("/test/host/workspace"),
            ..Default::default()
        });
    let host = Arc::new(gestalt_runtime::RuntimeHost::new(
        builder,
        Arc::new(gestalt_runtime::InMemoryArtifactStore::new()),
    ));

    let mut config_override = RuntimeConfig::default();
    config_override.workspace_root = std::path::PathBuf::from("/malicious/override");
    config_override.trusted_extension_ids = vec!["untrusted-but-trusted-via-override".to_string()];

    let session_id = host
        .spawn_session("test-session-override", Some(config_override))
        .await
        .unwrap();
    let registry = host.session_registry.lock().unwrap();
    let session_runtime = registry.get(&session_id).unwrap();

    assert_eq!(
        session_runtime.config.workspace_root,
        std::path::PathBuf::from("/test/host/workspace")
    );
    assert!(session_runtime.config.trusted_extension_ids.is_empty());
}

#[tokio::test]
async fn host_reload_advances_shared_generation_only_once_across_sessions() {
    let builder = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(EmptyToolCatalog))
        .assembler(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default());
    let host = Arc::new(gestalt_runtime::RuntimeHost::new(
        builder,
        Arc::new(gestalt_runtime::InMemoryArtifactStore::new()),
    ));

    host.spawn_session("session-a", None).await.unwrap();
    host.spawn_session("session-b", None).await.unwrap();

    let report = host
        .reload_extensions(ReloadExtensionsRequest::default())
        .await
        .unwrap();

    assert_eq!(report.previous_generation.0, 0);
    assert_eq!(report.candidate_generation.0, 1);
    assert_eq!(host.current_generation().0, 1);

    let sessions = host.session_registry.lock().unwrap();
    for runtime in sessions.values() {
        assert_eq!(runtime.current_generation().0, 1);
    }
}
