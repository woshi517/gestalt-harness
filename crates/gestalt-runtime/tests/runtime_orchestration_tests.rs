#![allow(deprecated)]

use gestalt_core::{
    approval::AutoApprovalProvider,
    context::{ContextPipeline, TokenBudget},
    event::{AgentEvent, StopReason},
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{Tool, ToolCatalog, ToolSchema},
};
use gestalt_runtime as gestalt_context;
use gestalt_runtime::{
    AgentRuntimeBuilder, AgentRuntimeHandle, DefaultAgentRuntimeHandle, HostControl,
    InMemoryArtifactStore, OrchestrationResult, OrchestrationTask, Orchestrator, RuntimeConfig,
    RuntimeEvent,
};
use std::sync::Arc;

fn config_without_context_management() -> RuntimeConfig {
    let mut config = RuntimeConfig::default();
    config.context_management_policy = Some(gestalt_core::ContextManagementPolicy {
        enabled: false,
        ..Default::default()
    });
    config
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

    fn as_assembler(&self) -> Option<Arc<dyn gestalt_core::context::ContextAssembler>> {
        Some(Arc::new(gestalt_context::ContextMessageAssembler::new(
            "pipeline-v1",
        )))
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
        .config(config_without_context_management())
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

    // Enqueue a steering message before session starts running -> SessionNotActive
    let ack = handle
        .enqueue_steering_message(
            &session_id,
            "Inject this follow-up",
            gestalt_core::session_queue::MessageSource::FollowUp,
            Some("key-1".to_string()),
        )
        .await
        .unwrap();

    assert_eq!(ack, gestalt_core::session_queue::QueueAck::SessionNotActive);

    // Verify no SessionMessageQueued event is published on event bus
    let mut event_found = false;
    while let Ok(evt) = rx.try_recv() {
        if let RuntimeEvent::SessionMessageQueued { .. } = &*evt {
            event_found = true;
        }
    }
    assert!(
        !event_found,
        "Should NOT have received SessionMessageQueued event before active"
    );
}

#[tokio::test]
async fn test_orchestration_steering_concurrent() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(SteeringTestProvider {
        requests: requests.clone(),
    });

    let tool_handle = Arc::new(Mutex::new(None));
    let steer_tool = Arc::new(SteeringTestTool {
        handle: tool_handle.clone(),
        session_id: "concurrent-steered-session".to_string(),
    });
    let tools = Arc::new(SteeringTestToolCatalog { tool: steer_tool });

    let builder = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(tools)
        .middleware(Arc::new(PassThroughContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig {
            max_turns: 3,
            ..config_without_context_management()
        });

    let artifact_store = Arc::new(InMemoryArtifactStore::new());
    let handle = Arc::new(DefaultAgentRuntimeHandle::new(builder, artifact_store));
    *tool_handle.lock().unwrap() = Some(handle.clone());

    // Spawn and run the session
    let session_id = handle
        .spawn_session("concurrent-steered-session", None)
        .await
        .unwrap();

    let res = handle
        .send_message(&session_id, "Initial prompt")
        .await
        .unwrap();

    assert_eq!(res.stop_reason, StopReason::EndTurn);

    // Verify requests sent to the provider
    let reqs = requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);

    // Turn 1 request should not have the steered message, but has the initial prompt
    let req1_msgs = &reqs[0].messages;
    assert_eq!(req1_msgs.len(), 1);
    match &req1_msgs[0] {
        Message::User { content, .. } => match &content[0] {
            gestalt_core::message::ContentBlock::Text { text } => {
                assert_eq!(text, "Initial prompt");
            }
            _ => panic!("Expected text block"),
        },
        _ => panic!("Expected user message"),
    }

    // Turn 2 request should have:
    // 1. Initial user prompt
    // 2. Tool call request (from provider response 1)
    // 3. Tool response "Tool executed"
    // 4. INJECTED steering message: "Concurrently injected steering message"
    let req2_msgs = &reqs[1].messages;
    // Let's verify that the last message is indeed the injected user message
    assert!(req2_msgs.len() >= 4);

    let injected_msg = &req2_msgs[3];
    match injected_msg {
        Message::User { content, .. } => match &content[0] {
            gestalt_core::message::ContentBlock::Text { text } => {
                assert_eq!(text, "Concurrently injected steering message");
            }
            _ => panic!("Expected text block"),
        },
        _ => panic!("Expected user message"),
    }
}

use std::sync::Mutex;

struct SteeringTestProvider {
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

#[async_trait::async_trait]
impl Provider for SteeringTestProvider {
    fn id(&self) -> &str {
        "steering-mock"
    }
    fn display_name(&self) -> &str {
        "Steering Mock"
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
        request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        self.requests.lock().unwrap().push(request.clone());
        let turn = self.requests.lock().unwrap().len();
        if turn == 1 {
            let events = vec![
                AgentEvent::ToolCallStreamed {
                    id: "call-1".to_string(),
                    name: "steer_tool".to_string(),
                    input_delta: "{}".to_string(),
                },
                AgentEvent::Stop {
                    reason: StopReason::ToolUse,
                },
            ];
            let stream = futures::stream::iter(
                events
                    .into_iter()
                    .map(Ok::<_, gestalt_core::error::HarnessError>),
            );
            Ok(Box::pin(stream))
        } else {
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
}

struct SteeringTestTool {
    handle: Arc<Mutex<Option<Arc<DefaultAgentRuntimeHandle>>>>,
    session_id: String,
}

#[async_trait::async_trait]
impl gestalt_core::tool::Tool for SteeringTestTool {
    fn name(&self) -> &str {
        "steer_tool"
    }
    fn description(&self) -> &str {
        "steer tool"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "name": "steer_tool",
            "description": "steer tool",
            "input_schema": {
                "type": "object",
                "properties": {}
            }
        })
    }
    fn risk(&self, _input: &serde_json::Value) -> gestalt_core::tool::RiskLevel {
        gestalt_core::tool::RiskLevel::Low
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &gestalt_core::tool::ToolContext,
    ) -> Result<gestalt_core::tool::ToolOutput, gestalt_core::error::ToolError> {
        let handle = self.handle.lock().unwrap().as_ref().unwrap().clone();
        let ack = handle
            .enqueue_steering_message(
                &self.session_id,
                "Concurrently injected steering message",
                gestalt_core::session_queue::MessageSource::FollowUp,
                None,
            )
            .await
            .unwrap();
        assert_eq!(ack, gestalt_core::session_queue::QueueAck::Queued);
        Ok(gestalt_core::tool::ToolOutput::Text {
            content: "Tool executed".to_string(),
        })
    }
}

struct SteeringTestToolCatalog {
    tool: Arc<SteeringTestTool>,
}
impl ToolCatalog for SteeringTestToolCatalog {
    fn schemas(&self) -> Vec<gestalt_core::tool::ToolSchema> {
        vec![self.tool.schema()]
    }
    fn get(&self, name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        if name == "steer_tool" {
            Some(self.tool.clone())
        } else {
            None
        }
    }
}

struct PassThroughAssembler;
impl gestalt_core::context::ContextAssembler for PassThroughAssembler {
    fn version(&self) -> &str {
        "pass-through"
    }
    fn system_messages(&self) -> Vec<Message> {
        Vec::new()
    }
    fn assemble(
        &self,
        plan: &gestalt_core::context::ContextPlan,
    ) -> Result<gestalt_core::context::ContextPacket, gestalt_core::error::ContextError> {
        let messages = plan
            .history
            .iter()
            .map(|entry| entry.message.clone())
            .collect();
        Ok(gestalt_core::context::ContextPacket {
            messages,
            packet_hash: "pass-through".to_string(),
            pipeline_version: "pass-through".to_string(),
            tokenizer_id: "default".to_string(),
            token_estimate: 0,
            sources: vec![],
            omissions: vec![],
            message_hashes: vec![],
            prompt_assembly_strategy: gestalt_core::context::PromptAssemblyStrategy::Snapshot,
            snapshot_hash: None,
            cache_prefix_hash: None,
            segments: vec![],
            cache_plan: None,
            prompt_source: None,
        })
    }
}

struct PassThroughContextPipeline;
impl ContextPipeline for PassThroughContextPipeline {
    fn process(
        &self,
        history: &[gestalt_core::SessionMessage],
        _budget: &TokenBudget,
    ) -> Vec<Message> {
        history.iter().map(|entry| entry.message.clone()).collect()
    }
    fn version(&self) -> &str {
        "pass-through"
    }

    fn as_assembler(&self) -> Option<Arc<dyn gestalt_core::context::ContextAssembler>> {
        Some(Arc::new(PassThroughAssembler))
    }
}
