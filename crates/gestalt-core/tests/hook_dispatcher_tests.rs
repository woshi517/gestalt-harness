use async_trait::async_trait;
use gestalt_core::{
    approval::AutoApprovalProvider,
    cancel::CancelToken,
    context::{ContextPipeline, SessionMessage, TokenBudget},
    event::{AgentEvent, StopReason},
    hook::{ContextHook, HookDispatcher, HookRegistry, NextTurnHook, ToolHook},
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    session::{ExecutionMode, Session, SessionConfig},
    snapshot::WorkspaceSnapshot,
    tool::{RiskLevel, Tool, ToolCatalog, ToolContext, ToolOutput},
    AgentLoop, ContextPacket, HarnessError, ToolError, ToolExecutionResult,
};
use std::sync::Arc;
use std::sync::Mutex;

struct MockProvider;
#[async_trait]
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
            supports_tools: true,
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
    fn count_tokens(&self, _model: &str, _messages: &[Message]) -> Result<usize, HarnessError> {
        Ok(0)
    }
    async fn stream(&self, _request: ProviderRequest) -> Result<EventStream, HarnessError> {
        let stream = futures::stream::iter(vec![
            Ok(AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "dummy".to_string(),
                input_delta: "{}".to_string(),
            }),
            Ok(AgentEvent::Stop {
                reason: StopReason::ToolUse,
            }),
        ]);
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

struct DummyTool;
#[async_trait]
impl Tool for DummyTool {
    fn name(&self) -> &str {
        "dummy"
    }
    fn description(&self) -> &str {
        "dummy"
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "name": "dummy",
            "description": "dummy",
            "input_schema": {
                "type": "object",
                "properties": {}
            }
        })
    }
    fn risk(&self, _input: &serde_json::Value) -> RiskLevel {
        RiskLevel::Low
    }
    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Text {
            content: "ok".to_string(),
        })
    }
}

struct MockToolCatalog {
    tool: Arc<dyn Tool>,
}
impl ToolCatalog for MockToolCatalog {
    fn schemas(&self) -> Vec<serde_json::Value> {
        vec![self.tool.schema()]
    }
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if name == "dummy" {
            Some(self.tool.clone())
        } else {
            None
        }
    }
}

struct MockPolicyEngine;
#[async_trait]
impl PolicyEngine for MockPolicyEngine {
    async fn evaluate(&self, _req: PolicyRequest) -> PolicyDecision {
        PolicyDecision {
            status: gestalt_core::event::PolicyStatus::Allowed,
            reason: None,
            policy_source: "mock".to_string(),
        }
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
            max_turns: 2,
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

struct FailingContextHook;
#[async_trait]
impl ContextHook for FailingContextHook {
    async fn before_context_build(
        &self,
        _session: &Session,
    ) -> Result<Vec<AgentEvent>, HarnessError> {
        Err(HarnessError::Context(
            gestalt_core::error::ContextError::PipelineFailed(
                "before_context_build fail".to_string(),
            ),
        ))
    }
    async fn after_context_build(
        &self,
        _session: &Session,
        _packet: &ContextPacket,
    ) -> Result<Vec<AgentEvent>, HarnessError> {
        Ok(vec![])
    }
}

struct FailingToolHook;
#[async_trait]
impl ToolHook for FailingToolHook {
    async fn before_tool_execution(
        &self,
        _session: &Session,
        _name: &str,
        _input: &serde_json::Value,
    ) -> Result<Vec<AgentEvent>, HarnessError> {
        Err(HarnessError::Tool(ToolError::NotFound(
            "before_tool_execution fail".to_string(),
        )))
    }
    async fn after_tool_execution(
        &self,
        _session: &Session,
        _name: &str,
        _res: &ToolExecutionResult,
    ) -> Result<Vec<AgentEvent>, HarnessError> {
        Ok(vec![])
    }
}

struct BlockingNextTurnHook;
#[async_trait]
impl NextTurnHook for BlockingNextTurnHook {
    async fn prepare_next_turn(
        &self,
        _session: &Session,
        _turn: usize,
    ) -> Result<Vec<AgentEvent>, HarnessError> {
        Ok(vec![AgentEvent::NextTurnBlocked {
            reason: "prepare_next_turn block".to_string(),
        }])
    }
}

#[tokio::test]
async fn test_hook_dispatcher_unit_success() {
    let cancel = CancelToken::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let emit = move |ev| {
        events_clone.lock().unwrap().push(ev);
        Ok(())
    };

    let res = HookDispatcher::dispatch("test_hook", "test_name", &cancel, emit, || async {
        Ok(vec![AgentEvent::UserMessage {
            content: "success".to_string(),
        }])
    })
    .await;

    assert!(res.is_ok());
    let res_events = res.unwrap();
    assert_eq!(res_events.len(), 1);

    let emitted = events.lock().unwrap().clone();
    assert_eq!(emitted.len(), 2);
    assert!(matches!(emitted[0], AgentEvent::HookStarted { .. }));
    assert!(matches!(emitted[1], AgentEvent::HookCompleted { .. }));
}

#[tokio::test]
async fn test_hook_dispatcher_unit_failure() {
    let cancel = CancelToken::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();
    let emit = move |ev| {
        events_clone.lock().unwrap().push(ev);
        Ok(())
    };

    let res = HookDispatcher::dispatch("test_hook", "test_name", &cancel, emit, || async {
        Err(HarnessError::Cancelled)
    })
    .await;

    assert!(res.is_err());
    let emitted = events.lock().unwrap().clone();
    assert_eq!(emitted.len(), 2);
    assert!(matches!(emitted[0], AgentEvent::HookStarted { .. }));
    assert!(matches!(emitted[1], AgentEvent::HookFailed { .. }));
}

#[tokio::test]
async fn test_context_hook_fail_open() {
    let loop_ = AgentLoop::new(
        Arc::new(MockProvider),
        Arc::new(MockToolCatalog {
            tool: Arc::new(DummyTool),
        }),
        Arc::new(MockContextPipeline),
        Arc::new(MockPolicyEngine),
        Arc::new(AutoApprovalProvider),
        1,
    );
    let mut hooks = HookRegistry::new();
    hooks.register_context_hook(Arc::new(FailingContextHook));
    let loop_ = loop_.with_hooks(hooks);

    let mut session = make_session();
    let cancel = CancelToken::new();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    let res = loop_
        .run(&mut session, &cancel, None, move |ev| {
            events_clone.lock().unwrap().push(ev);
        })
        .await;

    // Fails open, so the loop runs successfully
    assert!(res.is_ok());

    let emitted = events.lock().unwrap().clone();
    // Verify HookFailed and Error events are emitted
    let has_hook_failed = emitted
        .iter()
        .any(|e| matches!(e, AgentEvent::HookFailed { hook_type, .. } if hook_type == "context"));
    let has_error = emitted.iter().any(|e| {
        matches!(
            e,
            AgentEvent::Error {
                recoverable: true,
                ..
            }
        )
    });
    assert!(has_hook_failed);
    assert!(has_error);
}

#[tokio::test]
async fn test_tool_hook_fail_open() {
    let loop_ = AgentLoop::new(
        Arc::new(MockProvider),
        Arc::new(MockToolCatalog {
            tool: Arc::new(DummyTool),
        }),
        Arc::new(MockContextPipeline),
        Arc::new(MockPolicyEngine),
        Arc::new(AutoApprovalProvider),
        1,
    );
    let mut hooks = HookRegistry::new();
    hooks.register_tool_hook(Arc::new(FailingToolHook));
    let loop_ = loop_.with_hooks(hooks);

    let mut session = make_session();
    let cancel = CancelToken::new();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    let res = loop_
        .run(&mut session, &cancel, None, move |ev| {
            events_clone.lock().unwrap().push(ev);
        })
        .await;

    assert!(res.is_ok());

    let emitted = events.lock().unwrap().clone();
    let has_hook_failed = emitted
        .iter()
        .any(|e| matches!(e, AgentEvent::HookFailed { hook_type, .. } if hook_type == "tool"));
    let has_error = emitted.iter().any(|e| {
        matches!(
            e,
            AgentEvent::Error {
                recoverable: true,
                ..
            }
        )
    });
    assert!(has_hook_failed);
    assert!(has_error);
}

#[tokio::test]
async fn test_next_turn_hook_fail_closed_blocked() {
    let loop_ = AgentLoop::new(
        Arc::new(MockProvider),
        Arc::new(MockToolCatalog {
            tool: Arc::new(DummyTool),
        }),
        Arc::new(MockContextPipeline),
        Arc::new(MockPolicyEngine),
        Arc::new(AutoApprovalProvider),
        2,
    );
    let mut hooks = HookRegistry::new();
    hooks.register_next_turn_hook(Arc::new(BlockingNextTurnHook));
    let loop_ = loop_.with_hooks(hooks);

    let mut session = make_session();
    let cancel = CancelToken::new();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    let res = loop_
        .run(&mut session, &cancel, None, move |ev| {
            events_clone.lock().unwrap().push(ev);
        })
        .await;

    // Should return Ok(RunResult) but stop loop with HookBlocked stop reason
    assert!(res.is_ok());

    let emitted = events.lock().unwrap().clone();
    let has_blocked_event = emitted
        .iter()
        .any(|e| matches!(e, AgentEvent::NextTurnBlocked { .. }));
    let has_blocked_stop = emitted.iter().any(|e| {
        matches!(
            e,
            AgentEvent::Stop {
                reason: StopReason::HookBlocked
            }
        )
    });
    assert!(has_blocked_event);
    assert!(has_blocked_stop);
}
