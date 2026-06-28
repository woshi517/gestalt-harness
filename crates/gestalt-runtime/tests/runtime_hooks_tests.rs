#![allow(deprecated)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gestalt_context::ContextMessageAssembler;
use gestalt_core::{
    approval::AutoApprovalProvider,
    context::{ContextPipeline, TokenBudget},
    error::ToolError,
    event::{AgentEvent, PolicyStatus, StopReason},
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolContext, ToolOutput, ToolSchema},
};
use gestalt_runtime as gestalt_context;
use gestalt_runtime::{
    AfterContextBuildCtx, AfterToolResultCtx, AgentRuntimeBuilder, BeforeContextBuildCtx,
    BeforeToolPolicyCtx, CompositionHooks, HookOutcome, OnEventCtx, RuntimeConfig, UserInput,
};

fn config_without_context_management() -> RuntimeConfig {
    let mut config = RuntimeConfig::default();
    config.context_management_policy = Some(gestalt_core::ContextManagementPolicy {
        enabled: false,
        ..Default::default()
    });
    config
}

struct MockProvider {
    last_request: Arc<Mutex<Option<ProviderRequest>>>,
    stream_events: Mutex<Vec<Vec<AgentEvent>>>,
}

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
        let mut req_lock = self.last_request.lock().unwrap();
        *req_lock = Some(request);

        let mut events_lock = self.stream_events.lock().unwrap();
        let events = if events_lock.is_empty() {
            vec![AgentEvent::Stop {
                reason: StopReason::EndTurn,
            }]
        } else {
            events_lock.remove(0)
        };
        let stream = futures::stream::iter(
            events
                .into_iter()
                .map(Ok::<_, gestalt_core::error::HarnessError>),
        );
        Ok(Box::pin(stream))
    }
}

struct MockTool {
    name: String,
}

#[async_trait::async_trait]
impl gestalt_core::tool::Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "test description"
    }
    fn schema(&self) -> ToolSchema {
        serde_json::from_value(serde_json::json!({
            "name": self.name.clone(),
            "description": "test description",
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
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Text {
            content: "test result".to_string(),
        })
    }
}

struct MockToolCatalog {
    tools: HashMap<String, Arc<dyn gestalt_core::tool::Tool>>,
}

impl ToolCatalog for MockToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|t| t.schema()).collect()
    }
    fn get(&self, name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        self.tools.get(name).cloned()
    }
}

struct MockContextPipeline;
impl ContextPipeline for MockContextPipeline {
    fn process(
        &self,
        _history: &[gestalt_core::SessionMessage],
        _budget: &TokenBudget,
    ) -> Vec<Message> {
        vec![Message::System {
            content: "base system instructions".to_string(),
        }]
    }
    fn version(&self) -> &str {
        "base-v1"
    }

    fn as_assembler(&self) -> Option<Arc<dyn gestalt_core::context::ContextAssembler>> {
        Some(Arc::new(
            ContextMessageAssembler::new("pipeline-v1")
                .with_prompt_override("base system instructions"),
        ))
    }
}

struct MockPolicyEngine;
#[async_trait::async_trait]
impl PolicyEngine for MockPolicyEngine {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

struct TestCompositionHooks {
    add_context: Mutex<Option<Message>>,
    block_reason: Mutex<Option<String>>,
    policy_outcome: Mutex<Option<gestalt_runtime::Result<HookOutcome>>>,
    after_context_outcome: Mutex<Option<HookOutcome>>,
    events: Mutex<Vec<AgentEvent>>,
    before_tool_policy_calls: Mutex<usize>,
}

#[async_trait::async_trait]
impl CompositionHooks for TestCompositionHooks {
    async fn before_context_build(
        &self,
        _context: &BeforeContextBuildCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        let block_reason = self.block_reason.lock().unwrap().clone();
        if let Some(reason) = block_reason {
            return Ok(HookOutcome::Block { reason });
        }
        let add_context = self.add_context.lock().unwrap().clone();
        if let Some(msg) = add_context {
            return Ok(HookOutcome::AddContext { message: msg });
        }
        Ok(HookOutcome::Continue)
    }

    async fn after_context_build(
        &self,
        _context: &AfterContextBuildCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        let outcome = self.after_context_outcome.lock().unwrap().clone();
        if let Some(outcome) = outcome {
            Ok(outcome)
        } else {
            Ok(HookOutcome::Continue)
        }
    }

    async fn before_tool_policy(
        &self,
        _context: &BeforeToolPolicyCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        *self.before_tool_policy_calls.lock().unwrap() += 1;
        if let Some(outcome) = self.policy_outcome.lock().unwrap().as_ref() {
            match outcome {
                Ok(o) => Ok(o.clone()),
                Err(_) => Err(gestalt_runtime::error::RuntimeError::Harness(
                    gestalt_core::error::HarnessError::Cancelled,
                )),
            }
        } else {
            Ok(HookOutcome::Continue)
        }
    }

    async fn after_tool_result(
        &self,
        _context: &AfterToolResultCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn prepare_next_turn(
        &self,
        _context: &gestalt_runtime::PrepareNextTurnCtx,
    ) -> gestalt_runtime::Result<HookOutcome> {
        Ok(HookOutcome::Continue)
    }

    async fn on_event(&self, context: &OnEventCtx) -> gestalt_runtime::Result<()> {
        self.events.lock().unwrap().push(context.event.clone());
        Ok(())
    }
}

#[tokio::test]
async fn test_hooks_context_injection() {
    let hooks = Arc::new(TestCompositionHooks {
        add_context: Mutex::new(Some(Message::System {
            content: "hook injected system content".to_string(),
        })),
        block_reason: Mutex::new(None),
        policy_outcome: Mutex::new(None),
        after_context_outcome: Mutex::new(None),
        events: Mutex::new(Vec::new()),
        before_tool_policy_calls: Mutex::new(0),
    });

    let last_request = Arc::new(Mutex::new(None));
    let provider = Arc::new(MockProvider {
        last_request: last_request.clone(),
        stream_events: Mutex::new(vec![vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }]]),
    });

    let runtime = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(Arc::new(MockToolCatalog {
            tools: HashMap::new(),
        }))
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(hooks.clone())
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

    // Verify context injection actually reaches the built prompt
    let req = last_request
        .lock()
        .unwrap()
        .clone()
        .expect("Provider should have received a request");
    let contains_injected = req.messages.iter().any(|msg| {
        if let Message::System { content } = msg {
            content.contains("hook injected system content")
        } else {
            false
        }
    });
    assert!(
        contains_injected,
        "ProviderRequest should contain the injected context message"
    );

    // Verify events were observed by our on_event hook
    let observed = hooks.events.lock().unwrap().clone();
    assert!(!observed.is_empty());

    // Check that we observed ContextBuildStarted or similar turn events
    assert!(observed
        .iter()
        .any(|ev| matches!(ev, AgentEvent::ContextBuildStarted)));
}

#[test]
fn test_composition_hooks_require_assembler_backed_pipeline() {
    struct LegacyOnlyContextPipeline;

    impl ContextPipeline for LegacyOnlyContextPipeline {
        fn process(
            &self,
            _history: &[gestalt_core::SessionMessage],
            _budget: &TokenBudget,
        ) -> Vec<Message> {
            vec![Message::System {
                content: "legacy-only pipeline".to_string(),
            }]
        }

        fn version(&self) -> &str {
            "legacy-v1"
        }
    }

    let hooks = Arc::new(TestCompositionHooks {
        add_context: Mutex::new(None),
        block_reason: Mutex::new(None),
        policy_outcome: Mutex::new(None),
        after_context_outcome: Mutex::new(None),
        events: Mutex::new(Vec::new()),
        before_tool_policy_calls: Mutex::new(0),
    });

    let result = AgentRuntimeBuilder::new()
        .provider(Arc::new(MockProvider {
            last_request: Arc::new(Mutex::new(None)),
            stream_events: Mutex::new(vec![vec![AgentEvent::Stop {
                reason: StopReason::EndTurn,
            }]]),
        }))
        .tools(Arc::new(MockToolCatalog {
            tools: HashMap::new(),
        }))
        .middleware(Arc::new(LegacyOnlyContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(hooks)
        .build();

    let err = match result {
        Ok(_) => panic!("legacy-only pipelines must not be wrapped under composition hooks"),
        Err(err) => err,
    };

    assert!(err
        .to_string()
        .contains("runtime requires an assembler-backed context pipeline"));
}

#[tokio::test]
async fn test_hooks_context_blocking_before() {
    let hooks = Arc::new(TestCompositionHooks {
        add_context: Mutex::new(None),
        block_reason: Mutex::new(Some("safety block".to_string())),
        policy_outcome: Mutex::new(None),
        after_context_outcome: Mutex::new(None),
        events: Mutex::new(Vec::new()),
        before_tool_policy_calls: Mutex::new(0),
    });

    let last_request = Arc::new(Mutex::new(None));
    let provider = Arc::new(MockProvider {
        last_request: last_request.clone(),
        stream_events: Mutex::new(vec![vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }]]),
    });

    let runtime = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(Arc::new(MockToolCatalog {
            tools: HashMap::new(),
        }))
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(hooks.clone())
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
    assert!(res.is_err(), "Run should fail when context hook blocks");

    // Verify that the error represents policy denial with block reason
    let err_str = res.unwrap_err().to_string();
    assert!(err_str.contains("safety block"));
}

#[tokio::test]
async fn test_hooks_context_blocking_after() {
    let hooks = Arc::new(TestCompositionHooks {
        add_context: Mutex::new(None),
        block_reason: Mutex::new(None),
        policy_outcome: Mutex::new(None),
        after_context_outcome: Mutex::new(Some(HookOutcome::Block {
            reason: "after block reason".to_string(),
        })),
        events: Mutex::new(Vec::new()),
        before_tool_policy_calls: Mutex::new(0),
    });

    let last_request = Arc::new(Mutex::new(None));
    let provider = Arc::new(MockProvider {
        last_request: last_request.clone(),
        stream_events: Mutex::new(vec![vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }]]),
    });

    let runtime = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(Arc::new(MockToolCatalog {
            tools: HashMap::new(),
        }))
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(hooks.clone())
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
    assert!(
        res.is_err(),
        "Run should fail when context hook blocks in after_context_build"
    );

    let err_str = res.unwrap_err().to_string();
    assert!(err_str.contains("after block reason"));
}

#[tokio::test]
async fn test_before_tool_policy_denial() {
    let hooks = Arc::new(TestCompositionHooks {
        add_context: Mutex::new(None),
        block_reason: Mutex::new(None),
        policy_outcome: Mutex::new(Some(Ok(HookOutcome::Block {
            reason: "policy hook denied tool".to_string(),
        }))),
        after_context_outcome: Mutex::new(None),
        events: Mutex::new(Vec::new()),
        before_tool_policy_calls: Mutex::new(0),
    });

    let last_request = Arc::new(Mutex::new(None));
    let provider = Arc::new(MockProvider {
        last_request: last_request.clone(),
        stream_events: Mutex::new(vec![
            vec![
                AgentEvent::ToolCallStreamed {
                    id: "call-1".to_string(),
                    name: "test-tool".to_string(),
                    input_delta: "{}".to_string(),
                },
                AgentEvent::Stop {
                    reason: StopReason::ToolUse,
                },
            ],
            vec![AgentEvent::Stop {
                reason: StopReason::EndTurn,
            }],
        ]),
    });

    let mut tools = HashMap::new();
    tools.insert(
        "test-tool".to_string(),
        Arc::new(MockTool {
            name: "test-tool".to_string(),
        }) as Arc<dyn gestalt_core::tool::Tool>,
    );
    let catalog = Arc::new(MockToolCatalog { tools });

    let runtime = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(catalog)
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(hooks.clone())
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
    assert_eq!(*hooks.before_tool_policy_calls.lock().unwrap(), 1);

    // Verify policy engine denied event was observed
    let observed = hooks.events.lock().unwrap().clone();
    let denied = observed.iter().any(|ev| {
        if let AgentEvent::PolicyDecision { decision, .. } = ev {
            *decision == PolicyStatus::Denied
        } else {
            false
        }
    });
    assert!(denied, "Tool execution should have been denied by policy");

    let history = runtime.event_bus.history();
    let started = history
        .iter()
        .filter(|event| {
            matches!(
                event,
                gestalt_runtime::RuntimeEvent::HookStarted { hook_name, .. }
                if hook_name == "before_tool_policy"
            )
        })
        .count();
    let completed = history
        .iter()
        .filter(|event| {
            matches!(
                event,
                gestalt_runtime::RuntimeEvent::HookCompleted { hook_name, .. }
                if hook_name == "before_tool_policy"
            )
        })
        .count();
    assert_eq!(started, 1);
    assert_eq!(completed, 1);
}

#[tokio::test]
async fn test_before_tool_policy_error() {
    let hooks = Arc::new(TestCompositionHooks {
        add_context: Mutex::new(None),
        block_reason: Mutex::new(None),
        policy_outcome: Mutex::new(Some(Err(gestalt_runtime::error::RuntimeError::Harness(
            gestalt_core::error::HarnessError::Cancelled,
        )))),
        after_context_outcome: Mutex::new(None),
        events: Mutex::new(Vec::new()),
        before_tool_policy_calls: Mutex::new(0),
    });

    let last_request = Arc::new(Mutex::new(None));
    let provider = Arc::new(MockProvider {
        last_request: last_request.clone(),
        stream_events: Mutex::new(vec![
            vec![
                AgentEvent::ToolCallStreamed {
                    id: "call-1".to_string(),
                    name: "test-tool".to_string(),
                    input_delta: "{}".to_string(),
                },
                AgentEvent::Stop {
                    reason: StopReason::ToolUse,
                },
            ],
            vec![AgentEvent::Stop {
                reason: StopReason::EndTurn,
            }],
        ]),
    });

    let mut tools = HashMap::new();
    tools.insert(
        "test-tool".to_string(),
        Arc::new(MockTool {
            name: "test-tool".to_string(),
        }) as Arc<dyn gestalt_core::tool::Tool>,
    );
    let catalog = Arc::new(MockToolCatalog { tools });

    let runtime = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(catalog)
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(hooks.clone())
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
    assert_eq!(*hooks.before_tool_policy_calls.lock().unwrap(), 1);

    // Verify policy engine denied event was observed due to fail-closed behavior on hook error
    let observed = hooks.events.lock().unwrap().clone();
    let denied = observed.iter().any(|ev| {
        if let AgentEvent::PolicyDecision {
            decision, reason, ..
        } = ev
        {
            *decision == PolicyStatus::Denied
                && reason
                    .as_ref()
                    .map_or(false, |r| r.contains("failed to evaluate"))
        } else {
            false
        }
    });
    assert!(
        denied,
        "Tool execution should have failed closed on hook error"
    );

    let history = runtime.event_bus.history();
    let started = history
        .iter()
        .filter(|event| {
            matches!(
                event,
                gestalt_runtime::RuntimeEvent::HookStarted { hook_name, .. }
                if hook_name == "before_tool_policy"
            )
        })
        .count();
    let failed = history
        .iter()
        .filter(|event| {
            matches!(
                event,
                gestalt_runtime::RuntimeEvent::HookFailed { hook_name, .. }
                if hook_name == "before_tool_policy"
            )
        })
        .count();
    assert_eq!(started, 1);
    assert_eq!(failed, 1);
}

#[tokio::test]
async fn test_before_tool_policy_runs_once_per_tool_call() {
    let hooks = Arc::new(TestCompositionHooks {
        add_context: Mutex::new(None),
        block_reason: Mutex::new(None),
        policy_outcome: Mutex::new(None),
        after_context_outcome: Mutex::new(None),
        events: Mutex::new(Vec::new()),
        before_tool_policy_calls: Mutex::new(0),
    });

    let provider = Arc::new(MockProvider {
        last_request: Arc::new(Mutex::new(None)),
        stream_events: Mutex::new(vec![
            vec![
                AgentEvent::ToolCallStreamed {
                    id: "call-1".to_string(),
                    name: "test-tool".to_string(),
                    input_delta: "{}".to_string(),
                },
                AgentEvent::Stop {
                    reason: StopReason::ToolUse,
                },
            ],
            vec![AgentEvent::Stop {
                reason: StopReason::EndTurn,
            }],
        ]),
    });

    let mut tools = HashMap::new();
    tools.insert(
        "test-tool".to_string(),
        Arc::new(MockTool {
            name: "test-tool".to_string(),
        }) as Arc<dyn gestalt_core::tool::Tool>,
    );

    let runtime = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(Arc::new(MockToolCatalog { tools }))
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(hooks.clone())
        .build()
        .unwrap();

    let result = runtime
        .run_prompt(UserInput {
            prompt: "hi".to_string(),
            session_id: None,
            cancel_token: gestalt_core::cancel::CancelToken::new(),
            event_tx: None,
            artifact_dir: None,
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(*hooks.before_tool_policy_calls.lock().unwrap(), 1);

    let history = runtime.event_bus.history();
    let started = history
        .iter()
        .filter(|event| {
            matches!(
                event,
                gestalt_runtime::RuntimeEvent::HookStarted { hook_name, .. }
                if hook_name == "before_tool_policy"
            )
        })
        .count();
    let completed = history
        .iter()
        .filter(|event| {
            matches!(
                event,
                gestalt_runtime::RuntimeEvent::HookCompleted { hook_name, .. }
                if hook_name == "before_tool_policy"
            )
        })
        .count();

    assert_eq!(started, 1);
    assert_eq!(completed, 1);
}

#[tokio::test]
async fn test_after_context_build_context_addition() {
    let hooks = Arc::new(TestCompositionHooks {
        add_context: Mutex::new(None),
        block_reason: Mutex::new(None),
        policy_outcome: Mutex::new(None),
        after_context_outcome: Mutex::new(Some(HookOutcome::AddContext {
            message: Message::System {
                content: "secret after_context message".to_string(),
            },
        })),
        events: Mutex::new(Vec::new()),
        before_tool_policy_calls: Mutex::new(0),
    });

    let turn1_request = Arc::new(Mutex::new(None));
    let turn2_request = Arc::new(Mutex::new(None));

    struct MultiTurnMockProvider {
        turn1_request: Arc<Mutex<Option<ProviderRequest>>>,
        turn2_request: Arc<Mutex<Option<ProviderRequest>>>,
        turn: Mutex<usize>,
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
        turn: Mutex::new(0),
    });

    let mut tools = HashMap::new();
    tools.insert(
        "test-tool".to_string(),
        Arc::new(MockTool {
            name: "test-tool".to_string(),
        }) as Arc<dyn gestalt_core::tool::Tool>,
    );
    let catalog = Arc::new(MockToolCatalog { tools });

    let runtime = AgentRuntimeBuilder::new()
        .provider(provider)
        .tools(catalog)
        .middleware(Arc::new(MockContextPipeline))
        .policy(Arc::new(MockPolicyEngine))
        .approval(Arc::new(AutoApprovalProvider))
        .config(config_without_context_management())
        .composition_hooks(hooks.clone())
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

    // Verify Turn 1 request does NOT have the secret context yet
    let req1 = turn1_request.lock().unwrap().clone().unwrap();
    let has_secret1 = req1.messages.iter().any(|msg| {
        if let Message::System { content } = msg {
            content.contains("secret after_context message")
        } else {
            false
        }
    });
    assert!(
        !has_secret1,
        "First request should not contain Turn 1's after_context addition"
    );

    // Verify Turn 2 request DOES have the secret context added in Turn 1's after_context_build
    let req2 = turn2_request.lock().unwrap().clone().unwrap();
    let has_secret2 = req2.messages.iter().any(|msg| {
        if let Message::System { content } = msg {
            content.contains("secret after_context message")
        } else {
            false
        }
    });
    assert!(
        has_secret2,
        "Second request must contain the accumulated after_context addition"
    );
}
#[allow(unused_imports)]
use gestalt_runtime as gestalt_trace;
