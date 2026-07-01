use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    context::{
        ContextPipeline, PromptAssemblyStrategy, PromptCachePlan, PromptSnapshot, TokenBudget,
    },
    event::{AgentEvent, StopReason},
    hook::ContextHook,
    message::{ContentBlock, Message},
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    session::{Session, SessionConfig},
    tool::{ToolCatalog, ToolContext, ToolOutput, ToolSchema},
};
use gestalt_runtime::unstable::ContextMessageAssembler;
use gestalt_runtime::unstable::{
    AfterContextBuildCtx, AgentRuntimeBuilder, CompositionHooks, ContextPatch, HookOutcome,
    RuntimeConfig, RuntimeContextHookAdapter, RuntimeContextPipeline, RuntimeEventBus, UserInput,
};

fn config_without_context_management() -> RuntimeConfig {
    let mut config = RuntimeConfig::default();
    config.context_management_policy = Some(gestalt_core::ContextManagementPolicy {
        enabled: false,
        ..Default::default()
    });
    config
}

struct NoopCompositionHooks;

#[async_trait::async_trait]
impl CompositionHooks for NoopCompositionHooks {
    async fn before_context_build(
        &self,
        _context: &gestalt_runtime::unstable::BeforeContextBuildCtx,
    ) -> gestalt_runtime::unstable::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn after_context_build(
        &self,
        _context: &AfterContextBuildCtx,
    ) -> gestalt_runtime::unstable::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn before_tool_policy(
        &self,
        _context: &gestalt_runtime::unstable::BeforeToolPolicyCtx,
    ) -> gestalt_runtime::unstable::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn after_tool_result(
        &self,
        _context: &gestalt_runtime::unstable::AfterToolResultCtx,
    ) -> gestalt_runtime::unstable::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn prepare_next_turn(
        &self,
        _context: &gestalt_runtime::unstable::PrepareNextTurnCtx,
    ) -> gestalt_runtime::unstable::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn on_event(
        &self,
        _context: &gestalt_runtime::unstable::OnEventCtx,
    ) -> gestalt_runtime::unstable::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn test_prepare_next_turn_switch_model() {
    struct SwitchModelCompositionHooks;

    #[async_trait::async_trait]
    impl CompositionHooks for SwitchModelCompositionHooks {
        async fn before_context_build(
            &self,
            _context: &gestalt_runtime::unstable::BeforeContextBuildCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }

        async fn after_context_build(
            &self,
            _context: &AfterContextBuildCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }

        async fn before_tool_policy(
            &self,
            _context: &gestalt_runtime::unstable::BeforeToolPolicyCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }

        async fn after_tool_result(
            &self,
            _context: &gestalt_runtime::unstable::AfterToolResultCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }

        async fn prepare_next_turn(
            &self,
            _context: &gestalt_runtime::unstable::PrepareNextTurnCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::SwitchModel {
                model: "cheaper-model".to_string(),
                provider: None,
                variant: None,
            })
        }

        async fn on_event(
            &self,
            _context: &gestalt_runtime::unstable::OnEventCtx,
        ) -> gestalt_runtime::unstable::Result<()> {
            Ok(())
        }
    }

    let turn1_request = Arc::new(std::sync::Mutex::new(None));
    let turn2_request = Arc::new(std::sync::Mutex::new(None));

    struct MultiTurnMockProvider {
        turn1_request: Arc<std::sync::Mutex<Option<ProviderRequest>>>,
        turn2_request: Arc<std::sync::Mutex<Option<ProviderRequest>>>,
        turn: std::sync::Mutex<usize>,
    }

    #[async_trait::async_trait]
    impl Provider for MultiTurnMockProvider {
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
                supports_json_schema_tools: true,
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
            let mut turn_lock = self.turn.lock().unwrap();
            *turn_lock += 1;
            let current_turn = *turn_lock;

            let events = if current_turn == 1 {
                *self.turn1_request.lock().unwrap() = Some(request);
                vec![
                    AgentEvent::ToolCallStreamed {
                        id: "call-1".to_string(),
                        name: "test-tool".to_string(),
                        input_delta: "{}".to_string(),
                    },
                    AgentEvent::Stop {
                        reason: StopReason::ToolUse,
                    },
                ]
            } else {
                *self.turn2_request.lock().unwrap() = Some(request);
                vec![AgentEvent::Stop {
                    reason: StopReason::EndTurn,
                }]
            };

            let stream = futures::stream::iter(
                events
                    .into_iter()
                    .map(Ok::<_, gestalt_core::error::HarnessError>),
            );
            Ok(Box::pin(stream))
        }
    }

    let provider = Arc::new(MultiTurnMockProvider {
        turn1_request: turn1_request.clone(),
        turn2_request: turn2_request.clone(),
        turn: std::sync::Mutex::new(0),
    });

    struct TestTool {
        name: String,
    }

    #[async_trait::async_trait]
    impl gestalt_core::tool::Tool for TestTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "test tool"
        }
        fn schema(&self) -> ToolSchema {
            serde_json::from_value(serde_json::json!({
                "name": self.name.clone(),
                "description": "test tool",
                "input_schema": {
                    "type": "object",
                    "properties": {}
                }
            }))
            .unwrap()
        }
        fn risk(&self, _input: &serde_json::Value) -> gestalt_core::tool::RiskLevel {
            gestalt_core::tool::RiskLevel::Low
        }
        async fn execute(
            &self,
            _input: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, gestalt_core::error::ToolError> {
            Ok(ToolOutput::Text {
                content: "ok".to_string(),
            })
        }
    }

    struct TestToolCatalog {
        tools: HashMap<String, Arc<dyn gestalt_core::tool::Tool>>,
    }

    impl ToolCatalog for TestToolCatalog {
        fn schemas(&self) -> Vec<ToolSchema> {
            self.tools.values().map(|t| t.schema()).collect()
        }
        fn get(&self, name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
            self.tools.get(name).cloned()
        }
    }

    let mut tools = HashMap::new();
    tools.insert(
        "test-tool".to_string(),
        Arc::new(TestTool {
            name: "test-tool".to_string(),
        }) as Arc<dyn gestalt_core::tool::Tool>,
    );
    let catalog = Arc::new(TestToolCatalog { tools });

    let runtime = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(catalog)
        .context_pipeline(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(Arc::new(SwitchModelCompositionHooks))
        .build()
        .unwrap();

    let input = UserInput {
        prompt: "hi".to_string(),
        session_id: None,
        cancel_token: gestalt_core::cancel::CancelToken::new(),
        event_tx: None,
        artifact_dir: None,
    };

    let res = runtime.run_prompt(input).await;
    assert!(res.is_ok());

    let req1 = turn1_request.lock().unwrap().clone().unwrap();
    assert_eq!(req1.model, "mock-model");

    let req2 = turn2_request.lock().unwrap().clone().unwrap();
    assert_eq!(req2.model, "cheaper-model");
}

#[tokio::test]
async fn test_run_prompt_uses_pinned_extension_snapshot_tool_catalog() {
    struct SnapshotTool;

    #[async_trait::async_trait]
    impl gestalt_core::tool::Tool for SnapshotTool {
        fn name(&self) -> &str {
            "snapshot-tool"
        }
        fn description(&self) -> &str {
            "snapshot tool"
        }
        fn schema(&self) -> ToolSchema {
            serde_json::from_value(serde_json::json!({
                "name": "snapshot-tool",
                "description": "snapshot tool",
                "input_schema": { "type": "object", "properties": {} }
            }))
            .unwrap()
        }
        fn risk(&self, _input: &serde_json::Value) -> gestalt_core::tool::RiskLevel {
            gestalt_core::tool::RiskLevel::Low
        }
        async fn execute(
            &self,
            _input: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, gestalt_core::error::ToolError> {
            Ok(ToolOutput::Text {
                content: "ok".to_string(),
            })
        }
    }

    struct SnapshotToolCatalog {
        tool: Arc<dyn gestalt_core::tool::Tool>,
    }

    impl ToolCatalog for SnapshotToolCatalog {
        fn schemas(&self) -> Vec<ToolSchema> {
            vec![self.tool.schema()]
        }

        fn get(&self, name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
            if name == self.tool.name() {
                Some(self.tool.clone())
            } else {
                None
            }
        }
    }

    struct ToolThenEndTurnMockProvider(std::sync::Mutex<usize>);

    #[async_trait::async_trait]
    impl Provider for ToolThenEndTurnMockProvider {
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
                supports_json_schema_tools: true,
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
            let mut turn = self.0.lock().unwrap();
            let current_turn = *turn;
            *turn += 1;

            let events = if current_turn == 0 {
                vec![
                    AgentEvent::ToolCallStreamed {
                        id: "call-1".to_string(),
                        name: "snapshot-tool".to_string(),
                        input_delta: "{}".to_string(),
                    },
                    AgentEvent::Stop {
                        reason: StopReason::ToolUse,
                    },
                ]
            } else {
                vec![AgentEvent::Stop {
                    reason: StopReason::EndTurn,
                }]
            };

            let stream = futures::stream::iter(
                events
                    .into_iter()
                    .map(Ok::<_, gestalt_core::error::HarnessError>),
            );
            Ok(Box::pin(stream))
        }
    }

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(ToolThenEndTurnMockProvider(
            std::sync::Mutex::new(0),
        )))
        .tools(Arc::new(SnapshotToolCatalog {
            tool: Arc::new(SnapshotTool),
        }))
        .context_pipeline(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .build()
        .unwrap();

    let mut runtime = runtime;
    runtime.tools = Arc::new(MockToolCatalog);

    let input = UserInput {
        prompt: "hi".to_string(),
        session_id: None,
        cancel_token: gestalt_core::cancel::CancelToken::new(),
        event_tx: None,
        artifact_dir: None,
    };

    let res = runtime.run_prompt(input).await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_prepare_next_turn_block_stops_session() {
    struct BlockCompositionHooks;

    #[async_trait::async_trait]
    impl CompositionHooks for BlockCompositionHooks {
        async fn before_context_build(
            &self,
            _context: &gestalt_runtime::unstable::BeforeContextBuildCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }

        async fn after_context_build(
            &self,
            _context: &AfterContextBuildCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }

        async fn before_tool_policy(
            &self,
            _context: &gestalt_runtime::unstable::BeforeToolPolicyCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }

        async fn after_tool_result(
            &self,
            _context: &gestalt_runtime::unstable::AfterToolResultCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::Continue)
        }

        async fn prepare_next_turn(
            &self,
            _context: &gestalt_runtime::unstable::PrepareNextTurnCtx,
        ) -> gestalt_runtime::unstable::Result<HookOutcome> {
            Ok(HookOutcome::Block {
                reason: "policy escalation blocked".to_string(),
            })
        }

        async fn on_event(
            &self,
            _context: &gestalt_runtime::unstable::OnEventCtx,
        ) -> gestalt_runtime::unstable::Result<()> {
            Ok(())
        }
    }

    struct ToolThenEndTurnMockProvider(std::sync::Mutex<usize>);

    #[async_trait::async_trait]
    impl Provider for ToolThenEndTurnMockProvider {
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
                supports_json_schema_tools: true,
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
            let turn = *self.0.lock().unwrap();
            *self.0.lock().unwrap() = turn + 1;

            let events = if turn == 0 {
                vec![
                    AgentEvent::ToolCallStreamed {
                        id: "call-1".to_string(),
                        name: "noop-tool".to_string(),
                        input_delta: "{}".to_string(),
                    },
                    AgentEvent::Stop {
                        reason: StopReason::ToolUse,
                    },
                ]
            } else {
                vec![AgentEvent::Stop {
                    reason: StopReason::EndTurn,
                }]
            };

            let stream = futures::stream::iter(
                events
                    .into_iter()
                    .map(Ok::<_, gestalt_core::error::HarnessError>),
            );
            Ok(Box::pin(stream))
        }
    }

    impl ToolThenEndTurnMockProvider {
        fn new() -> Self {
            Self(std::sync::Mutex::new(0))
        }
    }

    struct NoopTool;

    #[async_trait::async_trait]
    impl gestalt_core::tool::Tool for NoopTool {
        fn name(&self) -> &str {
            "noop-tool"
        }
        fn description(&self) -> &str {
            "noop"
        }
        fn schema(&self) -> ToolSchema {
            serde_json::from_value(serde_json::json!({
                "name": "noop-tool",
                "description": "noop",
                "input_schema": { "type": "object", "properties": {} }
            }))
            .unwrap()
        }
        fn risk(&self, _input: &serde_json::Value) -> gestalt_core::tool::RiskLevel {
            gestalt_core::tool::RiskLevel::Low
        }
        async fn execute(
            &self,
            _input: serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, gestalt_core::error::ToolError> {
            Ok(ToolOutput::Text {
                content: "ok".to_string(),
            })
        }
    }

    let mut tools = HashMap::new();
    tools.insert(
        "noop-tool".to_string(),
        Arc::new(NoopTool) as Arc<dyn gestalt_core::tool::Tool>,
    );
    struct LocalToolCatalog {
        tools: HashMap<String, Arc<dyn gestalt_core::tool::Tool>>,
    }

    impl ToolCatalog for LocalToolCatalog {
        fn schemas(&self) -> Vec<ToolSchema> {
            self.tools.values().map(|tool| tool.schema()).collect()
        }
        fn get(&self, name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
            self.tools.get(name).cloned()
        }
    }

    let tool_catalog = Arc::new(LocalToolCatalog { tools });

    let runtime = AgentRuntimeBuilder::new()
        .provider(Arc::new(ToolThenEndTurnMockProvider::new()))
        .tools(tool_catalog)
        .context_pipeline(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(Arc::new(BlockCompositionHooks))
        .build()
        .unwrap();

    let input = UserInput {
        prompt: "hi".to_string(),
        session_id: None,
        cancel_token: gestalt_core::cancel::CancelToken::new(),
        event_tx: None,
        artifact_dir: None,
    };

    let res = runtime.run_prompt(input).await;
    assert!(res.is_ok());
    let run_res = res.unwrap();
    assert_eq!(run_res.stop_reason, StopReason::HookBlocked);
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
        Some(Arc::new(ContextMessageAssembler::new("pipeline-v1")))
    }
}

struct MockPolicyEngine;
#[async_trait::async_trait]
impl PolicyEngine for MockPolicyEngine {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

fn build_test_runtime() -> gestalt_runtime::unstable::runtime::AgentRuntime {
    AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider))
        .tools(Arc::new(MockToolCatalog))
        .context_pipeline(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
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
    assert!(matches!(
        events[0],
        AgentEvent::WorkspaceSnapshotCaptured { .. }
    ));
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
            top_p: None,
            reasoning_effort: None,
            text_verbosity: None,
            metadata: serde_json::Value::Null,
            resolved_model: None,
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
            ignore_patterns: Vec::new(),
        },
        gestalt_core::session::ExecutionMode::Yolo,
        snapshot,
    );
    session.context_policy = gestalt_core::ContextManagementPolicy {
        enabled: false,
        ..Default::default()
    };

    // Prepopulate session history
    session.append_message(Message::User {
        content: vec![gestalt_core::message::ContentBlock::Text {
            text: "Initial user turn".to_string(),
        }],
        metadata: None,
    });
    session.append_message(Message::Assistant {
        content: vec![gestalt_core::message::ContentBlock::Text {
            text: "Initial assistant turn".to_string(),
        }],
    });

    let res = runtime.run_session(&mut session, &cancel, None, None).await;
    assert!(res.is_ok());

    // Verify history was preserved and not reset
    assert_eq!(session.history.len(), 3);
    if let Message::User { content, .. } = &session.history[0].message {
        if let gestalt_core::message::ContentBlock::Text { text } = &content[0] {
            assert_eq!(text, "Initial user turn");
        }
    }
    if let Message::Assistant { content } = &session.history[1].message {
        if let gestalt_core::message::ContentBlock::Text { text } = &content[0] {
            assert_eq!(text, "Initial assistant turn");
        }
    }
}

#[tokio::test]
async fn test_runtime_context_hook_persists_prompt_snapshot() {
    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-runtime-snapshot-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let artifact_dir = temp_dir.join("artifacts");
    let adapter = RuntimeContextHookAdapter {
        hooks: Arc::new(NoopCompositionHooks),
        patch_store: Arc::new(std::sync::Mutex::new(Vec::new())),
        contributors: vec![],
        workspace_root: temp_dir.clone(),
        block_reason: None,
        event_bus: RuntimeEventBus::new(),
        prompt_snapshot_state: Arc::new(std::sync::Mutex::new(None)),
        #[cfg(feature = "skills")]
        skill_state: None,
    };

    let session = Session::new(
        "test-session",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 5,
            top_p: None,
            reasoning_effort: None,
            text_verbosity: None,
            metadata: serde_json::Value::Null,
            resolved_model: None,
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
            working_dir: temp_dir.clone(),
            workspace_root: Some(temp_dir.clone()),
            timeout: Duration::from_secs(1),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: 100,
            artifact_dir: Some(artifact_dir.clone()),
            current_tool_call_id: None,
            ignore_patterns: Vec::new(),
        },
        gestalt_core::session::ExecutionMode::Yolo,
        gestalt_core::snapshot::WorkspaceSnapshot {
            workspace_root: temp_dir.clone(),
            git_sha: None,
            git_dirty: None,
            untracked_count: None,
            content_hash: "mock-hash".to_string(),
            captured_at: chrono::Utc::now(),
        },
    );

    let stable_message = Message::System {
        content: "stable prefix".to_string(),
    };
    let dynamic_message = Message::User {
        content: vec![ContentBlock::Text {
            text: "latest turn".to_string(),
        }],
        metadata: None,
    };
    let snapshot = PromptSnapshot::new(vec![stable_message.clone()], 0);
    let cache_plan = PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot);
    let packet = gestalt_core::context::ContextPacket {
        messages: vec![stable_message, dynamic_message],
        packet_hash: "packet-hash".to_string(),
        pipeline_version: "mock-v1".to_string(),
        tokenizer_id: "default".to_string(),
        token_estimate: 12,
        sources: vec![],
        omissions: vec![],
        message_hashes: vec![],
        prompt_assembly_strategy: PromptAssemblyStrategy::Snapshot,
        snapshot_hash: Some(snapshot.snapshot_hash.clone()),
        cache_prefix_hash: Some(snapshot.prefix_hash.clone()),
        segments: vec![],
        cache_plan: Some(cache_plan),
        prompt_source: Some("default".to_string()),
    };

    let events_first = adapter
        .after_context_build(&session, &packet)
        .await
        .expect("after_context_build succeeds");
    assert!(matches!(
        events_first[0],
        AgentEvent::PromptSnapshotCreated { .. }
    ));
    assert!(matches!(
        events_first[1],
        AgentEvent::PromptCachePlanGenerated { .. }
    ));

    let persisted =
        gestalt_runtime::unstable::read_prompt_snapshot(artifact_dir.join("prompt-snapshot.json"))
            .expect("prompt snapshot persisted");
    assert_eq!(persisted.snapshot_hash, snapshot.snapshot_hash);

    let events_second = adapter
        .after_context_build(&session, &packet)
        .await
        .expect("after_context_build succeeds again");
    assert!(matches!(
        events_second[0],
        AgentEvent::PromptSnapshotReused { .. }
    ));

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_runtime_context_pipeline_keeps_cache_metadata_for_stable_patches() {
    struct SnapshotPipeline;
    use gestalt_core::context::{ContextAssembler, ContextPlan, PromptCachePlan};

    impl ContextAssembler for SnapshotPipeline {
        fn version(&self) -> &str {
            "pipeline-v1"
        }

        fn system_messages(&self) -> Vec<Message> {
            vec![Message::System {
                content: "stable system prefix".to_string(),
            }]
        }

        fn assemble(
            &self,
            _plan: &ContextPlan,
        ) -> std::result::Result<
            gestalt_core::context::ContextPacket,
            gestalt_core::error::ContextError,
        > {
            let messages = self.system_messages();
            let snapshot = PromptSnapshot::new(messages.clone(), 0);
            let plan = PromptCachePlan::new(PromptAssemblyStrategy::Snapshot, &snapshot);

            Ok(gestalt_core::context::ContextPacket {
                messages,
                packet_hash: "base-packet-hash".to_string(),
                pipeline_version: self.version().to_string(),
                tokenizer_id: "default".to_string(),
                token_estimate: 0,
                sources: vec![],
                omissions: vec![],
                message_hashes: vec![],
                prompt_assembly_strategy: PromptAssemblyStrategy::Snapshot,
                snapshot_hash: Some(snapshot.snapshot_hash),
                cache_prefix_hash: Some(snapshot.prefix_hash),
                segments: vec![],
                cache_plan: Some(plan),
                prompt_source: Some("default".to_string()),
            })
        }
    }

    let pipeline = RuntimeContextPipeline {
        base: Arc::new(SnapshotPipeline),
        patch_store: Arc::new(std::sync::Mutex::new(vec![ContextPatch::new(
            Message::System {
                content: "extension snapshot context".to_string(),
            },
            gestalt_core::ContextStability::SessionStatic,
        )])),
    };

    let packet = pipeline.build_packet(
        &[],
        &TokenBudget {
            model_limit: 256,
            reserved_output: 16,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 8,
        },
    );

    assert_eq!(
        packet.prompt_assembly_strategy,
        PromptAssemblyStrategy::Snapshot
    );
    assert!(packet.snapshot_hash.is_some());
    assert!(packet.cache_prefix_hash.is_some());
    assert!(packet.cache_plan.is_some());
    assert!(packet
        .segments
        .iter()
        .any(|segment| segment.kind == gestalt_core::context::PromptSegmentKind::Snapshot));
}

struct NoopProvider;

#[async_trait::async_trait]
impl Provider for NoopProvider {
    fn id(&self) -> &str {
        "noop"
    }

    fn display_name(&self) -> &str {
        "Noop"
    }

    fn default_model(&self) -> &str {
        "noop-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        static CAP: ProviderCapabilities = ProviderCapabilities {
            supports_tools: true,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: true,
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
        messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(messages
            .iter()
            .map(gestalt_runtime::unstable::estimate_message_tokens)
            .sum())
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

struct OverheadProvider;

#[async_trait::async_trait]
impl Provider for OverheadProvider {
    fn id(&self) -> &str {
        "overhead"
    }

    fn display_name(&self) -> &str {
        "Overhead"
    }

    fn default_model(&self) -> &str {
        "overhead-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        static CAP: ProviderCapabilities = ProviderCapabilities {
            supports_tools: true,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: true,
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
        messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(messages
            .iter()
            .map(gestalt_runtime::unstable::estimate_message_tokens)
            .sum())
    }

    fn count_request_tokens(
        &self,
        request: &ProviderRequest,
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        let base: usize = request
            .messages
            .iter()
            .map(gestalt_runtime::unstable::estimate_message_tokens)
            .sum();
        Ok(base.saturating_add(request.tools.len().saturating_mul(512)))
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        Ok(Box::pin(futures::stream::empty()))
    }
}

#[tokio::test]
async fn test_prepare_context_uses_full_history_when_management_enabled() {
    let pipeline = RuntimeContextPipeline {
        base: Arc::new(ContextMessageAssembler::new("pipeline-v1").with_prompt_override("prompt")),
        patch_store: Arc::new(std::sync::Mutex::new(Vec::new())),
    };

    let history: Vec<gestalt_core::SessionMessage> = (0..8)
        .map(|idx| gestalt_core::SessionMessage {
            id: gestalt_core::MessageId {
                origin_session_id: "session-1".to_string(),
                origin_message_namespace: "session-1".to_string(),
                sequence: u64::try_from(idx).expect("test index should fit into u64"),
            },
            message: Message::User {
                content: vec![ContentBlock::Text {
                    text: format!("message-{idx}-{}", "x".repeat(80)),
                }],
                metadata: None,
            },
            metadata: None,
        })
        .collect();

    let budget = TokenBudget {
        model_limit: 180,
        reserved_output: 16,
        used_system: 0,
        used_history: 0,
        used_sources: 0,
        used_tools: 0,
        used_memory: 0,
        minimum_turn_budget: 0,
    };

    let truncated_packet = pipeline.build_packet(&history, &budget);
    assert!(truncated_packet.token_estimate <= 164);

    let policy = gestalt_core::ContextManagementPolicy {
        enabled: true,
        buffer_tokens: 0,
        keep_recent_tokens: usize::MAX,
        keep_recent_turns: usize::MAX,
        durability: gestalt_core::DurabilityMode::BestEffort,
        ..Default::default()
    };
    let request_template = ProviderRequest {
        model: "noop-model".to_string(),
        max_tokens: 128,
        ..Default::default()
    };

    let err = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget,
            provider: &NoopProvider,
            request_template: &request_template,
            model: "noop-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &policy,
            artifacts_dir: None,
            tool_retention: &gestalt_core::ToolRetentionRegistrySnapshot::default(),
            emit: &mut |_| Ok(()),
        })
        .await
        .expect_err("full-history pressure should not be hidden by pre-truncation");

    assert!(format!("{err}").contains("exceeds limit"));
}

#[tokio::test]
async fn test_prepare_context_counts_tool_schema_overhead() {
    let pipeline = RuntimeContextPipeline {
        base: Arc::new(ContextMessageAssembler::new("pipeline-v1").with_prompt_override("prompt")),
        patch_store: Arc::new(std::sync::Mutex::new(Vec::new())),
    };

    let history = vec![gestalt_core::SessionMessage {
        id: gestalt_core::MessageId {
            origin_session_id: "session-1".to_string(),
            origin_message_namespace: "session-1".to_string(),
            sequence: 0,
        },
        message: Message::User {
            content: vec![ContentBlock::Text {
                text: "short request".to_string(),
            }],
            metadata: None,
        },
        metadata: None,
    }];
    let budget = TokenBudget {
        model_limit: 240,
        reserved_output: 16,
        used_system: 0,
        used_history: 0,
        used_sources: 0,
        used_tools: 0,
        used_memory: 0,
        minimum_turn_budget: 0,
    };
    let request_template = ProviderRequest {
        model: "overhead-model".to_string(),
        tools: vec![gestalt_core::provider::ProviderToolSchema {
            name: "expensive_tool".to_string(),
            description: "large schema".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
            }),
            strict: None,
        }],
        max_tokens: 128,
        ..Default::default()
    };

    let err = pipeline
        .prepare_context(gestalt_core::ContextPreparationRequest {
            history: &history,
            context_state: &gestalt_core::ContextProjectionState::default(),
            token_budget: &budget,
            provider: &OverheadProvider,
            request_template: &request_template,
            model: "overhead-model",
            session_id: "session-1",
            run_id: "run-1",
            turn_id: 0,
            policy: &gestalt_core::ContextManagementPolicy {
                enabled: true,
                buffer_tokens: 0,
                keep_recent_tokens: usize::MAX,
                keep_recent_turns: usize::MAX,
                durability: gestalt_core::DurabilityMode::BestEffort,
                ..Default::default()
            },
            artifacts_dir: None,
            tool_retention: &gestalt_core::ToolRetentionRegistrySnapshot::default(),
            emit: &mut |_| Ok(()),
        })
        .await
        .expect_err("tool schema overhead should participate in fit checks");

    assert!(format!("{err}").contains("exceeds limit"));
}
