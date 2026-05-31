use std::{
    collections::{HashMap, VecDeque},
    error::Error as _,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures::stream;
use gestalt_core::{
    agent::AgentLoop,
    approval::{ApprovalDecision, ApprovalProvider, ApprovalRequest, AutoApprovalProvider},
    context::{ContextPipeline, TokenBudget},
    error::{HarnessError, ProviderError},
    event::{AgentEvent, PolicyStatus, StopReason},
    message::{ContentBlock, Message},
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    session::{ExecutionMode, RunResult, Session, SessionConfig},
    tool::{RiskLevel, Tool, ToolCatalog, ToolContext, ToolOutput, ToolSchema},
    turn::TurnAccumulator,
};
use serde_json::{json, Value};

#[test]
fn contract_types_round_trip_through_serde() {
    let message = Message::Assistant {
        content: vec![
            ContentBlock::Text {
                text: "hello".to_string(),
            },
            ContentBlock::ToolUse {
                id: "call-1".to_string(),
                name: "read".to_string(),
                input: json!({"path":"README.md"}),
            },
        ],
    };
    let encoded = serde_json::to_string(&message).expect("message encodes");
    let decoded: Message = serde_json::from_str(&encoded).expect("message decodes");
    assert_eq!(message, decoded);

    let event = AgentEvent::ToolResult {
        id: "call-1".to_string(),
        output: "done".to_string(),
        is_error: false,
        truncated: false,
    };
    let encoded = serde_json::to_string(&event).expect("event encodes");
    let decoded: AgentEvent = serde_json::from_str(&encoded).expect("event decodes");
    assert_eq!(event, decoded);

    let result = RunResult {
        session_id: "session-1".to_string(),
        turns: 2,
        stop_reason: StopReason::EndTurn,
        total_input_tokens: 11,
        total_output_tokens: 7,
        artifacts: vec!["artifact.txt".to_string()],
    };
    let encoded = serde_json::to_string(&result).expect("run result encodes");
    let decoded: RunResult = serde_json::from_str(&encoded).expect("run result decodes");
    assert_eq!(result, decoded);
}

#[test]
fn contract_traits_are_object_safe() {
    let provider = mock_provider(vec![]);
    let tools = Arc::new(MockCatalog::default());
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    accepts_trait_objects(provider, tools, pipeline, policy, approval);
}

#[test]
fn error_display_and_source_are_preserved() {
    let io_error = std::io::Error::other("boom");
    let err = HarnessError::Provider(ProviderError::Transport(io_error));

    assert!(format!("{err}").contains("provider error"));
    assert!(err.source().is_some());
}

#[test]
fn turn_accumulator_collects_streamed_tool_calls() {
    let mut accumulator = TurnAccumulator::default();

    accumulator
        .record(&AgentEvent::Text {
            delta: "hello".to_string(),
        })
        .expect("text accumulates");
    accumulator
        .record(&AgentEvent::ToolCallStreamed {
            id: "call-1".to_string(),
            name: "read".to_string(),
            input_delta: "{\"path\":\"".to_string(),
        })
        .expect("tool call accumulates");
    accumulator
        .record(&AgentEvent::ToolCallStreamed {
            id: "call-1".to_string(),
            name: "read".to_string(),
            input_delta: "README.md\"}".to_string(),
        })
        .expect("tool call completes");
    accumulator
        .record(&AgentEvent::Stop {
            reason: StopReason::ToolUse,
        })
        .expect("stop records");

    let turn = accumulator.finish().expect("turn finalizes");
    assert_eq!(turn.full_text(), "hello");
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].id, "call-1");
    assert_eq!(turn.tool_calls[0].name, "read");
    assert_eq!(turn.tool_calls[0].input, json!({"path":"README.md"}));
}

#[tokio::test]
async fn agent_loop_handles_text_only_turn() {
    let provider = mock_provider(vec![vec![
        AgentEvent::Text {
            delta: "final answer".to_string(),
        },
        AgentEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]]);
    let tools = Arc::new(MockCatalog::default());
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);
    session.history.push(Message::User {
        content: vec![ContentBlock::Text {
            text: "question".to_string(),
        }],
    });

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.turns, 1);
    assert_eq!(session.history.len(), 2);
}

#[tokio::test]
async fn agent_loop_executes_single_tool_call() {
    let tool = Arc::new(MockTool::new("read", true, "tool result"));
    let provider = mock_provider(vec![
        vec![
            AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "read".to_string(),
                input_delta: "{\"path\":\"README.md\"}".to_string(),
            },
            AgentEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.turns, 2);
    assert_eq!(tool.executed_inputs.lock().expect("lock").len(), 1);
    assert!(session.history.iter().any(|message| matches!(message, Message::ToolResult { tool_use_id, content, is_error } if tool_use_id == "call-1" && content == "tool result" && !is_error)));
}

#[tokio::test]
async fn agent_loop_preserves_original_tool_result_order() {
    let first = Arc::new(MockTool::new("alpha", true, "alpha result"));
    let second = Arc::new(MockTool::new("beta", false, "beta result"));
    let provider = mock_provider(vec![
        vec![
            AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "beta".to_string(),
                input_delta: "{\"value\":\"b\"}".to_string(),
            },
            AgentEvent::ToolCallStreamed {
                id: "call-2".to_string(),
                name: "alpha".to_string(),
                input_delta: "{\"value\":\"a\"}".to_string(),
            },
            AgentEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![first.clone(), second.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.turns, 2);
    let tool_results = session
        .history
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some((tool_use_id.clone(), content.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_results,
        vec![
            ("call-1".to_string(), "beta result".to_string()),
            ("call-2".to_string(), "alpha result".to_string()),
        ]
    );
}

#[tokio::test]
async fn agent_loop_denies_tool_call_as_error_result() {
    let tool = Arc::new(MockTool::new("write", false, "should not run"));
    let provider = mock_provider(vec![vec![
        AgentEvent::ToolCallStreamed {
            id: "call-1".to_string(),
            name: "write".to_string(),
            input_delta: "{\"path\":\"file.txt\"}".to_string(),
        },
        AgentEvent::Stop {
            reason: StopReason::ToolUse,
        },
    ]]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::deny_all("blocked by policy"));
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.turns, 2);
    assert!(tool.executed_inputs.lock().expect("lock").is_empty());
    assert!(session
        .history
        .iter()
        .any(|message| matches!(message, Message::ToolResult { tool_use_id, is_error, .. } if tool_use_id == "call-1" && *is_error)));
}

#[tokio::test]
async fn agent_loop_routes_confirm_calls_through_approval() {
    let tool = Arc::new(MockTool::new("edit", true, "approved"));
    let provider = mock_provider(vec![
        vec![
            AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "edit".to_string(),
                input_delta: "{\"value\":\"original\"}".to_string(),
            },
            AgentEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::confirm_all("confirm required"));
    let approval = Arc::new(MockApproval::approve_all());
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Confirm);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.turns, 2);
    assert_eq!(tool.executed_inputs.lock().expect("lock").len(), 1);
}

#[tokio::test]
async fn agent_loop_stops_on_max_turns() {
    let provider = mock_provider(vec![
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);
    let tools = Arc::new(MockCatalog::default());
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 1);
    let mut session = make_session(ExecutionMode::Yolo);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.stop_reason, StopReason::MaxTurns);
}

#[tokio::test]
async fn agent_loop_stops_on_budget_exhaustion() {
    let provider = mock_provider(vec![vec![AgentEvent::Stop {
        reason: StopReason::EndTurn,
    }]]);
    let tools = Arc::new(MockCatalog::default());
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);
    session.token_budget = TokenBudget {
        model_limit: 16,
        reserved_output: 8,
        used_system: 0,
        used_history: 0,
        used_sources: 0,
        used_tools: 0,
        used_memory: 0,
        minimum_turn_budget: 16,
    };

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.stop_reason, StopReason::BudgetExhausted);
}

fn accepts_trait_objects(
    _provider: Arc<dyn Provider>,
    _tools: Arc<dyn ToolCatalog>,
    _pipeline: Arc<dyn ContextPipeline>,
    _policy: Arc<dyn PolicyEngine>,
    _approval: Arc<dyn ApprovalProvider>,
) {
}

fn make_session(mode: ExecutionMode) -> Session {
    Session::new(
        "session-1",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 128,
            temperature: Some(0.0),
            max_turns: 3,
        },
        TokenBudget {
            model_limit: 256,
            reserved_output: 32,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 8,
        },
        ToolContext {
            working_dir: std::env::current_dir().expect("cwd"),
            workspace_root: Some(std::env::current_dir().expect("cwd")),
            timeout: Duration::from_secs(1),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: 1024,
        },
        mode,
    )
}

fn mock_provider(turns: Vec<Vec<AgentEvent>>) -> Arc<MockProvider> {
    Arc::new(MockProvider {
        turns: Mutex::new(turns.into_iter().collect()),
        capabilities: ProviderCapabilities::default(),
    })
}

struct MockProvider {
    turns: Mutex<VecDeque<Vec<AgentEvent>>>,
    capabilities: ProviderCapabilities,
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn default_model(&self) -> &str {
        "mock-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    fn count_tokens(&self, messages: &[Message]) -> usize {
        messages.len().saturating_mul(8)
    }

    async fn stream(&self, _request: ProviderRequest) -> Result<EventStream, HarnessError> {
        let events = self
            .turns
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or_else(|| {
                vec![AgentEvent::Stop {
                    reason: StopReason::EndTurn,
                }]
            });

        let stream = stream::iter(events.into_iter().map(Ok::<_, HarnessError>));
        Ok(Box::pin(stream))
    }
}

struct MockTool {
    name: String,
    parallel_safe: bool,
    output: String,
    executed_inputs: Arc<Mutex<Vec<Value>>>,
}

impl MockTool {
    fn new(name: impl Into<String>, parallel_safe: bool, output: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            parallel_safe,
            output: output.into(),
            executed_inputs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "mock tool"
    }

    fn schema(&self) -> ToolSchema {
        json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            }
        })
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        if self.parallel_safe {
            RiskLevel::Low
        } else {
            RiskLevel::Medium
        }
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        self.parallel_safe
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::ToolError> {
        self.executed_inputs.lock().expect("lock").push(input);
        Ok(ToolOutput::Text {
            content: self.output.clone(),
        })
    }
}

#[derive(Default)]
struct MockCatalog {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl MockCatalog {
    fn with_tools(tools: Vec<Arc<MockTool>>) -> Self {
        let mut catalog = Self::default();
        for tool in tools {
            catalog.tools.insert(tool.name.clone(), tool);
        }
        catalog
    }
}

#[async_trait::async_trait]
impl ToolCatalog for MockCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|tool| tool.schema()).collect()
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
}

#[derive(Default)]
struct MockPipeline;

impl ContextPipeline for MockPipeline {
    fn process(&self, history: &[Message], _budget: &TokenBudget) -> Vec<Message> {
        history.to_vec()
    }

    fn version(&self) -> &str {
        "mock-pipeline"
    }
}

struct MockPolicy {
    decision_for: Arc<dyn Fn(&PolicyRequest) -> PolicyDecision + Send + Sync>,
}

impl MockPolicy {
    fn allow_all() -> Self {
        Self {
            decision_for: Arc::new(|_request| PolicyDecision {
                status: PolicyStatus::Allowed,
                reason: None,
                policy_source: "allow-all".to_string(),
            }),
        }
    }

    fn deny_all(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            decision_for: Arc::new(move |_request| PolicyDecision {
                status: PolicyStatus::Denied,
                reason: Some(reason.clone()),
                policy_source: "deny-all".to_string(),
            }),
        }
    }

    fn confirm_all(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            decision_for: Arc::new(move |_request| PolicyDecision {
                status: PolicyStatus::Confirm,
                reason: Some(reason.clone()),
                policy_source: "confirm-all".to_string(),
            }),
        }
    }
}

#[async_trait::async_trait]
impl PolicyEngine for MockPolicy {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        (self.decision_for)(&request)
    }
}

struct MockApproval {
    decision_for: Arc<dyn Fn(&ApprovalRequest) -> ApprovalDecision + Send + Sync>,
}

impl MockApproval {
    fn approve_all() -> Self {
        Self {
            decision_for: Arc::new(|_request| ApprovalDecision::Approve),
        }
    }
}

#[async_trait::async_trait]
impl ApprovalProvider for MockApproval {
    async fn approve(&self, request: ApprovalRequest) -> ApprovalDecision {
        (self.decision_for)(&request)
    }
}
