use gestalt_core::{
    approval::AutoApprovalProvider,
    cancel::CancelToken,
    context::{ContextPipeline, SessionMessage, TokenBudget},
    event::{AgentEvent, StopReason},
    hook::{HookRegistry, TraceHook},
    message::{ContentBlock, Message},
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    session::{ExecutionMode, Session, SessionConfig},
    session_queue::{MessageSource, QueueAck, QueueLifecycle, QueuedSessionMessage, SteeringQueue},
    snapshot::WorkspaceSnapshot,
    tool::{ToolCatalog, ToolContext},
    AgentLoop,
};
use std::sync::Arc;
use std::sync::Mutex;

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
            supports_streaming: false,
            supports_strict_schema: false,
        };
        &CAP
    }
    fn model_info(&self, _model: &str) -> Option<gestalt_core::model::ModelInfo> {
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
        let stream = futures::stream::iter(vec![Ok(AgentEvent::Stop {
            reason: StopReason::EndTurn,
        })]);
        Ok(Box::pin(stream))
    }
}

struct MockContextPipeline;
impl ContextPipeline for MockContextPipeline {
    fn process(&self, history: &[SessionMessage], _budget: &TokenBudget) -> Vec<Message> {
        history.iter().map(|entry| entry.message.clone()).collect()
    }
    fn version(&self) -> &str {
        "mock"
    }
}

struct MockToolCatalog;
impl ToolCatalog for MockToolCatalog {
    fn schemas(&self) -> Vec<serde_json::Value> {
        vec![]
    }
    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        None
    }
}

struct MockPolicyEngine;
#[async_trait::async_trait]
impl PolicyEngine for MockPolicyEngine {
    async fn evaluate(&self, _req: PolicyRequest) -> PolicyDecision {
        PolicyDecision {
            status: gestalt_core::event::PolicyStatus::Allowed,
            reason: None,
            policy_source: "mock".to_string(),
        }
    }
}

struct TestSteeringQueue {
    messages: Mutex<Vec<QueuedSessionMessage>>,
}

struct EnqueueOnAssistantCommitHook {
    queue: Arc<TestSteeringQueue>,
}

impl TraceHook for EnqueueOnAssistantCommitHook {
    fn on_trace_write(
        &self,
        event: &AgentEvent,
    ) -> std::result::Result<(), gestalt_core::error::TraceError> {
        if matches!(event, AgentEvent::AssistantMessageCommitted { .. }) {
            self.queue
                .messages
                .lock()
                .unwrap()
                .push(QueuedSessionMessage {
                    id: "late-msg".to_string(),
                    content: "Late operator correction".to_string(),
                    source: MessageSource::Operator,
                    idempotency_key: None,
                    injected_at_turn: None,
                });
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SteeringQueue for TestSteeringQueue {
    async fn enqueue(
        &self,
        message: QueuedSessionMessage,
    ) -> Result<QueueAck, gestalt_core::error::HarnessError> {
        self.messages.lock().unwrap().push(message);
        Ok(QueueAck::Queued)
    }
    async fn drain(&self) -> Result<Vec<QueuedSessionMessage>, gestalt_core::error::HarnessError> {
        let mut guard = self.messages.lock().unwrap();
        Ok(std::mem::take(&mut *guard))
    }
    async fn update_lifecycle(
        &self,
        _state: QueueLifecycle,
    ) -> Result<(), gestalt_core::error::HarnessError> {
        Ok(())
    }
    async fn len(&self) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(self.messages.lock().unwrap().len())
    }
}

fn make_session() -> Session {
    Session::new(
        "test-session",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 1,
            top_p: None,
            reasoning_effort: None,
            text_verbosity: None,
            metadata: serde_json::Value::Null,
            resolved_model: None,
        },
        TokenBudget {
            model_limit: 1000,
            reserved_output: 10,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 1,
        },
        ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            workspace_root: None,
            timeout: std::time::Duration::from_secs(1),
            allow_network: false,
            environment: std::collections::HashMap::new(),
            max_output_bytes: 1000,
            artifact_dir: None,
            current_tool_call_id: None,
            ignore_patterns: Vec::new(),
        },
        ExecutionMode::Yolo,
        WorkspaceSnapshot {
            workspace_root: std::path::PathBuf::from("/"),
            git_sha: None,
            git_dirty: Some(false),
            untracked_count: None,
            content_hash: "hash".to_string(),
            captured_at: chrono::Utc::now(),
        },
    )
}

#[tokio::test]
async fn test_agent_loop_drain_single_message() {
    let queue = Arc::new(TestSteeringQueue {
        messages: Mutex::new(vec![]),
    });
    let loop_ = AgentLoop::new(
        Arc::new(MockProvider),
        Arc::new(MockToolCatalog),
        Arc::new(MockContextPipeline),
        Arc::new(MockPolicyEngine),
        Arc::new(AutoApprovalProvider),
        1,
    )
    .with_steering_queue(queue.clone());

    let mut session = make_session();
    let cancel = CancelToken::new();

    // Queue an operator message
    let msg = QueuedSessionMessage {
        id: "msg-1".to_string(),
        content: "Stop and report".to_string(),
        source: MessageSource::Operator,
        idempotency_key: None,
        injected_at_turn: None,
    };
    queue.enqueue(msg).await.unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    loop_
        .run(&mut session, &cancel, None, move |ev| {
            events_clone.lock().unwrap().push(ev);
        })
        .await
        .unwrap();

    let events_guard = events.lock().unwrap();

    // Check that we got SessionMessageInjected first before ContextBuilt
    let mut injected_index = None;
    let mut context_built_index = None;
    let mut drained_index = None;

    for (idx, ev) in events_guard.iter().enumerate() {
        match ev {
            AgentEvent::SessionMessageInjected { message } => {
                assert_eq!(message.id, "msg-1");
                assert_eq!(message.injected_at_turn, Some(0));
                injected_index = Some(idx);
            }
            AgentEvent::SessionMessageQueueDrained { count } => {
                assert_eq!(*count, 1);
                drained_index = Some(idx);
            }
            AgentEvent::ContextBuilt { .. } => {
                context_built_index = Some(idx);
            }
            _ => {}
        }
    }

    assert!(
        injected_index.is_some(),
        "Should emit SessionMessageInjected"
    );
    assert!(
        drained_index.is_some(),
        "Should emit SessionMessageQueueDrained"
    );
    assert!(context_built_index.is_some(), "Should emit ContextBuilt");

    let inj = injected_index.unwrap();
    let dr = drained_index.unwrap();
    let cb = context_built_index.unwrap();

    assert!(
        inj < dr,
        "Injected event must be emitted before Drained event"
    );
    assert!(dr < cb, "Drained event must be emitted before ContextBuilt");

    // History should contain the injected message
    let user_injected = session.history.iter().any(|m| {
        if let Message::User { content, metadata } = &m.message {
            if let ContentBlock::Text { text } = &content[0] {
                return text == "Stop and report"
                    && metadata.as_ref().and_then(|value| value.source)
                        == Some(MessageSource::Operator);
            }
        }
        false
    });
    assert!(user_injected);
}

#[tokio::test]
async fn test_agent_loop_drain_empty_no_events() {
    let queue = Arc::new(TestSteeringQueue {
        messages: Mutex::new(vec![]),
    });
    let loop_ = AgentLoop::new(
        Arc::new(MockProvider),
        Arc::new(MockToolCatalog),
        Arc::new(MockContextPipeline),
        Arc::new(MockPolicyEngine),
        Arc::new(AutoApprovalProvider),
        1,
    )
    .with_steering_queue(queue.clone());

    let mut session = make_session();
    let cancel = CancelToken::new();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    loop_
        .run(&mut session, &cancel, None, move |ev| {
            events_clone.lock().unwrap().push(ev);
        })
        .await
        .unwrap();

    let events_guard = events.lock().unwrap();

    for ev in events_guard.iter() {
        assert!(!matches!(ev, AgentEvent::SessionMessageInjected { .. }));
        assert!(!matches!(ev, AgentEvent::SessionMessageQueueDrained { .. }));
    }
}

#[tokio::test]
async fn test_agent_loop_does_not_drain_messages_after_terminal_stop() {
    let queue = Arc::new(TestSteeringQueue {
        messages: Mutex::new(vec![]),
    });
    let mut hooks = HookRegistry::new();
    hooks.register_trace_hook(Arc::new(EnqueueOnAssistantCommitHook {
        queue: queue.clone(),
    }));

    let loop_ = AgentLoop::new(
        Arc::new(MockProvider),
        Arc::new(MockToolCatalog),
        Arc::new(MockContextPipeline),
        Arc::new(MockPolicyEngine),
        Arc::new(AutoApprovalProvider),
        1,
    )
    .with_hooks(hooks)
    .with_steering_queue(queue.clone());

    let mut session = make_session();
    let cancel = CancelToken::new();

    loop_
        .run(&mut session, &cancel, None, |_ev| {})
        .await
        .unwrap();

    assert!(!session.history.iter().any(|message| {
        matches!(&message.message, Message::User { content, .. }
            if matches!(&content[0], ContentBlock::Text { text } if text == "Late operator correction"))
    }));
    assert_eq!(queue.len().await.unwrap(), 1);
}
