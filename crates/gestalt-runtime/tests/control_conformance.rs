//! H1A/H1B criterion-to-evidence matrix.
//!
//! - H1A-F01–F07: `dto_families_*` plus the compiling host implementations.
//! - H1A-B01–B07: `run_conformance`.
//! - H1B-F01–F03, H1B-B01: in-memory, mock, and runtime-backed hosts invoke
//!   the same generic suite.
//! - H1B-F02: `mock_host_exposes_controllable_failures`.
//! - H1B-F07, H1B-B05: `examples/embed_runtime.rs` is compiled by `--all-targets`.
//! - H1B-F04–F05, H1B-B02–B03: `gestalt-app/tests/report_contract_tests.rs`.
//! - H1B-F06: legacy broad traits are no longer crate-root exports.
//! - H1B-B04: this suite also passes with `--no-default-features`.

use gestalt_runtime::api::v1::*;
use sha2::Digest;
use std::sync::Arc;

use gestalt_core::{
    approval::AutoApprovalProvider,
    event::{AgentEvent, StopReason},
    message::Message,
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    tool::{ToolCatalog, ToolSchema},
};
use gestalt_runtime::unstable::{
    AgentRuntimeBuilder, ContextMessageAssembler, InMemoryArtifactStore, RuntimeConfig,
};

#[async_trait::async_trait]
trait ConformanceHost: RuntimeControlV1 + Clone {
    fn add_approval(&self, approval: ApprovalProjectionV1);
    fn add_policy_projection(&self, projection: PolicyProjectionV1);
    async fn complete_run(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
    ) -> std::result::Result<(), ControlErrorV1>;
}

#[async_trait::async_trait]
impl ConformanceHost for InMemoryControlHost {
    fn add_approval(&self, approval: ApprovalProjectionV1) {
        self.add_approval(approval);
    }

    fn add_policy_projection(&self, projection: PolicyProjectionV1) {
        self.add_policy_projection(projection);
    }

    async fn complete_run(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
    ) -> std::result::Result<(), ControlErrorV1> {
        self.complete_run(session_id, run_id).await
    }
}

#[async_trait::async_trait]
impl ConformanceHost for RuntimeBackedControlHost {
    fn add_approval(&self, approval: ApprovalProjectionV1) {
        self.add_approval(approval);
    }

    fn add_policy_projection(&self, projection: PolicyProjectionV1) {
        self.add_policy_projection(projection);
    }

    async fn complete_run(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
    ) -> std::result::Result<(), ControlErrorV1> {
        self.complete_run(session_id, run_id).await
    }
}

struct ConformanceProvider;

#[async_trait::async_trait]
impl Provider for ConformanceProvider {
    fn id(&self) -> &str {
        "conformance"
    }

    fn display_name(&self) -> &str {
        "Conformance"
    }

    fn default_model(&self) -> &str {
        "conformance-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        static CAPABILITIES: ProviderCapabilities = ProviderCapabilities {
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
        &CAPABILITIES
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> std::result::Result<usize, gestalt_core::HarnessError> {
        Ok(0)
    }

    async fn stream(
        &self,
        _request: ProviderRequest,
    ) -> std::result::Result<EventStream, gestalt_core::HarnessError> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(
            AgentEvent::Stop {
                reason: StopReason::EndTurn,
            },
        )])))
    }
}

struct EmptyTools;

impl ToolCatalog for EmptyTools {
    fn schemas(&self) -> Vec<ToolSchema> {
        Vec::new()
    }

    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::Tool>> {
        None
    }
}

struct AllowPolicy;

#[async_trait::async_trait]
impl PolicyEngine for AllowPolicy {
    async fn evaluate(&self, _request: PolicyRequest) -> PolicyDecision {
        PolicyDecision::allowed(None)
    }
}

fn runtime_backed_host() -> RuntimeBackedControlHost {
    let mut config = RuntimeConfig::default();
    config.context_management_policy = Some(gestalt_core::ContextManagementPolicy {
        enabled: false,
        ..Default::default()
    });
    RuntimeBackedControlHost::with_options_and_trace_directory(
        AgentRuntimeBuilder::new()
            .provider(Arc::new(ConformanceProvider))
            .tools(Arc::new(EmptyTools))
            .assembler(Arc::new(ContextMessageAssembler::new(
                "control-conformance",
            )))
            .policy(Arc::new(AllowPolicy))
            .approval(Arc::new(AutoApprovalProvider))
            .config(config),
        Arc::new(InMemoryArtifactStore::new()),
        options(),
        None,
    )
    .unwrap()
}

#[async_trait::async_trait]
impl ConformanceHost for MockControlHost {
    fn add_approval(&self, approval: ApprovalProjectionV1) {
        self.add_approval(approval);
    }

    fn add_policy_projection(&self, projection: PolicyProjectionV1) {
        self.add_policy_projection(projection);
    }

    async fn complete_run(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
    ) -> std::result::Result<(), ControlErrorV1> {
        self.complete_run(session_id, run_id).await
    }
}

fn options() -> ControlHostOptions {
    ControlHostOptions {
        queue_capacity: 2,
        event_retention: 3,
        max_artifact_read_bytes: 4,
    }
}

async fn run_conformance<H: ConformanceHost>(host: H) {
    let start_request = StartSessionRequestV1 {
        session_id: Some(SessionIdV1("session-1".to_string())),
        idempotency_key: Some(IdempotencyKeyV1("start-1".to_string())),
        config_override: None,
    };
    let started = host.start_session(start_request.clone()).await.unwrap();
    assert_eq!(
        host.inspect_session(InspectSessionRequestV1 {
            session_id: started.session_id.clone(),
        })
        .await
        .unwrap()
        .active_run_id,
        Some(started.run_id.clone())
    );
    assert_eq!(
        host.inspect_run(InspectRunRequestV1 {
            session_id: started.session_id.clone(),
            run_id: started.run_id.clone(),
        })
        .await
        .unwrap()
        .status,
        RunStatusV1::Running
    );
    let initial_cursor = host
        .poll_events(PollEventsRequestV1 {
            session_id: started.session_id.clone(),
            cursor: None,
            limit: Some(0),
            kinds: None,
        })
        .await
        .unwrap()
        .next_cursor
        .unwrap();
    assert_eq!(
        host.start_session(start_request).await.unwrap(),
        started,
        "same idempotency key must replay the original response"
    );

    let conflict = host
        .start_session(StartSessionRequestV1 {
            session_id: Some(SessionIdV1("other".to_string())),
            idempotency_key: Some(IdempotencyKeyV1("start-1".to_string())),
            config_override: None,
        })
        .await
        .unwrap_err();
    assert_eq!(conflict.code, ControlErrorCodeV1::Conflict);

    let submit = SubmitMessageRequestV1 {
        session_id: started.session_id.clone(),
        message: "hello".to_string(),
        idempotency_key: Some(IdempotencyKeyV1("message-1".to_string())),
    };
    let acknowledgement = host.submit_message(submit.clone()).await.unwrap();
    assert!(acknowledgement.acknowledged);
    assert_eq!(
        host.submit_message(submit).await.unwrap(),
        acknowledgement,
        "message idempotency must not enqueue twice"
    );

    host.submit_message(SubmitMessageRequestV1 {
        session_id: started.session_id.clone(),
        message: "second".to_string(),
        idempotency_key: None,
    })
    .await
    .unwrap();
    let full = host
        .submit_message(SubmitMessageRequestV1 {
            session_id: started.session_id.clone(),
            message: "third".to_string(),
            idempotency_key: None,
        })
        .await
        .unwrap_err();
    assert_eq!(full.code, ControlErrorCodeV1::QueueFull);

    let concurrent = host
        .start_session(StartSessionRequestV1 {
            session_id: Some(SessionIdV1("session-concurrent".to_string())),
            idempotency_key: None,
            config_override: None,
        })
        .await
        .unwrap();
    let send = |message: &str| {
        host.submit_message(SubmitMessageRequestV1 {
            session_id: concurrent.session_id.clone(),
            message: message.to_string(),
            idempotency_key: None,
        })
    };
    let results = tokio::join!(send("one"), send("two"), send("three"));
    let results: [_; 3] = results.into();
    assert_eq!(
        results
            .into_iter()
            .filter(|result| {
                result
                    .as_ref()
                    .is_err_and(|error| error.code == ControlErrorCodeV1::QueueFull)
            })
            .count(),
        1
    );

    host.complete_run(&started.session_id, &started.run_id)
        .await
        .unwrap();
    let branch_request = BranchSessionRequestV1 {
        parent_session_id: started.session_id.clone(),
        parent_run_id: started.run_id.clone(),
        new_session_id: Some(SessionIdV1("session-branch".to_string())),
        idempotency_key: Some(IdempotencyKeyV1("branch-1".to_string())),
    };
    let branched = host.branch_session(branch_request.clone()).await.unwrap();
    assert_ne!(branched.new_session_id, started.session_id);
    assert_eq!(host.branch_session(branch_request).await.unwrap(), branched);
    let resume_request = ResumeSessionRequestV1 {
        session_id: started.session_id.clone(),
        run_id: started.run_id.clone(),
        idempotency_key: Some(IdempotencyKeyV1("resume-1".to_string())),
    };
    let resumed = host.resume_session(resume_request.clone()).await.unwrap();
    assert_eq!(host.resume_session(resume_request).await.unwrap(), resumed);
    host.complete_run(&started.session_id, &started.run_id)
        .await
        .unwrap();

    let events = host
        .poll_events(PollEventsRequestV1 {
            session_id: started.session_id.clone(),
            cursor: None,
            limit: None,
            kinds: None,
        })
        .await
        .unwrap();
    assert!(matches!(
        events.events.last().map(|event| &event.payload),
        Some(EventPayloadV1::RunCompleted)
    ));
    let completed_events = host
        .poll_events(PollEventsRequestV1 {
            session_id: started.session_id.clone(),
            cursor: None,
            limit: None,
            kinds: Some(vec!["run_completed".to_string()]),
        })
        .await
        .unwrap();
    assert!(completed_events
        .events
        .iter()
        .all(|event| matches!(event.payload, EventPayloadV1::RunCompleted)));
    let wrong_stream = host
        .poll_events(PollEventsRequestV1 {
            session_id: started.session_id.clone(),
            cursor: Some(CursorV1::new("other:0")),
            limit: None,
            kinds: None,
        })
        .await
        .unwrap_err();
    assert_eq!(wrong_stream.code, ControlErrorCodeV1::Validation);
    let lagged = host
        .poll_events(PollEventsRequestV1 {
            session_id: started.session_id.clone(),
            cursor: Some(initial_cursor.clone()),
            limit: None,
            kinds: None,
        })
        .await
        .unwrap_err();
    assert_eq!(lagged.code, ControlErrorCodeV1::LaggedCursor);
    let (cursor_prefix, _) = initial_cursor.as_str().rsplit_once(':').unwrap();
    let expired = host
        .poll_events(PollEventsRequestV1 {
            session_id: started.session_id.clone(),
            cursor: Some(CursorV1::new(format!("{cursor_prefix}:0"))),
            limit: None,
            kinds: None,
        })
        .await
        .unwrap_err();
    assert_eq!(expired.code, ControlErrorCodeV1::ExpiredCursor);

    let metadata = host
        .create_artifact(CreateArtifactRequestV1 {
            session_id: started.session_id.clone(),
            display_path: "result.txt".to_string(),
            data: b"abcdef".to_vec(),
        })
        .await
        .unwrap()
        .metadata;
    let range = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id: started.session_id.clone(),
            artifact_id: metadata.logical_id.clone(),
            offset: 1,
            length: 3,
        })
        .await
        .unwrap();
    assert_eq!(range.data, b"bcd");
    let oversized = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id: started.session_id.clone(),
            artifact_id: metadata.logical_id.clone(),
            offset: 0,
            length: 5,
        })
        .await
        .unwrap_err();
    assert_eq!(oversized.code, ControlErrorCodeV1::Validation);
    let invalid_range = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id: started.session_id.clone(),
            artifact_id: metadata.logical_id.clone(),
            offset: 99,
            length: 1,
        })
        .await
        .unwrap_err();
    assert_eq!(invalid_range.code, ControlErrorCodeV1::Validation);
    let cross_session = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id: branched.new_session_id,
            artifact_id: metadata.logical_id,
            offset: 0,
            length: 1,
        })
        .await
        .unwrap_err();
    assert_eq!(cross_session.code, ControlErrorCodeV1::NotFound);
    let traversal = host
        .create_artifact(CreateArtifactRequestV1 {
            session_id: started.session_id.clone(),
            display_path: "../secret".to_string(),
            data: Vec::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(traversal.code, ControlErrorCodeV1::Validation);

    let approval_id = ApprovalIdV1("approval-1".to_string());
    let tool_call_id = ToolCallIdV1("tool-call-1".to_string());
    host.add_policy_projection(PolicyProjectionV1 {
        tool_call_id: tool_call_id.clone(),
        canonical_tool_id: "builtin:test".to_string(),
        input_hash: "input-hash".to_string(),
        risk_level: RiskLevelV1::Low,
        execution_backend: ExecutionBackendV1::Local,
        decision: PolicyDecisionV1::RequiresApproval,
        reason: Some("test".to_string()),
        matched_rule: Some("test-rule".to_string()),
        source: Some("test".to_string()),
    });
    assert_eq!(
        host.get_policy_projection(tool_call_id.clone())
            .await
            .unwrap()
            .tool_call_id,
        tool_call_id
    );
    host.add_approval(ApprovalProjectionV1 {
        approval_id: approval_id.clone(),
        tool_call_id,
        correlation_id: None,
        summary: "approve".to_string(),
        editable_input_rules: Some(serde_json::json!({"type": "object"})),
        original_hash: "original".to_string(),
        edited_hash: None,
        expires_at: None,
        is_cancelled: false,
        session_grant_terms: None,
    });
    let approved = host
        .respond_to_approval(RespondToApprovalRequestV1 {
            approval_id: approval_id.clone(),
            decision: ApprovalDecisionV1::Edit(serde_json::json!({"value": 1})),
        })
        .await
        .unwrap();
    assert!(approved.edited_hash.is_some());
    let duplicate = host
        .respond_to_approval(RespondToApprovalRequestV1 {
            approval_id,
            decision: ApprovalDecisionV1::Approve,
        })
        .await
        .unwrap_err();
    assert_eq!(duplicate.code, ControlErrorCodeV1::Conflict);

    let invalid_edit_id = ApprovalIdV1("approval-invalid-edit".to_string());
    host.add_approval(ApprovalProjectionV1 {
        approval_id: invalid_edit_id.clone(),
        tool_call_id: ToolCallIdV1("tool-call-1".to_string()),
        correlation_id: None,
        summary: "edit".to_string(),
        editable_input_rules: Some(serde_json::json!({"type": "object"})),
        original_hash: "original".to_string(),
        edited_hash: None,
        expires_at: None,
        is_cancelled: false,
        session_grant_terms: None,
    });
    let invalid_edit = host
        .respond_to_approval(RespondToApprovalRequestV1 {
            approval_id: invalid_edit_id,
            decision: ApprovalDecisionV1::Edit(serde_json::json!("not-an-object")),
        })
        .await
        .unwrap_err();
    assert_eq!(invalid_edit.code, ControlErrorCodeV1::Validation);

    let expired_id = ApprovalIdV1("approval-expired".to_string());
    host.add_approval(ApprovalProjectionV1 {
        approval_id: expired_id.clone(),
        tool_call_id: ToolCallIdV1("tool-call-1".to_string()),
        correlation_id: None,
        summary: "expired".to_string(),
        editable_input_rules: None,
        original_hash: "original".to_string(),
        edited_hash: None,
        expires_at: Some("1970-01-01T00:00:00Z".to_string()),
        is_cancelled: false,
        session_grant_terms: None,
    });
    let expired_approval = host
        .respond_to_approval(RespondToApprovalRequestV1 {
            approval_id: expired_id,
            decision: ApprovalDecisionV1::Approve,
        })
        .await
        .unwrap_err();
    assert_eq!(expired_approval.code, ControlErrorCodeV1::Conflict);

    let grant_id = ApprovalIdV1("approval-grant".to_string());
    host.add_approval(ApprovalProjectionV1 {
        approval_id: grant_id.clone(),
        tool_call_id: ToolCallIdV1("tool-call-1".to_string()),
        correlation_id: None,
        summary: "grant".to_string(),
        editable_input_rules: None,
        original_hash: "original".to_string(),
        edited_hash: None,
        expires_at: None,
        is_cancelled: false,
        session_grant_terms: Some(SessionGrantTermsV1 {
            tool_name: "builtin:test".to_string(),
            input_hash: "original".to_string(),
            risk_ceiling: RiskLevelV1::Low,
            matched_rule: "test-rule".to_string(),
            policy_source: "test".to_string(),
            expires_in_turns: 1,
        }),
    });
    let grant = host
        .respond_to_approval(RespondToApprovalRequestV1 {
            approval_id: grant_id,
            decision: ApprovalDecisionV1::AlwaysAllowForSession,
        })
        .await
        .unwrap();
    assert_eq!(
        grant
            .session_grant_terms
            .as_ref()
            .map(|terms| terms.expires_in_turns),
        Some(1)
    );

    let terminal = host
        .cancel_run(CancelRunRequestV1 {
            session_id: started.session_id.clone(),
            run_id: started.run_id.clone(),
            correlation_id: None,
        })
        .await
        .unwrap();
    assert!(!terminal.cancelled);

    let cancellable = host
        .start_session(StartSessionRequestV1 {
            session_id: Some(SessionIdV1("session-cancel".to_string())),
            idempotency_key: None,
            config_override: None,
        })
        .await
        .unwrap();
    host.submit_message(SubmitMessageRequestV1 {
        session_id: cancellable.session_id.clone(),
        message: "queued before cancellation".to_string(),
        idempotency_key: None,
    })
    .await
    .unwrap();
    let cancelled_approval_id = ApprovalIdV1("approval-cancelled".to_string());
    host.add_approval(ApprovalProjectionV1 {
        approval_id: cancelled_approval_id.clone(),
        tool_call_id: ToolCallIdV1("tool-call-cancelled".to_string()),
        correlation_id: Some(CorrelationIdV1(cancellable.session_id.0.clone())),
        summary: "cancelled".to_string(),
        editable_input_rules: None,
        original_hash: "original".to_string(),
        edited_hash: None,
        expires_at: None,
        is_cancelled: false,
        session_grant_terms: None,
    });
    let cancelled = host
        .cancel_run(CancelRunRequestV1 {
            session_id: cancellable.session_id.clone(),
            run_id: cancellable.run_id.clone(),
            correlation_id: None,
        })
        .await
        .unwrap();
    assert!(cancelled.cancelled);
    let cancelled_approval = host
        .respond_to_approval(RespondToApprovalRequestV1 {
            approval_id: cancelled_approval_id,
            decision: ApprovalDecisionV1::Approve,
        })
        .await
        .unwrap_err();
    assert_eq!(cancelled_approval.code, ControlErrorCodeV1::Conflict);
    let already_terminal = host
        .cancel_run(CancelRunRequestV1 {
            session_id: cancellable.session_id,
            run_id: cancellable.run_id,
            correlation_id: None,
        })
        .await
        .unwrap();
    assert!(!already_terminal.cancelled);
}

#[tokio::test]
async fn in_memory_host_conforms() {
    run_conformance(InMemoryControlHost::with_options(options())).await;
}

#[tokio::test]
async fn mock_host_conforms() {
    run_conformance(MockControlHost::with_options(options())).await;
}

#[tokio::test]
async fn runtime_backed_host_conforms() {
    run_conformance(runtime_backed_host()).await;
}

#[tokio::test]
async fn mock_host_exposes_controllable_failures() {
    let host = MockControlHost::new();
    host.fail_next(ControlErrorV1 {
        code: ControlErrorCodeV1::Unavailable,
        message: "injected".to_string(),
        retryable: true,
        details: None,
        correlation_id: None,
    });
    let error = host
        .start_session(StartSessionRequestV1 {
            session_id: None,
            idempotency_key: None,
            config_override: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ControlErrorCodeV1::Unavailable);
}

fn lifecycle_approval(
    id: &str,
    expires_at: Option<&str>,
    is_cancelled: bool,
) -> ApprovalProjectionV1 {
    ApprovalProjectionV1 {
        approval_id: ApprovalIdV1(id.to_string()),
        tool_call_id: ToolCallIdV1(format!("tool-{id}")),
        correlation_id: None,
        summary: "lifecycle test".to_string(),
        editable_input_rules: None,
        original_hash: "original".to_string(),
        edited_hash: None,
        expires_at: expires_at.map(str::to_string),
        is_cancelled,
        session_grant_terms: None,
    }
}

#[tokio::test]
async fn approval_expired_rejected() {
    let host = InMemoryControlHost::new();
    let approval = lifecycle_approval("expired", Some("1970-01-01T00:00:00Z"), false);
    host.add_approval(approval.clone());

    let error = host
        .respond_to_approval(RespondToApprovalRequestV1 {
            approval_id: approval.approval_id,
            decision: ApprovalDecisionV1::Approve,
        })
        .await
        .unwrap_err();

    assert_eq!(error.code, ControlErrorCodeV1::Conflict);
}

#[tokio::test]
async fn approval_duplicate_rejected() {
    let host = InMemoryControlHost::new();
    let approval = lifecycle_approval("duplicate", None, false);
    host.add_approval(approval.clone());
    let request = RespondToApprovalRequestV1 {
        approval_id: approval.approval_id,
        decision: ApprovalDecisionV1::Approve,
    };
    host.respond_to_approval(request.clone()).await.unwrap();

    let error = host.respond_to_approval(request).await.unwrap_err();

    assert_eq!(error.code, ControlErrorCodeV1::Conflict);
}

#[tokio::test]
async fn approval_cancelled_rejected() {
    let host = InMemoryControlHost::new();
    let approval = lifecycle_approval("cancelled", None, true);
    host.add_approval(approval.clone());

    let error = host
        .respond_to_approval(RespondToApprovalRequestV1 {
            approval_id: approval.approval_id,
            decision: ApprovalDecisionV1::Approve,
        })
        .await
        .unwrap_err();

    assert_eq!(error.code, ControlErrorCodeV1::Conflict);
}

async fn artifact_fixture(
    data: &[u8],
    max_read: usize,
) -> (InMemoryControlHost, SessionIdV1, CreateArtifactResponseV1) {
    let host = InMemoryControlHost::with_options(ControlHostOptions {
        max_artifact_read_bytes: max_read as u64,
        ..ControlHostOptions::default()
    });
    let started = host
        .start_session(StartSessionRequestV1 {
            session_id: None,
            idempotency_key: None,
            config_override: None,
        })
        .await
        .unwrap();
    let artifact = host
        .create_artifact(CreateArtifactRequestV1 {
            session_id: started.session_id.clone(),
            display_path: "result.txt".to_string(),
            data: data.to_vec(),
        })
        .await
        .unwrap();
    (host, started.session_id, artifact)
}

#[tokio::test]
async fn artifact_create_rejects_traversal() {
    let (host, session_id, _) = artifact_fixture(b"x", 4).await;
    for path in ["../secret", "dir/../secret", r"dir\..\secret"] {
        let error = host
            .create_artifact(CreateArtifactRequestV1 {
                session_id: session_id.clone(),
                display_path: path.to_string(),
                data: Vec::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, ControlErrorCodeV1::Validation, "{path}");
    }
}

#[tokio::test]
async fn artifact_create_rejects_absolute_path() {
    let (host, session_id, _) = artifact_fixture(b"x", 4).await;
    for path in ["/secret", "C:/secret", "", "bad\nname"] {
        let error = host
            .create_artifact(CreateArtifactRequestV1 {
                session_id: session_id.clone(),
                display_path: path.to_string(),
                data: Vec::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, ControlErrorCodeV1::Validation, "{path:?}");
    }
}

#[tokio::test]
async fn artifact_read_rejects_oversized_chunk() {
    let (host, session_id, artifact) = artifact_fixture(b"abcdef", 4).await;
    let error = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id,
            artifact_id: artifact.metadata.logical_id,
            offset: 0,
            length: 5,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ControlErrorCodeV1::Validation);
}

#[tokio::test]
async fn artifact_read_rejects_offset_overflow() {
    let (host, session_id, artifact) = artifact_fixture(b"abcdef", 4).await;
    let error = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id,
            artifact_id: artifact.metadata.logical_id,
            offset: u64::MAX,
            length: 1,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ControlErrorCodeV1::Validation);
}

#[tokio::test]
async fn artifact_read_rejects_range_past_eof() {
    let (host, session_id, artifact) = artifact_fixture(b"abcdef", 4).await;
    let error = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id: session_id.clone(),
            artifact_id: artifact.metadata.logical_id.clone(),
            offset: 4,
            length: 3,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ControlErrorCodeV1::Validation);

    let empty = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id,
            artifact_id: artifact.metadata.logical_id,
            offset: 6,
            length: 0,
        })
        .await
        .unwrap();
    assert!(empty.data.is_empty());
}

#[tokio::test]
async fn artifact_cross_session_access_denied_or_not_found() {
    let (host, _, artifact) = artifact_fixture(b"secret", 4).await;
    let other = host
        .start_session(StartSessionRequestV1 {
            session_id: None,
            idempotency_key: None,
            config_override: None,
        })
        .await
        .unwrap();
    let error = host
        .read_artifact_range(ReadArtifactRangeRequestV1 {
            session_id: other.session_id,
            artifact_id: artifact.metadata.logical_id,
            offset: 0,
            length: 1,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ControlErrorCodeV1::NotFound);
}

#[tokio::test]
async fn artifact_integrity_matches_content() {
    let content = b"integrity";
    let (_, _, artifact) = artifact_fixture(content, content.len()).await;
    assert_eq!(
        artifact.metadata.integrity,
        format!("{:x}", sha2::Sha256::digest(content))
    );
    assert_eq!(artifact.metadata.media_type, "application/octet-stream");
}

#[test]
fn dto_families_accept_unknown_additive_fields() {
    let value = serde_json::json!({
        "session_id": "session-1",
        "idempotency_key": null,
        "config_override": null,
        "future_field": true
    });
    let request: StartSessionRequestV1 = serde_json::from_value(value).unwrap();
    assert_eq!(request.session_id.unwrap().0, "session-1");

    let unknown: EventPayloadV1 =
        serde_json::from_value(serde_json::json!({"kind": "future_event"})).unwrap();
    assert_eq!(unknown, EventPayloadV1::Unknown);
}

#[test]
fn dto_families_round_trip() {
    fn round_trip<T>(value: T)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(serde_json::from_value::<T>(json).unwrap(), value);
    }

    round_trip(PolicyProjectionV1 {
        tool_call_id: ToolCallIdV1("tool-call".to_string()),
        canonical_tool_id: "builtin:test".to_string(),
        input_hash: "hash".to_string(),
        risk_level: RiskLevelV1::High,
        execution_backend: ExecutionBackendV1::Sandbox,
        decision: PolicyDecisionV1::RequiresApproval,
        reason: None,
        matched_rule: None,
        source: None,
    });
    round_trip(ApprovalProjectionV1 {
        approval_id: ApprovalIdV1("approval".to_string()),
        tool_call_id: ToolCallIdV1("tool-call".to_string()),
        correlation_id: None,
        summary: "summary".to_string(),
        editable_input_rules: None,
        original_hash: "hash".to_string(),
        edited_hash: None,
        expires_at: None,
        is_cancelled: false,
        session_grant_terms: None,
    });
    round_trip(EventEnvelopeV1 {
        schema_version: 1,
        sequence_number: 1,
        run_id: RunIdV1("run".to_string()),
        session_id: SessionIdV1("session".to_string()),
        timestamp: "1970-01-01T00:00:00Z".to_string(),
        payload: EventPayloadV1::RunCompleted,
    });
    round_trip(ArtifactMetadataV1 {
        logical_id: ArtifactIdV1("artifact".to_string()),
        display_path: "result.txt".to_string(),
        size: 1,
        media_type: "text/plain".to_string(),
        integrity: "hash".to_string(),
    });
    round_trip(InspectRuntimeResponseV1 {
        generation: "1".to_string(),
        extension_health: Vec::new(),
        active_sessions_count: 1,
    });
    round_trip(ControlErrorV1 {
        code: ControlErrorCodeV1::Validation,
        message: "invalid".to_string(),
        retryable: false,
        details: None,
        correlation_id: None,
    });
}
