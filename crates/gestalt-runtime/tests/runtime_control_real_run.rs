use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
use sha2::Digest;

#[derive(Clone, Copy)]
enum ProviderBehavior {
    EndTurn,
    ToolThenEnd,
    ToolTwiceSameThenEnd,
    ToolTwiceDifferentThenEnd,
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
            behavior
                if (matches!(behavior, ProviderBehavior::ToolThenEnd) && call == 0)
                    || (matches!(
                        behavior,
                        ProviderBehavior::ToolTwiceSameThenEnd
                            | ProviderBehavior::ToolTwiceDifferentThenEnd
                    ) && call < 2) =>
            {
                let value = if matches!(self.behavior, ProviderBehavior::ToolTwiceDifferentThenEnd)
                    && call == 1
                {
                    2
                } else {
                    1
                };
                Ok(Box::pin(futures::stream::iter(vec![
                    Ok(AgentEvent::ToolCallStreamed {
                        id: format!("call-{}", call + 1),
                        name: "record".to_string(),
                        input_delta: format!(r#"{{"value":{value}}}"#),
                    }),
                    Ok(AgentEvent::Stop {
                        reason: StopReason::ToolUse,
                    }),
                ])))
            }
            ProviderBehavior::ToolThenEnd
            | ProviderBehavior::ToolTwiceSameThenEnd
            | ProviderBehavior::ToolTwiceDifferentThenEnd => Ok(Box::pin(futures::stream::iter(
                vec![Ok(AgentEvent::Stop {
                    reason: StopReason::EndTurn,
                })],
            ))),
            ProviderBehavior::Pending => Ok(Box::pin(futures::stream::pending())),
        }
    }
}

struct RecordingTool {
    executions: Arc<AtomicUsize>,
    inputs: Arc<Mutex<Vec<serde_json::Value>>>,
}

struct ArtifactTool {
    path: std::path::PathBuf,
}

#[async_trait::async_trait]
impl Tool for ArtifactTool {
    fn name(&self) -> &str {
        "record"
    }

    fn description(&self) -> &str {
        "materialize artifact"
    }

    fn schema(&self) -> ToolSchema {
        serde_json::from_value(serde_json::json!({
            "name": "record",
            "description": "materialize artifact",
            "input_schema": {
                "type": "object",
                "properties": {"value": {"type": "integer"}},
                "required": ["value"]
            }
        }))
        .unwrap()
    }

    fn risk(&self, _input: &serde_json::Value) -> gestalt_core::RiskLevel {
        gestalt_core::RiskLevel::Low
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::ToolError> {
        Ok(ToolOutput::Artifact {
            path: self.path.clone(),
            mime_type: "text/plain".to_string(),
            size_bytes: std::fs::metadata(&self.path)
                .unwrap()
                .len()
                .try_into()
                .unwrap(),
        })
    }
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
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::ToolError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        self.inputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(input);
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
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        if self.confirm
            && request
                .input
                .get("edited")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        {
            return PolicyDecision::allowed(Some(
                "edited input accepted by test policy".to_string(),
            ));
        }
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
            gestalt_runtime::unstable::TraceEventV1::UserMessage { .. }
        )
    }));
}

#[tokio::test]
async fn approval_requested_for_confirm_policy() {
    let executions = Arc::new(AtomicUsize::new(0));
    let host = host(
        ProviderBehavior::ToolThenEnd,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(SingleToolCatalog {
            tool: Arc::new(RecordingTool {
                executions: executions.clone(),
                inputs: Arc::new(Mutex::new(Vec::new())),
            }),
        }),
        true,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-approval").await;
    continue_run(&host, &session_id, &run_id, None).await;

    let approval = wait_for_approval(&host, &session_id).await;
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        approval
            .session_grant_terms
            .as_ref()
            .map(|terms| terms.risk_ceiling),
        Some(gestalt_runtime::api::v1::RiskLevelV1::Medium)
    );
}

#[tokio::test]
async fn approval_approve_executes_tool() {
    let executions = Arc::new(AtomicUsize::new(0));
    let host = host(
        ProviderBehavior::ToolThenEnd,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(SingleToolCatalog {
            tool: Arc::new(RecordingTool {
                executions: executions.clone(),
                inputs: Arc::new(Mutex::new(Vec::new())),
            }),
        }),
        true,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-approval-approve").await;
    continue_run(&host, &session_id, &run_id, None).await;
    let approval = wait_for_approval(&host, &session_id).await;

    host.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: approval.approval_id,
        decision: ApprovalDecisionV1::Approve,
    })
    .await
    .unwrap();
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

async fn wait_for_approval(
    host: &RuntimeBackedControlHost,
    session_id: &SessionIdV1,
) -> gestalt_runtime::api::v1::ApprovalProjectionV1 {
    tokio::time::timeout(Duration::from_secs(2), async {
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
    .unwrap()
}

#[tokio::test]
async fn approval_deny_blocks_tool() {
    let executions = Arc::new(AtomicUsize::new(0));
    let host = host(
        ProviderBehavior::ToolThenEnd,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(SingleToolCatalog {
            tool: Arc::new(RecordingTool {
                executions: executions.clone(),
                inputs: Arc::new(Mutex::new(Vec::new())),
            }),
        }),
        true,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-approval-deny").await;
    continue_run(&host, &session_id, &run_id, None).await;
    let approval = wait_for_approval(&host, &session_id).await;

    host.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: approval.approval_id,
        decision: ApprovalDecisionV1::Deny,
    })
    .await
    .unwrap();
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn approval_edit_revalidates_policy() {
    let executions = Arc::new(AtomicUsize::new(0));
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let host = host(
        ProviderBehavior::ToolThenEnd,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(SingleToolCatalog {
            tool: Arc::new(RecordingTool {
                executions: executions.clone(),
                inputs: inputs.clone(),
            }),
        }),
        true,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-approval-edit").await;
    continue_run(&host, &session_id, &run_id, None).await;
    let approval = wait_for_approval(&host, &session_id).await;

    host.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: approval.approval_id,
        decision: ApprovalDecisionV1::Edit(serde_json::json!({"value": 2, "edited": true})),
    })
    .await
    .unwrap();
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(
        *inputs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![serde_json::json!({"value": 2, "edited": true})]
    );
}

#[tokio::test]
async fn runtime_session_grant_is_limited_by_input_hash() {
    let executions = Arc::new(AtomicUsize::new(0));
    let host = host(
        ProviderBehavior::ToolTwiceDifferentThenEnd,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(SingleToolCatalog {
            tool: Arc::new(RecordingTool {
                executions: executions.clone(),
                inputs: Arc::new(Mutex::new(Vec::new())),
            }),
        }),
        true,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-approval-grant").await;
    continue_run(&host, &session_id, &run_id, None).await;
    let first = wait_for_approval(&host, &session_id).await;
    assert_eq!(
        first
            .session_grant_terms
            .as_ref()
            .map(|terms| terms.input_hash.as_str()),
        Some(first.original_hash.as_str())
    );
    host.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: first.approval_id,
        decision: ApprovalDecisionV1::AlwaysAllowForSession,
    })
    .await
    .unwrap();

    let second = wait_for_approval(&host, &session_id).await;
    host.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: second.approval_id,
        decision: ApprovalDecisionV1::Deny,
    })
    .await
    .unwrap();
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runtime_session_grant_reuses_only_its_exact_bound() {
    let executions = Arc::new(AtomicUsize::new(0));
    let host = host(
        ProviderBehavior::ToolTwiceSameThenEnd,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(SingleToolCatalog {
            tool: Arc::new(RecordingTool {
                executions: executions.clone(),
                inputs: Arc::new(Mutex::new(Vec::new())),
            }),
        }),
        true,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let (session_id, run_id) = start(&host, "real-approval-grant-reuse").await;
    continue_run(&host, &session_id, &run_id, None).await;
    let approval = wait_for_approval(&host, &session_id).await;
    host.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: approval.approval_id,
        decision: ApprovalDecisionV1::AlwaysAllowForSession,
    })
    .await
    .unwrap();
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    assert_eq!(executions.load(Ordering::SeqCst), 2);
    assert!(host
        .list_pending_approvals(ListPendingApprovalsRequestV1 { session_id })
        .await
        .unwrap()
        .approvals
        .is_empty());
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
async fn artifact_real_runtime_tool_output_materialized() {
    let source = tempfile::tempdir().unwrap();
    let source_path = source.path().join("tool-output.txt");
    std::fs::write(&source_path, b"tool artifact").unwrap();
    let store = Arc::new(InMemoryArtifactStore::new());
    let host = host(
        ProviderBehavior::ToolThenEnd,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(SingleToolCatalog {
            tool: Arc::new(ArtifactTool { path: source_path }),
        }),
        false,
        store.clone(),
        None,
    );
    let (session_id, run_id) = start(&host, "real-tool-artifact").await;
    continue_run(&host, &session_id, &run_id, None).await;
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    let listed = host
        .list_artifacts(ListArtifactsRequestV1 {
            session_id: session_id.clone(),
            cursor: None,
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(listed.artifacts.len(), 1);
    let metadata = &listed.artifacts[0];
    assert!(!metadata
        .display_path
        .contains(source.path().to_str().unwrap()));
    assert_eq!(
        store
            .get_artifact(&session_id.0, &metadata.logical_id.0)
            .unwrap(),
        b"tool artifact"
    );
    assert_eq!(
        metadata.integrity,
        format!("{:x}", sha2::Sha256::digest(b"tool artifact"))
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

#[cfg(feature = "trace")]
#[tokio::test]
async fn runtime_control_submit_before_continue_is_seen_by_real_runtime() {
    let trace_root = tempfile::tempdir().unwrap();
    let host = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        Some(trace_root.path().to_path_buf()),
    );
    let (session_id, run_id) = start(&host, "submit-before").await;

    let submit_resp = host
        .submit_message(gestalt_runtime::api::v1::SubmitMessageRequestV1 {
            session_id: session_id.clone(),
            message: "steered-msg".to_string(),
            idempotency_key: None,
        })
        .await
        .unwrap();
    assert!(submit_resp.acknowledged);

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
        match &event.event {
            gestalt_runtime::unstable::TraceEventV1::SessionMessageInjected { message } => {
                message.content.contains("steered-msg")
            }
            _ => false,
        }
    }));
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn runtime_control_submit_between_completed_and_resumed_run_is_seen_by_real_runtime() {
    let trace_root = tempfile::tempdir().unwrap();
    let host = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        Some(trace_root.path().to_path_buf()),
    );
    let (session_id, run_id) = start(&host, "submit-between").await;
    continue_run(&host, &session_id, &run_id, None).await;
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    host.resume_session(gestalt_runtime::api::v1::ResumeSessionRequestV1 {
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        idempotency_key: None,
    })
    .await
    .unwrap();

    let run_id2 = host
        .inspect_session(gestalt_runtime::api::v1::InspectSessionRequestV1 {
            session_id: session_id.clone(),
        })
        .await
        .unwrap()
        .active_run_id
        .unwrap();

    let submit_resp = host
        .submit_message(gestalt_runtime::api::v1::SubmitMessageRequestV1 {
            session_id: session_id.clone(),
            message: "msg-between".to_string(),
            idempotency_key: None,
        })
        .await
        .unwrap();
    assert!(submit_resp.acknowledged);

    continue_run(&host, &session_id, &run_id2, None).await;
    wait_for_status(&host, &session_id, &run_id2, RunStatusV1::Completed).await;

    let mut found = false;
    let paths = std::fs::read_dir(trace_root.path()).unwrap();
    for entry in paths.flatten() {
        let trace_path = entry.path().join("trace.jsonl");
        if trace_path.exists() {
            let events = gestalt_runtime::unstable::read_trace(trace_path).unwrap();
            if events.iter().any(|event| match &event.event {
                gestalt_runtime::unstable::TraceEventV1::SessionMessageInjected { message } => {
                    message.content.contains("msg-between")
                }
                _ => false,
            }) {
                found = true;
                break;
            }
        }
    }
    assert!(
        found,
        "Message between runs was not seen by the real runtime!"
    );
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn runtime_control_submit_during_execution_is_seen_by_real_runtime() {
    let trace_root = tempfile::tempdir().unwrap();
    let host = host(
        ProviderBehavior::Pending,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        Some(trace_root.path().to_path_buf()),
    );
    let (session_id, run_id) = start(&host, "submit-during").await;
    host.continue_session(ContinueSessionRequestV1 {
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        message: "run".to_string(),
        idempotency_key: None,
    })
    .await
    .unwrap();

    let submit_resp = host
        .submit_message(gestalt_runtime::api::v1::SubmitMessageRequestV1 {
            session_id: session_id.clone(),
            message: "msg-during".to_string(),
            idempotency_key: None,
        })
        .await
        .unwrap();
    assert!(submit_resp.acknowledged);

    host.cancel_run(gestalt_runtime::api::v1::CancelRunRequestV1 {
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        correlation_id: None,
    })
    .await
    .unwrap();
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Cancelled).await;
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn runtime_control_resume_completed_run_rebinds_real_runtime_session() {
    let trace_root = tempfile::tempdir().unwrap();
    let host1 = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        Some(trace_root.path().to_path_buf()),
    );
    let (session_id, run_id) = start(&host1, "resume-rebind").await;
    continue_run(&host1, &session_id, &run_id, None).await;
    wait_for_status(&host1, &session_id, &run_id, RunStatusV1::Completed).await;

    let host2 = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        Some(trace_root.path().to_path_buf()),
    );

    let resume_resp = host2
        .resume_session(gestalt_runtime::api::v1::ResumeSessionRequestV1 {
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            idempotency_key: None,
        })
        .await
        .unwrap();
    assert_eq!(resume_resp.session_id, session_id);
    assert_eq!(resume_resp.run_id, run_id);

    let inspect = host2
        .inspect_session(gestalt_runtime::api::v1::InspectSessionRequestV1 {
            session_id: session_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(inspect.active_run_id, Some(run_id));
}

#[tokio::test]
async fn runtime_control_resume_missing_runtime_state_returns_stable_error() {
    let host = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let err = host
        .resume_session(gestalt_runtime::api::v1::ResumeSessionRequestV1 {
            session_id: SessionIdV1("nonexistent".to_string()),
            run_id: gestalt_runtime::api::v1::RunIdV1("nonexistent-run".to_string()),
            idempotency_key: None,
        })
        .await
        .unwrap_err();

    assert_eq!(err.code, ControlErrorCodeV1::NotFound);
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn runtime_control_resume_preserves_trace_artifact_policy_context_expectations() {
    let trace_root = tempfile::tempdir().unwrap();
    let store = Arc::new(InMemoryArtifactStore::new());
    let host1 = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        store.clone(),
        Some(trace_root.path().to_path_buf()),
    );
    let (session_id, run_id) = start(&host1, "resume-preserves").await;

    host1
        .create_artifact(CreateArtifactRequestV1 {
            session_id: session_id.clone(),
            display_path: "resume-file.txt".to_string(),
            data: b"some data".to_vec(),
        })
        .await
        .unwrap();

    continue_run(&host1, &session_id, &run_id, None).await;
    wait_for_status(&host1, &session_id, &run_id, RunStatusV1::Completed).await;

    let host2 = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        store.clone(),
        Some(trace_root.path().to_path_buf()),
    );
    host2
        .resume_session(gestalt_runtime::api::v1::ResumeSessionRequestV1 {
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            idempotency_key: None,
        })
        .await
        .unwrap();

    let artifacts = host2
        .list_artifacts(ListArtifactsRequestV1 {
            session_id: session_id.clone(),
            cursor: None,
            limit: None,
        })
        .await
        .unwrap()
        .artifacts;
    assert!(artifacts
        .iter()
        .any(|a| a.display_path == "resume-file.txt"));
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn runtime_control_branch_uses_requested_parent_run_boundary() {
    let trace_root = tempfile::tempdir().unwrap();
    let host = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        Some(trace_root.path().to_path_buf()),
    );
    let (session_id, run_id) = start(&host, "branch-boundary").await;

    host.submit_message(gestalt_runtime::api::v1::SubmitMessageRequestV1 {
        session_id: session_id.clone(),
        message: "msg1".to_string(),
        idempotency_key: None,
    })
    .await
    .unwrap();
    continue_run(&host, &session_id, &run_id, None).await;
    wait_for_status(&host, &session_id, &run_id, RunStatusV1::Completed).await;

    let branch_resp = host
        .branch_session(gestalt_runtime::api::v1::BranchSessionRequestV1 {
            parent_session_id: session_id.clone(),
            parent_run_id: run_id.clone(),
            new_session_id: Some(SessionIdV1("branched-session".to_string())),
            idempotency_key: None,
        })
        .await
        .unwrap();

    let inspect = host
        .inspect_session(gestalt_runtime::api::v1::InspectSessionRequestV1 {
            session_id: branch_resp.new_session_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(inspect.active_run_id, Some(branch_resp.new_run_id));
}

#[cfg(feature = "trace")]
#[tokio::test]
async fn runtime_control_branch_does_not_include_messages_after_branch_point() {
    let trace_root = tempfile::tempdir().unwrap();
    let host = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        Some(trace_root.path().to_path_buf()),
    );
    let (session_id, run_id1) = start(&host, "branch-messages").await;

    host.submit_message(gestalt_runtime::api::v1::SubmitMessageRequestV1 {
        session_id: session_id.clone(),
        message: "msg1".to_string(),
        idempotency_key: None,
    })
    .await
    .unwrap();
    continue_run(&host, &session_id, &run_id1, None).await;
    wait_for_status(&host, &session_id, &run_id1, RunStatusV1::Completed).await;

    host.resume_session(gestalt_runtime::api::v1::ResumeSessionRequestV1 {
        session_id: session_id.clone(),
        run_id: run_id1.clone(),
        idempotency_key: None,
    })
    .await
    .unwrap();
    let run_id2 = host
        .inspect_session(gestalt_runtime::api::v1::InspectSessionRequestV1 {
            session_id: session_id.clone(),
        })
        .await
        .unwrap()
        .active_run_id
        .unwrap();
    host.submit_message(gestalt_runtime::api::v1::SubmitMessageRequestV1 {
        session_id: session_id.clone(),
        message: "msg2".to_string(),
        idempotency_key: None,
    })
    .await
    .unwrap();
    continue_run(&host, &session_id, &run_id2, None).await;
    wait_for_status(&host, &session_id, &run_id2, RunStatusV1::Completed).await;

    let branch_resp = host
        .branch_session(gestalt_runtime::api::v1::BranchSessionRequestV1 {
            parent_session_id: session_id.clone(),
            parent_run_id: run_id1.clone(),
            new_session_id: Some(SessionIdV1("branched-msg1-only".to_string())),
            idempotency_key: None,
        })
        .await
        .unwrap();

    continue_run(
        &host,
        &branch_resp.new_session_id,
        &branch_resp.new_run_id,
        None,
    )
    .await;
    wait_for_status(
        &host,
        &branch_resp.new_session_id,
        &branch_resp.new_run_id,
        RunStatusV1::Completed,
    )
    .await;

    let history = host
        .get_session_history(&branch_resp.new_session_id)
        .unwrap();
    let branched_has_msg1 = history.iter().any(|msg| match msg {
        gestalt_core::message::Message::User { content, .. } => {
            content.iter().any(|block| match block {
                gestalt_core::message::ContentBlock::Text { text } => text.contains("msg1"),
                _ => false,
            })
        }
        _ => false,
    });
    let branched_has_msg2 = history.iter().any(|msg| match msg {
        gestalt_core::message::Message::User { content, .. } => {
            content.iter().any(|block| match block {
                gestalt_core::message::ContentBlock::Text { text } => text.contains("msg2"),
                _ => false,
            })
        }
        _ => false,
    });
    assert!(
        branched_has_msg1,
        "Branched run should inherit msg1 from parent!"
    );
    assert!(
        !branched_has_msg2,
        "Branched run should NOT inherit msg2 from after the branch point!"
    );
}

#[tokio::test]
async fn runtime_control_branch_missing_checkpoint_returns_stable_error() {
    let host = host(
        ProviderBehavior::EndTurn,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(EmptyToolCatalog),
        false,
        Arc::new(InMemoryArtifactStore::new()),
        None,
    );
    let err = host
        .branch_session(gestalt_runtime::api::v1::BranchSessionRequestV1 {
            parent_session_id: SessionIdV1("nonexistent".to_string()),
            parent_run_id: gestalt_runtime::api::v1::RunIdV1("nonexistent-run".to_string()),
            new_session_id: Some(SessionIdV1("branched-session".to_string())),
            idempotency_key: None,
        })
        .await
        .unwrap_err();

    assert_eq!(err.code, ControlErrorCodeV1::NotFound);
}
