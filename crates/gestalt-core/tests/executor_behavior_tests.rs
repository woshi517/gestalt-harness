use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use gestalt_core::{
    agent::executor::ToolExecutor,
    approval::{ApprovalDecision, ApprovalProvider, ApprovalRequest, SessionGrant},
    cancel::CancelToken,
    context::TokenBudget,
    error::{HarnessError, ToolError, TraceError},
    event::{AgentEvent, ApprovalOutcome, PolicyStatus},
    hook::HookRegistry,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    session::{ExecutionMode, Session, SessionConfig},
    snapshot::WorkspaceSnapshot,
    tool::{RiskLevel, Tool, ToolCatalog, ToolContext, ToolExecutionResult, ToolOutput},
    tool_descriptor::{
        AnnotationSource, CanonicalToolId, ProviderToolFormat, ToolAnnotation, ToolAnnotations,
        ToolDescriptor, ToolNamespace, ToolResponseContract, ToolRetryPolicy,
    },
    tool_failure::ToolFailureKind,
    tool_name_mapping::ToolNameMapping,
    trace::TraceSink,
    turn::ProposedToolCall,
};
use serde_json::{json, Value};

mod tool_materializer;

#[derive(Clone)]
enum ScriptedResponse {
    EchoField(&'static str),
    Text(&'static str),
    Timeout,
}

struct TestTool {
    name: String,
    risk: RiskLevel,
    parallel_safe: bool,
    descriptor: ToolDescriptor,
    responses: Mutex<VecDeque<ScriptedResponse>>,
    executed_inputs: Mutex<Vec<Value>>,
}

impl TestTool {
    fn new(
        name: &str,
        risk: RiskLevel,
        parallel_safe: bool,
        retry_policy: Option<ToolRetryPolicy>,
        annotations: ToolAnnotations,
        responses: Vec<ScriptedResponse>,
    ) -> Self {
        Self {
            name: name.to_string(),
            risk,
            parallel_safe,
            descriptor: ToolDescriptor {
                id: CanonicalToolId {
                    namespace: ToolNamespace::BuiltIn,
                    name: name.to_string(),
                },
                description: format!("test tool {name}"),
                schema: json!({
                    "name": name,
                    "description": format!("test tool {name}"),
                    "input_schema": {
                        "type": "object",
                        "properties": {}
                    }
                }),
                risk,
                annotations,
                response_contract: ToolResponseContract {
                    format: ProviderToolFormat::Text,
                    shape_rules: None,
                },
                retry_policy,
                retention: None,
            },
            responses: Mutex::new(VecDeque::from(responses)),
            executed_inputs: Mutex::new(Vec::new()),
        }
    }

    fn executions(&self) -> Vec<Value> {
        self.executed_inputs.lock().unwrap().clone()
    }
}

fn materializer() -> Arc<dyn gestalt_core::tool::ToolOutputMaterializer> {
    Arc::new(tool_materializer::TestToolOutputMaterializer)
}

#[async_trait]
impl Tool for TestTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.descriptor.description
    }

    fn schema(&self) -> serde_json::Value {
        self.descriptor.schema.clone()
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        self.risk
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        self.parallel_safe
    }

    fn descriptor(&self) -> ToolDescriptor {
        self.descriptor.clone()
    }

    async fn execute(&self, input: Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.executed_inputs.lock().unwrap().push(input.clone());

        let sleep_ms = input.get("sleep_ms").and_then(Value::as_u64).unwrap_or(0);
        if sleep_ms > 0 {
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        }

        let response = self.responses.lock().unwrap().pop_front();
        match response.unwrap_or(ScriptedResponse::Text("ok")) {
            ScriptedResponse::EchoField(field) => Ok(ToolOutput::Text {
                content: input
                    .get(field)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            ScriptedResponse::Text(text) => Ok(ToolOutput::Text {
                content: text.to_string(),
            }),
            ScriptedResponse::Timeout => Err(ToolError::Timeout {
                tool_name: self.name.clone(),
                timeout_secs: 1,
            }),
        }
    }
}

struct TestCatalog {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl TestCatalog {
    fn new(tools: Vec<Arc<dyn Tool>>) -> Self {
        let tools = tools
            .into_iter()
            .map(|tool| (tool.name().to_string(), tool))
            .collect();
        Self { tools }
    }
}

impl ToolCatalog for TestCatalog {
    fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools.values().map(|tool| tool.schema()).collect()
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
}

struct QueuePolicyEngine {
    decisions: Mutex<VecDeque<PolicyDecision>>,
    requests: Mutex<Vec<PolicyRequest>>,
}

impl QueuePolicyEngine {
    fn new(decisions: Vec<PolicyDecision>) -> Self {
        Self {
            decisions: Mutex::new(VecDeque::from(decisions)),
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl PolicyEngine for QueuePolicyEngine {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        self.requests.lock().unwrap().push(request);
        self.decisions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| PolicyDecision {
                status: PolicyStatus::Allowed,
                reason: None,
                policy_source: "test:default_allow".to_string(),
            })
    }
}

struct QueueApprovalProvider {
    decisions: Mutex<VecDeque<ApprovalDecision>>,
    requests: Mutex<Vec<ApprovalRequest>>,
}

impl QueueApprovalProvider {
    fn new(decisions: Vec<ApprovalDecision>) -> Self {
        Self {
            decisions: Mutex::new(VecDeque::from(decisions)),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait]
impl ApprovalProvider for QueueApprovalProvider {
    async fn approve(&self, request: ApprovalRequest) -> Result<ApprovalDecision, HarnessError> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .decisions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ApprovalDecision::Approve))
    }
}

struct FailingFlushTraceSink;

impl TraceSink for FailingFlushTraceSink {
    fn emit(&self, _event: AgentEvent) -> Result<(), TraceError> {
        Ok(())
    }

    fn flush(&self) -> Result<(), TraceError> {
        Err(TraceError::ReadFailed {
            reason: "simulated flush failure".to_string(),
        })
    }
}

fn make_session(mode: ExecutionMode) -> Session {
    Session::new(
        "session-1",
        SessionConfig {
            model: "test-model".to_string(),
            provider: "test-provider".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 4,
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
            working_dir: PathBuf::from("/workspace"),
            workspace_root: Some(PathBuf::from("/workspace")),
            timeout: Duration::from_secs(1),
            allow_network: true,
            environment: HashMap::new(),
            max_output_bytes: 4096,
            artifact_dir: None,
            current_tool_call_id: None,
            ignore_patterns: Vec::new(),
        },
        mode,
        WorkspaceSnapshot {
            workspace_root: PathBuf::from("/workspace"),
            git_sha: None,
            git_dirty: Some(false),
            untracked_count: None,
            content_hash: "snapshot-hash".to_string(),
            captured_at: chrono::Utc::now(),
        },
    )
}

fn mapping_for(tool_name: &str) -> ToolNameMapping {
    ToolNameMapping::new(
        CanonicalToolId {
            namespace: ToolNamespace::BuiltIn,
            name: tool_name.to_string(),
        },
        tool_name.to_string(),
        format!("desc-hash-{tool_name}"),
    )
}

fn allow(source: &str) -> PolicyDecision {
    PolicyDecision {
        status: PolicyStatus::Allowed,
        reason: Some("allowed".to_string()),
        policy_source: source.to_string(),
    }
}

fn confirm(source: &str) -> PolicyDecision {
    PolicyDecision {
        status: PolicyStatus::Confirm,
        reason: Some("confirm".to_string()),
        policy_source: source.to_string(),
    }
}

fn deny(source: &str) -> PolicyDecision {
    PolicyDecision {
        status: PolicyStatus::Denied,
        reason: Some("denied".to_string()),
        policy_source: source.to_string(),
    }
}

async fn execute_batch(
    executor: &ToolExecutor,
    session: &Session,
    tool_calls: Vec<ProposedToolCall>,
    session_grants: &mut Vec<SessionGrant>,
    current_turn: usize,
    sink: Option<&dyn TraceSink>,
) -> (
    Vec<(usize, String, ToolExecutionResult, u64, String)>,
    Vec<AgentEvent>,
) {
    let mut events = Vec::new();
    let result = executor
        .execute_tool_batch(
            session,
            tool_calls,
            &[mapping_for("dummy")],
            &mut |event| {
                events.push(event);
                Ok(())
            },
            session_grants,
            current_turn,
            4,
            &HookRegistry::default(),
            &CancelToken::new(),
            sink,
        )
        .await
        .expect("batch should succeed");

    (result, events)
}

fn trusted_annotations() -> ToolAnnotations {
    ToolAnnotations::new(vec![
        ToolAnnotation {
            key: "read_only".to_string(),
            value: "true".to_string(),
            source: AnnotationSource::BuiltInTrusted,
        },
        ToolAnnotation {
            key: "idempotent".to_string(),
            value: "true".to_string(),
            source: AnnotationSource::BuiltInTrusted,
        },
    ])
}

#[tokio::test]
async fn denied_policy_returns_failure_without_executing_tool() {
    let tool = Arc::new(TestTool::new(
        "dummy",
        RiskLevel::Low,
        false,
        None,
        ToolAnnotations::default(),
        vec![ScriptedResponse::Text("unexpected")],
    ));
    let executor = ToolExecutor::new(
        Arc::new(TestCatalog::new(vec![tool.clone()])),
        Arc::new(QueuePolicyEngine::new(vec![deny("test:deny")])),
        Arc::new(QueueApprovalProvider::new(vec![])),
        materializer(),
    );

    let (results, events) = execute_batch(
        &executor,
        &make_session(ExecutionMode::Confirm),
        vec![ProposedToolCall {
            id: "call-1".to_string(),
            name: "dummy".to_string(),
            input: json!({"value": "ignored"}),
        }],
        &mut Vec::new(),
        0,
        None,
    )
    .await;

    assert_eq!(tool.executions().len(), 0);
    assert_eq!(results.len(), 1);
    assert!(results[0].2.is_error);
    assert_eq!(
        results[0].2.failure.as_ref().unwrap().kind,
        ToolFailureKind::PolicyDenied
    );
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::PolicyDecision {
            decision: PolicyStatus::Denied,
            ..
        }
    )));
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolExecutionStarted { .. })));
}

#[tokio::test]
async fn session_grant_applies_only_to_same_input_hash() {
    let tool = Arc::new(TestTool::new(
        "dummy",
        RiskLevel::High,
        false,
        None,
        ToolAnnotations::default(),
        vec![
            ScriptedResponse::EchoField("value"),
            ScriptedResponse::EchoField("value"),
        ],
    ));
    let approvals = Arc::new(QueueApprovalProvider::new(vec![
        ApprovalDecision::AlwaysAllowForSession,
        ApprovalDecision::Deny,
    ]));
    let executor = ToolExecutor::new(
        Arc::new(TestCatalog::new(vec![tool.clone()])),
        Arc::new(QueuePolicyEngine::new(vec![
            confirm("test:confirm-1"),
            confirm("test:confirm-2"),
            confirm("test:confirm-3"),
        ])),
        approvals.clone(),
        materializer(),
    );
    let session = make_session(ExecutionMode::Confirm);
    let mut grants = Vec::new();

    let (first_results, first_events) = execute_batch(
        &executor,
        &session,
        vec![ProposedToolCall {
            id: "call-1".to_string(),
            name: "dummy".to_string(),
            input: json!({"value": "same"}),
        }],
        &mut grants,
        0,
        None,
    )
    .await;
    let (second_results, second_events) = execute_batch(
        &executor,
        &session,
        vec![ProposedToolCall {
            id: "call-2".to_string(),
            name: "dummy".to_string(),
            input: json!({"value": "same"}),
        }],
        &mut grants,
        1,
        None,
    )
    .await;
    let (third_results, third_events) = execute_batch(
        &executor,
        &session,
        vec![ProposedToolCall {
            id: "call-3".to_string(),
            name: "dummy".to_string(),
            input: json!({"value": "different"}),
        }],
        &mut grants,
        2,
        None,
    )
    .await;

    assert_eq!(first_results[0].2.content, "same");
    assert_eq!(second_results[0].2.content, "same");
    assert!(third_results[0].2.is_error);
    assert_eq!(approvals.request_count(), 2);
    assert!(first_events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalDecision {
            decision: ApprovalOutcome::AlwaysAllow,
            ..
        }
    )));
    assert!(!second_events
        .iter()
        .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })));
    assert!(third_events
        .iter()
        .any(|event| matches!(event, AgentEvent::ApprovalRequested { .. })));
}

#[tokio::test]
async fn edited_approval_re_evaluates_and_executes_edited_input() {
    let tool = Arc::new(TestTool::new(
        "dummy",
        RiskLevel::High,
        false,
        None,
        ToolAnnotations::default(),
        vec![ScriptedResponse::EchoField("value")],
    ));
    let executor = ToolExecutor::new(
        Arc::new(TestCatalog::new(vec![tool.clone()])),
        Arc::new(QueuePolicyEngine::new(vec![
            confirm("test:confirm"),
            allow("test:allow-after-edit"),
        ])),
        Arc::new(QueueApprovalProvider::new(vec![ApprovalDecision::Edit(
            json!({"value": "edited"}),
        )])),
        materializer(),
    );

    let (results, events) = execute_batch(
        &executor,
        &make_session(ExecutionMode::Confirm),
        vec![ProposedToolCall {
            id: "call-1".to_string(),
            name: "dummy".to_string(),
            input: json!({"value": "original"}),
        }],
        &mut Vec::new(),
        0,
        None,
    )
    .await;

    assert_eq!(results[0].2.content, "edited");
    assert_eq!(tool.executions(), vec![json!({"value": "edited"})]);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ApprovalDecision {
            decision: ApprovalOutcome::Edit,
            edited_input_hash: Some(_),
            ..
        }
    )));
}

#[tokio::test]
async fn edited_input_that_still_requires_confirmation_is_denied() {
    let tool = Arc::new(TestTool::new(
        "dummy",
        RiskLevel::High,
        false,
        None,
        ToolAnnotations::default(),
        vec![],
    ));
    let executor = ToolExecutor::new(
        Arc::new(TestCatalog::new(vec![tool.clone()])),
        Arc::new(QueuePolicyEngine::new(vec![
            confirm("test:confirm"),
            confirm("test:confirm-after-edit"),
        ])),
        Arc::new(QueueApprovalProvider::new(vec![ApprovalDecision::Edit(
            json!({"value": "still-risky"}),
        )])),
        materializer(),
    );

    let (results, events) = execute_batch(
        &executor,
        &make_session(ExecutionMode::Confirm),
        vec![ProposedToolCall {
            id: "call-1".to_string(),
            name: "dummy".to_string(),
            input: json!({"value": "original"}),
        }],
        &mut Vec::new(),
        0,
        None,
    )
    .await;

    assert!(results[0].2.is_error);
    assert_eq!(
        results[0].2.failure.as_ref().unwrap().kind,
        ToolFailureKind::ApprovalDenied
    );
    assert!(tool.executions().is_empty());
    assert!(!events
        .iter()
        .any(|event| matches!(event, AgentEvent::ToolExecutionStarted { .. })));
}

#[tokio::test]
async fn retryable_timeout_retries_for_trusted_read_only_tool() {
    let tool = Arc::new(TestTool::new(
        "dummy",
        RiskLevel::Low,
        false,
        Some(ToolRetryPolicy {
            max_retries: 1,
            backoff_ms: 0,
        }),
        trusted_annotations(),
        vec![
            ScriptedResponse::Timeout,
            ScriptedResponse::Text("ok after retry"),
        ],
    ));
    let executor = ToolExecutor::new(
        Arc::new(TestCatalog::new(vec![tool.clone()])),
        Arc::new(QueuePolicyEngine::new(vec![allow("test:allow")])),
        Arc::new(QueueApprovalProvider::new(vec![])),
        materializer(),
    );

    let (results, events) = execute_batch(
        &executor,
        &make_session(ExecutionMode::Confirm),
        vec![ProposedToolCall {
            id: "call-1".to_string(),
            name: "dummy".to_string(),
            input: json!({"value": "retry"}),
        }],
        &mut Vec::new(),
        0,
        None,
    )
    .await;

    assert_eq!(results[0].2.content, "ok after retry");
    assert_eq!(tool.executions().len(), 2);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolRetryAttempt {
            tool_call_id,
            attempt: 1,
            delay_ms: 0,
            ..
        } if tool_call_id == "call-1"
    )));
}

#[tokio::test]
async fn parallel_results_preserve_original_order_and_grouping() {
    let tool = Arc::new(TestTool::new(
        "dummy",
        RiskLevel::Low,
        true,
        None,
        ToolAnnotations::default(),
        vec![
            ScriptedResponse::EchoField("value"),
            ScriptedResponse::EchoField("value"),
        ],
    ));
    let executor = ToolExecutor::new(
        Arc::new(TestCatalog::new(vec![tool])),
        Arc::new(QueuePolicyEngine::new(vec![
            allow("test:allow-1"),
            allow("test:allow-2"),
        ])),
        Arc::new(QueueApprovalProvider::new(vec![])),
        materializer(),
    );

    let (results, events) = execute_batch(
        &executor,
        &make_session(ExecutionMode::Confirm),
        vec![
            ProposedToolCall {
                id: "call-1".to_string(),
                name: "dummy".to_string(),
                input: json!({"value": "first", "sleep_ms": 20}),
            },
            ProposedToolCall {
                id: "call-2".to_string(),
                name: "dummy".to_string(),
                input: json!({"value": "second", "sleep_ms": 0}),
            },
        ],
        &mut Vec::new(),
        0,
        None,
    )
    .await;

    let started: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionStarted {
                id,
                parallel_group_id,
                parallel_safe,
                ..
            } => Some((id.clone(), parallel_group_id.clone(), *parallel_safe)),
            _ => None,
        })
        .collect();

    assert_eq!(results[0].1, "call-1");
    assert_eq!(results[0].2.content, "first");
    assert_eq!(results[1].1, "call-2");
    assert_eq!(results[1].2.content, "second");
    assert_eq!(started.len(), 2);
    assert!(started.iter().all(|(_, group, parallel_safe)| {
        *parallel_safe && group.as_deref() == Some("group-2")
    }));
}

#[tokio::test]
async fn trace_flush_failure_is_non_fatal_and_emits_warning() {
    let tool = Arc::new(TestTool::new(
        "dummy",
        RiskLevel::Low,
        false,
        None,
        ToolAnnotations::default(),
        vec![ScriptedResponse::Text("ok")],
    ));
    let executor = ToolExecutor::new(
        Arc::new(TestCatalog::new(vec![tool])),
        Arc::new(QueuePolicyEngine::new(vec![allow("test:allow")])),
        Arc::new(QueueApprovalProvider::new(vec![])),
        materializer(),
    );

    let (results, events) = execute_batch(
        &executor,
        &make_session(ExecutionMode::Confirm),
        vec![ProposedToolCall {
            id: "call-1".to_string(),
            name: "dummy".to_string(),
            input: json!({"value": "ok"}),
        }],
        &mut Vec::new(),
        0,
        Some(&FailingFlushTraceSink),
    )
    .await;

    assert_eq!(results[0].2.content, "ok");
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::Error { message, recoverable: true }
            if message.contains("trace flush failed")
    )));
}
