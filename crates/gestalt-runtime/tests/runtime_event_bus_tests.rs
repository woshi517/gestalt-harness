use std::sync::Arc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    context::{ContextPipeline, TokenBudget},
    event::{AgentEvent, StopReason},
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolSchema},
};
use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig, RuntimeEvent, UserInput};

fn temp_artifact_dir() -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "gestalt-runtime-event-bus-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
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
        let events = vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }];
        let stream = futures::stream::iter(
            events
                .into_iter()
                .map(Ok::<_, gestalt_core::error::HarnessError>),
        );
        Ok(Box::pin(stream))
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

struct MockContextPipeline;
impl ContextPipeline for MockContextPipeline {
    fn process(
        &self,
        _history: &[gestalt_core::SessionMessage],
        _budget: &TokenBudget,
    ) -> Vec<Message> {
        Vec::new()
    }
    fn version(&self) -> &str {
        "mock-v1"
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
async fn test_runtime_event_bus_basic_fanout() {
    let mut config = RuntimeConfig::default();
    config.context_management_policy = Some(gestalt_core::ContextManagementPolicy {
        enabled: false,
        ..Default::default()
    });

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config)
        .build()
        .unwrap();

    let mut sub = runtime.event_bus.subscribe();
    let input = UserInput {
        prompt: "hello".to_string(),
        session_id: None,
        cancel_token: gestalt_core::cancel::CancelToken::new(),
        event_tx: None,
        artifact_dir: Some(temp_artifact_dir()),
    };

    let res = runtime.run_prompt(input).await;
    assert!(res.is_ok(), "{res:?}");

    let mut events = Vec::new();
    while let Ok(evt) = sub.try_recv() {
        events.push((*evt).clone());
    }

    assert!(!events.is_empty());
    // The first event should be SessionSpawned
    assert!(matches!(events[0], RuntimeEvent::SessionSpawned { .. }));

    // Subsequent events should be Agent events
    let agent_events: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let RuntimeEvent::Agent { event, .. } = e {
                Some(event)
            } else {
                None
            }
        })
        .collect();

    assert!(!agent_events.is_empty());

    // Check that key events were received
    let has_snapshot = agent_events
        .iter()
        .any(|e| matches!(e, AgentEvent::WorkspaceSnapshotCaptured { .. }));
    let has_user_msg = agent_events
        .iter()
        .any(|e| matches!(e, AgentEvent::UserMessage { .. }));
    let has_stop = agent_events
        .iter()
        .any(|e| matches!(e, AgentEvent::Stop { .. }));

    assert!(
        has_snapshot,
        "Should have received WorkspaceSnapshotCaptured event"
    );
    assert!(has_user_msg, "Should have received UserMessage event");
    assert!(has_stop, "Should have received Stop event");
}
