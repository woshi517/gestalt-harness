use gestalt_core::{
    approval::AutoApprovalProvider,
    context::{ContextPipeline, TokenBudget},
    event::{AgentEvent, StopReason},
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolSchema},
};
use gestalt_runtime::{
    AgentRuntimeBuilder, AgentRuntimeHandle, DefaultAgentRuntimeHandle, InMemoryArtifactStore,
    OrchestrationResult, OrchestrationTask, Orchestrator, RuntimeConfig, RuntimeEvent,
};
use std::sync::Arc;

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
    fn process(&self, _history: &[Message], _budget: &TokenBudget) -> Vec<Message> {
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

fn build_test_builder() -> AgentRuntimeBuilder {
    AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
}

struct TwoStepOrchestrator;

#[async_trait::async_trait]
impl Orchestrator for TwoStepOrchestrator {
    async fn execute(
        &self,
        handle: Arc<dyn AgentRuntimeHandle>,
        task: OrchestrationTask,
    ) -> Result<OrchestrationResult, gestalt_runtime::error::RuntimeError> {
        // Step 1: Spawn session 1, execute it, save artifact
        let session_1 = handle.spawn_session("writer-session", None).await?;
        let result_1 = handle.send_message(&session_1, &task.prompt).await?;

        let artifact_content = format!("Writer output: {:?}", result_1.stop_reason);
        let artifact_uri = handle
            .create_artifact(&session_1, "output.txt", artifact_content.as_bytes())
            .await?;

        // Step 2: Spawn session 2, read artifact, do downstream work
        let session_2 = handle.spawn_session("reviewer-session", None).await?;
        let read_back = handle.read_artifact(&session_1, "output.txt").await?;
        let read_back_str = String::from_utf8_lossy(&read_back);

        let prompt_2 = format!("Review this output: {}", read_back_str);
        let result_2 = handle.send_message(&session_2, &prompt_2).await?;

        Ok(OrchestrationResult {
            output: format!("Finished review: {:?}", result_2.stop_reason),
            output_artifacts: vec![artifact_uri],
        })
    }
}

#[tokio::test]
async fn test_orchestration_happy_path() {
    let builder = build_test_builder();
    let artifact_store = Arc::new(InMemoryArtifactStore::new());
    let handle = Arc::new(DefaultAgentRuntimeHandle::new(builder, artifact_store));

    // Subscribe to events
    let mut receiver = handle.subscribe();

    let orchestrator = TwoStepOrchestrator;
    let task = OrchestrationTask {
        prompt: "Write a short summary".to_string(),
        input_artifacts: vec![],
    };

    let result = orchestrator.execute(handle.clone(), task).await;
    assert!(result.is_ok());
    let run_res = result.unwrap();
    assert!(run_res.output.contains("Finished review"));
    assert_eq!(run_res.output_artifacts.len(), 1);
    assert_eq!(
        run_res.output_artifacts[0],
        "memory://writer-session/output.txt"
    );

    // Verify events received on subscriber
    let mut spawned_sessions = Vec::new();
    let mut artifact_routed = false;

    // Drain receiver to see what was published
    while let Ok(evt) = receiver.try_recv() {
        match &*evt {
            RuntimeEvent::SessionSpawned { session_id } => {
                spawned_sessions.push(session_id.clone());
            }
            RuntimeEvent::ArtifactRouted {
                session_id,
                path,
                size_bytes,
            } => {
                assert_eq!(session_id, "writer-session");
                assert_eq!(path, "memory://writer-session/output.txt");
                assert!(*size_bytes > 0);
                artifact_routed = true;
            }
            _ => {}
        }
    }

    spawned_sessions.sort();
    spawned_sessions.dedup();
    assert_eq!(spawned_sessions.len(), 2);
    assert!(spawned_sessions.contains(&"writer-session".to_string()));
    assert!(spawned_sessions.contains(&"reviewer-session".to_string()));
    assert!(artifact_routed);
}

#[tokio::test]
async fn test_orchestration_unknown_session_error() {
    let builder = build_test_builder();
    let artifact_store = Arc::new(InMemoryArtifactStore::new());
    let handle = Arc::new(DefaultAgentRuntimeHandle::new(builder, artifact_store));

    let res = handle.send_message("non-existent-session", "hello").await;
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.to_string().contains("Session not found"));
}

#[tokio::test]
async fn test_orchestration_duplicate_session_error() {
    let builder = build_test_builder();
    let artifact_store = Arc::new(InMemoryArtifactStore::new());
    let handle = Arc::new(DefaultAgentRuntimeHandle::new(builder, artifact_store));

    let res1 = handle.spawn_session("s1", None).await;
    assert!(res1.is_ok());

    let res2 = handle.spawn_session("s1", None).await;
    assert!(res2.is_err());
    let err = res2.unwrap_err();
    assert!(err.to_string().contains("Session already exists"));
}

#[tokio::test]
async fn test_orchestration_steering_enqueue() {
    let builder = build_test_builder();
    let artifact_store = Arc::new(InMemoryArtifactStore::new());
    let handle = Arc::new(DefaultAgentRuntimeHandle::new(builder, artifact_store));

    // Spawn a session
    let session_id = handle.spawn_session("steered-session", None).await.unwrap();

    let mut rx = handle.subscribe();

    // Enqueue a steering message
    let ack = handle
        .enqueue_steering_message(
            &session_id,
            "Inject this follow-up",
            gestalt_core::session_queue::MessageSource::FollowUp,
            Some("key-1".to_string()),
        )
        .await
        .unwrap();

    assert_eq!(ack, gestalt_core::session_queue::QueueAck::Queued);

    // Verify SessionMessageQueued event is published on event bus
    let mut event_found = false;
    while let Ok(evt) = rx.try_recv() {
        if let RuntimeEvent::SessionMessageQueued { message } = &*evt {
            assert_eq!(message.content, "Inject this follow-up");
            assert_eq!(message.idempotency_key, Some("key-1".to_string()));
            event_found = true;
        }
    }
    assert!(event_found, "Should have received SessionMessageQueued event");
}
