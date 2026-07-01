use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gestalt_core::{
    approval::AutoApprovalProvider,
    event::{AgentEvent, StopReason},
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{Tool, ToolCatalog, ToolContext, ToolOutput, ToolSchema},
};
use gestalt_runtime::api::v1::RuntimeBackedControlHost;
use gestalt_runtime::api::v1::{
    ApprovalControlV1, ApprovalDecisionV1, ArtifactAccessV1, ContinueSessionRequestV1,
    ControlErrorCodeV1, CreateArtifactRequestV1, EventPayloadV1, EventSourceV1, IdempotencyKeyV1,
    InspectRunRequestV1, ListArtifactsRequestV1, ListPendingApprovalsRequestV1,
    PollEventsRequestV1, ReadArtifactRangeRequestV1, RespondToApprovalRequestV1, RunQueryV1,
    RunStatusV1, SessionControlV1, SessionIdV1, StartSessionRequestV1,
};
use gestalt_runtime::unstable::{
    AgentRuntimeBuilder, ArtifactStore, ContextMessageAssembler, InMemoryArtifactStore,
    RuntimeConfig,
};

#[derive(Clone, Copy)]
enum ProviderBehavior {
    EndTurn,
    ToolThenEnd,
    Pending,
}

struct FakeProvider {
    behavior: ProviderBehavior,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Provider for FakeProvider {
    fn id(&self) -> &str {
        "fake"
    }

    fn display_name(&self) -> &str {
        "Fake"
    }

    fn default_model(&self) -> &str {
        "fake-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        static CAPABILITIES: ProviderCapabilities = ProviderCapabilities {
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
        &CAPABILITIES
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::HarnessError> {
        Ok(0)
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::HarnessError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        match self.behavior {
            ProviderBehavior::EndTurn => Ok(Box::pin(futures::stream::iter(vec![
                Ok(AgentEvent::Text {
                    delta: "done".to_string(),
                }),
                Ok(AgentEvent::Stop {
                    reason: StopReason::EndTurn,
                }),
            ]))),
            ProviderBehavior::ToolThenEnd if call == 0 => {
                Ok(Box::pin(futures::stream::iter(vec![
                    Ok(AgentEvent::ToolCallStreamed {
                        id: "call-1".to_string(),
                        name: "record".to_string(),
                        input_delta: r#"{"value":1}"#.to_string(),
                    }),
                    Ok(AgentEvent::Stop {
                        reason: StopReason::ToolUse,
                    }),
                ])))
            }
            ProviderBehavior::ToolThenEnd => Ok(Box::pin(futures::stream::iter(vec![Ok(
                AgentEvent::Stop {
                    reason: StopReason::EndTurn,
                },
            )]))),
            ProviderBehavior::Pending => Ok(Box::pin(futures::stream::pending())),
        }
    }
}

struct RecordingTool {
    executions: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for RecordingTool {
    fn name(&self) -> &str {
        "record"
    }

    fn description(&self) -> &str {
        "record execution"
    }

    fn schema(&self) -> ToolSchema {
        serde_json::from_value(serde_json::json!({
            "name": "record",
            "description": "record execution",
            "input_schema": {
                "type": "object",
                "properties": {"value": {"type": "integer"}},
                "required": ["value"]
            }
        }))
        .unwrap()
    }

    fn risk(&self, _input: &serde_json::Value) -> gestalt_core::RiskLevel {
        gestalt_core::RiskLevel::Medium
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::ToolError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::Text {
            content: "recorded".to_string(),
        })
    }
}

struct SingleToolCatalog {
    tool: Arc<dyn Tool>,
}

impl ToolCatalog for SingleToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        vec![self.tool.schema()]
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        (name == self.tool.name()).then(|| self.tool.clone())
    }
}

struct EmptyToolCatalog;

impl ToolCatalog for EmptyToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }

    fn get(&self, _name: &str) -> Option<Arc<dyn Tool>> {
        None
    }
}

struct TestPolicy {
    confirm: bool,
}

#[async_trait::async_trait]
impl PolicyEngine for TestPolicy {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        if self.confirm {
            PolicyDecision::confirm(
                "test approval required".to_string(),
                "test-policy".to_string(),
            )
        } else {
            PolicyDecision::allowed(None)
        }
    }
}

fn host(
    behavior: ProviderBehavior,
    calls: Arc<AtomicUsize>,
    tools: Arc<dyn ToolCatalog>,
    confirm: bool,
    store: Arc<InMemoryArtifactStore>,
    trace_directory: Option<std::path::PathBuf>,
) -> RuntimeBackedControlHost {
    let mut config = RuntimeConfig::default();
    config.max_turns = 3;
    config.context_management_policy = Some(gestalt_core::ContextManagementPolicy {
        enabled: false,
        ..Default::default()
    });
    RuntimeBackedControlHost::with_trace_directory(
        AgentRuntimeBuilder::new()
            .provider(Arc::new(FakeProvider { behavior, calls }))
            .tools(tools)
            .assembler(Arc::new(ContextMessageAssembler::new(
                "runtime-control-real",
            )))
            .policy(Arc::new(TestPolicy { confirm }))
            .approval(Arc::new(AutoApprovalProvider))
            .config(config),
        store,
        trace_directory,
    )
    .unwrap()
}

async fn start(
    host: &RuntimeBackedControlHost,
    id: &str,
) -> (SessionIdV1, gestalt_runtime::api::v1::RunIdV1) {
    let response = host
        .start_session(StartSessionRequestV1 {
            session_id: Some(SessionIdV1(id.to_string())),
            idempotency_key: None,
            config_override: None,
        })
        .await
        .unwrap();
    (response.session_id, response.run_id)
}

async fn wait_for_status(
    host: &RuntimeBackedControlHost,
    session_id: &SessionIdV1,
    run_id: &gestalt_runtime::api::v1::RunIdV1,
    expected: RunStatusV1,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = host
                .inspect_run(InspectRunRequestV1 {
                    session_id: session_id.clone(),
                    run_id: run_id.clone(),
                })
                .await
                .unwrap()
                .status;
            if status == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn continue_run(
    host: &RuntimeBackedControlHost,
    session_id: &SessionIdV1,
    run_id: &gestalt_runtime::api::v1::RunIdV1,
    key: Option<&str>,
) {
    let response = host
        .continue_session(ContinueSessionRequestV1 {
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            message: "run".to_string(),
            idempotency_key: key.map(|key| IdempotencyKeyV1(key.to_string())),
        })
        .await
        .unwrap();
    assert!(response.acknowledged);
}

#[tokio::test]
async fn runtime_control_start_and_complete_fake_provider_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let host = host(
        ProviderBehavior::EndTurn,
        calls.clone(),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-complete").await;

    continue_run(&host, &session_id, &run_id, None).await;
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runtime_control_streams_or_polls_real_events() {
    let host = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-events").await;
    continue_run(&host, &session_id, &run_id, None).await;
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    let events = host
        .poll_events(PollEventsRequestV1 {
            session_id,
            cursor: None,
            limit: None,
            kinds: None,
        })
        .await
        .unwrap()
        .events;

    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayloadV1::Unknown)));
    assert!(events
        .iter()
        .any(|event| matches!(event.payload, EventPayloadV1::RunCompleted)));
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn runtime_control_persists_real_trace() {
    let trace_root = tempfile::tempdir().unwrap();
    let host = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        Some(trace_root.path().to_path_buf()),
    );
    let (session_id, run_id) = start(&host, "real-trace").await;
    continue_run(&host, &session_id, &run_id, None).await;
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    let run_directory = std::fs::read_dir(trace_root.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let events = gestalt_runtime::unstable::read_trace(run_directory.join("trace.jsonl")).unwrap();

    assert!(events.iter().any(|event| {
        matches!(
            event.event,
            gestalt_runtime::unstable::TraceEvent::UserMessage { .. }
        )
    }));
}

#[tokio::test]
async fn runtime_control_policy_requires_approval_then_executes_tool() {
    let executions = Arc::new(AtomicUsize::new(0));
    let host = host(
        ProviderBehavior::ToolThenEnd,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(SingleToolCatalog {
            tool: Arc::new(RecordingTool {
                executions: executions.clone(),
            }),
        }),
        true,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-approval").await;
    continue_run(&host, &session_id, &run_id, None).await;

    let approval = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let mut approvals = host
                .list_pending_approvals(ListPendingApprovalsRequestV1 {
                    session_id: session_id.clone(),
                })
                .await
                .unwrap()
                .approvals;
            if let Some(approval) = approvals.pop() {
                break approval;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    host.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: approval.approval_id,
        decision: ApprovalDecisionV1::Approve,
    })
    .await
    .unwrap();
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runtime_control_cancel_active_run() {
    let calls = Arc::new(AtomicUsize::new(0));
    let host = host(
        ProviderBehavior::Pending,
        calls.clone(),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-cancel").await;
    continue_run(&host, &session_id, &run_id, None).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let cancelled = host
        .cancel_run(gestalt_runtime::api::v1::CancelRunRequestV1 {
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            correlation_id: None,
        })
        .await
        .unwrap();
    assert!(cancelled.cancelled);
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Cancelled).await;

    let terminal = host
        .cancel_run(gestalt_runtime::api::v1::CancelRunRequestV1 {
            session_id,
            run_id,
            correlation_id: None,
        })
        .await
        .unwrap();
    assert!(!terminal.cancelled);
}

#[tokio::test]
async fn runtime_control_artifact_lifecycle_uses_real_artifact_store() {
    let store = Arc::new(InMemoryArtifactStore::new());
    let host = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        store.clone(),
        None,
    );
    let (session_id, _) = start(&host, "real-artifact").await;

    let artifact = host
        .create_artifact(CreateArtifactRequestV1 {
            session_id: session_id.clone(),
            display_path: "result.txt".to_string(),
            data: b"artifact".to_vec(),
        })
        .await
        .unwrap();
    let listed = host
        .list_artifacts(ListArtifactsRequestV1 {
            session_id: session_id.clone(),
            cursor: None,
            limit: None,
        })
        .await
        .unwrap();
    let range = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id: session_id.clone(),
            artifact_id: artifact.metadata.logical_id,
            offset: 0,
            length: 8,
        })
        .await
        .unwrap();

    assert_eq!(listed.artifacts.len(), 1);
    assert_eq!(range.data, b"artifact");
    assert_eq!(
        store.get_artifact(&session_id.0, "result.txt").unwrap(),
        b"artifact"
    );
}

#[tokio::test]
async fn runtime_control_idempotency_does_not_duplicate_execution() {
    let calls = Arc::new(AtomicUsize::new(0));
    let host = host(
        ProviderBehavior::EndTurn,
        calls.clone(),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-idempotency").await;
    let request = ContinueSessionRequestV1 {
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        message: "run once".to_string(),
        idempotency_key: Some(IdempotencyKeyV1("continue-once".to_string())),
    };

    let first = host.continue_session(request.clone()).await.unwrap();
    let replay = host.continue_session(request).await.unwrap();
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    assert_eq!(first, replay);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runtime_control_concurrent_continue_rejects_or_queues_deterministically() {
    let host = host(
        ProviderBehavior::Pending,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-concurrent").await;
    let request = || ContinueSessionRequestV1 {
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        message: "run".to_string(),
        idempotency_key: None,
    };

    let (first, second) = tokio::join!(
        host.continue_session(request()),
        host.continue_session(request())
    );

    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    let conflict = first.err().or_else(|| second.err()).unwrap();
    assert_eq!(conflict.code, ControlErrorCodeV1::Conflict);
    host.cancel_run(gestalt_runtime::api::v1::CancelRunRequestV1 {
        session_id,
        run_id,
        correlation_id: None,
    })
    .await
    .unwrap();
}
