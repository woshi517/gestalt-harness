use std::sync::Arc;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    context::{ContextPipeline, TokenBudget},
    event::{AgentEvent, StopReason},
    message::Message,
    policy::{PolicyEngine, PolicyDecision, PolicyRequest},
    provider::{Provider, ProviderCapabilities, ProviderRequest, EventStream},
    session::{Session, SessionConfig},
    tool::{ToolCatalog, ToolContext, ToolSchema},
};
use gestalt_runtime::{AgentRuntimeBuilder, RuntimeConfig, UserInput};

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
        };
        &CAP
    }
    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> { None }
    fn count_tokens(&self, _model: &str, _messages: &[Message]) -> Result<usize, gestalt_core::error::HarnessError> { Ok(0) }
    async fn stream(&self, _request: ProviderRequest) -> Result<EventStream, gestalt_core::error::HarnessError> {
        let events = vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }];
        let stream = futures::stream::iter(events.into_iter().map(Ok::<_, gestalt_core::error::HarnessError>));
        Ok(Box::pin(stream))
    }
}

struct MockToolCatalog;
impl ToolCatalog for MockToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> { Vec::new() }
    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> { None }
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

fn build_test_runtime() -> gestalt_runtime::runtime::AgentRuntime {
    AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(RuntimeConfig::default())
        .build()
        .unwrap()
}

#[tokio::test]
async fn test_runtime_run_prompt_happy_path() {
    let runtime = build_test_runtime();
    let (tx, mut rx) = mpsc::unbounded_channel();
    
    let input = UserInput {
        prompt: "hello".to_string(),
        session_id: None,
        cancel_token: gestalt_core::cancel::CancelToken::new(),
        event_tx: Some(tx),
        artifact_dir: None,
    };

    let res = runtime.run_prompt(input).await;
    assert!(res.is_ok());
    let run_res = res.unwrap();
    assert_eq!(run_res.stop_reason, StopReason::EndTurn);

    // Drain events to verify expected sequences
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert!(!events.is_empty());
    // Verify it captures workspace snapshot first
    assert!(matches!(events[0], AgentEvent::WorkspaceSnapshotCaptured { .. }));
    // Verify it outputs the user message
    assert!(matches!(events[1], AgentEvent::UserMessage { .. }));
}

#[tokio::test]
async fn test_runtime_run_session_preserves_history() {
    let runtime = build_test_runtime();
    let cancel = gestalt_core::cancel::CancelToken::new();

    let snapshot = gestalt_core::snapshot::WorkspaceSnapshot {
        workspace_root: std::env::current_dir().unwrap(),
        git_sha: None,
        git_dirty: None,
        untracked_count: None,
        content_hash: "mock-hash".to_string(),
        captured_at: chrono::Utc::now(),
    };

    let mut session = Session::new(
        "test-session",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 5,
        },
        TokenBudget {
            model_limit: 100,
            reserved_output: 10,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 8,
        },
        ToolContext {
            working_dir: std::env::current_dir().unwrap(),
            workspace_root: Some(std::env::current_dir().unwrap()),
            timeout: Duration::from_secs(1),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: 100,
            artifact_dir: None,
            current_tool_call_id: None,
        },
        gestalt_core::session::ExecutionMode::Yolo,
        snapshot,
    );

    // Prepopulate session history
    session.history.push(Message::User {
        content: vec![gestalt_core::message::ContentBlock::Text {
            text: "Initial user turn".to_string(),
        }],
    });
    session.history.push(Message::Assistant {
        content: vec![gestalt_core::message::ContentBlock::Text {
            text: "Initial assistant turn".to_string(),
        }],
    });

    let res = runtime.run_session(&mut session, &cancel, None).await;
    assert!(res.is_ok());

    // Verify history was preserved and not reset
    assert_eq!(session.history.len(), 3); 
    if let Message::User { content } = &session.history[0] {
        if let gestalt_core::message::ContentBlock::Text { text } = &content[0] {
            assert_eq!(text, "Initial user turn");
        }
    }
    if let Message::Assistant { content } = &session.history[1] {
        if let gestalt_core::message::ContentBlock::Text { text } = &content[0] {
            assert_eq!(text, "Initial assistant turn");
        }
    }
}
