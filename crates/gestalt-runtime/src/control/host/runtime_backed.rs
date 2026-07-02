use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gestalt_core::event::{AgentEvent, PolicyStatus};
use gestalt_core::session::Session;
use gestalt_core::session_queue::{MessageSource, QueueAck, QueuedSessionMessage};
use gestalt_core::tool::RiskLevel;
use sha2::Digest;

use super::{InMemoryControl, InMemoryControlHost};
use crate::artifact_store::ArtifactStore;
use crate::control::contract::{
    ApprovalControlV1, ApprovalDecisionV1, ApprovalIdV1, ApprovalProjectionV1, ArtifactAccessV1,
    ArtifactIdV1, ArtifactMetadataV1, BranchSessionRequestV1, BranchSessionResponseV1,
    CancelRunRequestV1, CancelRunResponseV1, ContinueSessionRequestV1, ContinueSessionResponseV1,
    ControlErrorCodeV1, ControlErrorV1, CreateArtifactRequestV1, CreateArtifactResponseV1,
    DescribeArtifactRequestV1, DescribeArtifactResponseV1, EventPayloadV1, EventSourceV1,
    ExtensionHealthV1, IdempotencyKeyV1, InspectRunRequestV1, InspectRunResponseV1,
    InspectRuntimeRequestV1, InspectRuntimeResponseV1, InspectSessionRequestV1,
    InspectSessionResponseV1, ListArtifactsRequestV1, ListArtifactsResponseV1,
    ListPendingApprovalsRequestV1, ListPendingApprovalsResponseV1, ListRunsRequestV1,
    ListRunsResponseV1, ListSessionsRequestV1, ListSessionsResponseV1, MessageIdV1,
    PolicyDecisionV1, PolicyProjectionV1, PollEventsRequestV1, PollEventsResponseV1,
    ReadArtifactRangeRequestV1, ReadArtifactRangeResponseV1, RespondToApprovalRequestV1,
    RespondToApprovalResponseV1, ResumeSessionRequestV1, ResumeSessionResponseV1, RiskLevelV1,
    RunIdV1, RunQueryV1, RunStatusV1, RuntimeControlV1, RuntimeInspectionV1, SessionControlV1,
    SessionGrantTermsV1, SessionIdV1, StartSessionRequestV1, StartSessionResponseV1,
    SubmitMessageRequestV1, SubmitMessageResponseV1, ToolCallIdV1,
};
use crate::control::{ControlHostOptions, HostControl};
use crate::error::RuntimeError;
use crate::{AgentRuntime, AgentRuntimeBuilder, RuntimeHost};

#[derive(Clone)]
struct RuntimeSession {
    runtime: Arc<AgentRuntime>,
    session: Arc<tokio::sync::Mutex<Session>>,
}

#[derive(Default)]
struct RuntimeBackedState {
    sessions: HashMap<SessionIdV1, RuntimeSession>,
    cancellations: HashMap<RunIdV1, gestalt_core::CancelToken>,
    continue_idempotency: HashMap<IdempotencyKeyV1, (serde_json::Value, ContinueSessionResponseV1)>,
    submit_idempotency: HashMap<IdempotencyKeyV1, (serde_json::Value, SubmitMessageResponseV1)>,
    #[cfg(feature = "trace")]
    trace_paths: HashMap<RunIdV1, std::path::PathBuf>,
}

/// Runtime-control implementation that owns real `AgentRuntime` sessions.
#[derive(Clone)]
pub struct RuntimeBackedControlHost {
    control: InMemoryControlHost,
    runtime_host: Arc<RuntimeHost>,
    state: Arc<Mutex<RuntimeBackedState>>,
    session_lock: Arc<tokio::sync::Mutex<()>>,
    trace_directory: Option<std::path::PathBuf>,
}

impl RuntimeBackedControlHost {
    pub fn new(
        builder: AgentRuntimeBuilder,
        artifact_store: Arc<dyn ArtifactStore>,
    ) -> crate::Result<Self> {
        #[cfg(feature = "trace")]
        let trace_directory = Some(builder.config.workspace_root.join(".gestalt/runs"));
        #[cfg(not(feature = "trace"))]
        let trace_directory = None;
        Self::with_options_and_trace_directory(
            builder,
            artifact_store,
            ControlHostOptions::default(),
            trace_directory,
        )
    }

    pub fn with_options(
        builder: AgentRuntimeBuilder,
        artifact_store: Arc<dyn ArtifactStore>,
        options: ControlHostOptions,
    ) -> crate::Result<Self> {
        #[cfg(feature = "trace")]
        let trace_directory = Some(builder.config.workspace_root.join(".gestalt/runs"));
        #[cfg(not(feature = "trace"))]
        let trace_directory = None;
        Self::with_options_and_trace_directory(builder, artifact_store, options, trace_directory)
    }

    pub fn with_trace_directory(
        builder: AgentRuntimeBuilder,
        artifact_store: Arc<dyn ArtifactStore>,
        trace_directory: Option<std::path::PathBuf>,
    ) -> crate::Result<Self> {
        Self::with_options_and_trace_directory(
            builder,
            artifact_store,
            ControlHostOptions::default(),
            trace_directory,
        )
    }

    pub fn with_options_and_trace_directory(
        builder: AgentRuntimeBuilder,
        artifact_store: Arc<dyn ArtifactStore>,
        options: ControlHostOptions,
        trace_directory: Option<std::path::PathBuf>,
    ) -> crate::Result<Self> {
        Ok(Self {
            control: InMemoryControlHost::with_options(options),
            runtime_host: Arc::new(RuntimeHost::new(builder, artifact_store)?),
            state: Arc::new(Mutex::new(RuntimeBackedState::default())),
            session_lock: Arc::new(tokio::sync::Mutex::new(())),
            trace_directory,
        })
    }

    pub fn add_approval(&self, approval: ApprovalProjectionV1) {
        self.control.add_approval(approval);
    }

    pub fn add_policy_projection(&self, projection: PolicyProjectionV1) {
        self.control.add_policy_projection(projection);
    }

    pub async fn complete_run(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
    ) -> Result<(), ControlErrorV1> {
        self.control.complete_run(session_id, run_id).await
    }

    fn runtime_error(error: RuntimeError) -> ControlErrorV1 {
        match error {
            RuntimeError::Harness(gestalt_core::HarnessError::Cancelled) => ControlErrorV1 {
                code: ControlErrorCodeV1::Cancelled,
                message: "runtime execution was cancelled".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            },
            error => ControlErrorV1 {
                code: ControlErrorCodeV1::InternalFailure,
                message: error.to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            },
        }
    }

    fn merge_config_override(
        &self,
        config_override: Option<serde_json::Value>,
    ) -> Result<Option<crate::RuntimeConfig>, ControlErrorV1> {
        let Some(serde_json::Value::Object(overrides)) = config_override else {
            return if config_override.is_none() {
                Ok(None)
            } else {
                Err(InMemoryControl::validation(
                    "config_override must be a JSON object",
                ))
            };
        };
        let mut base = serde_json::to_value(&self.runtime_host.config)
            .map_err(|_| InMemoryControl::validation("runtime config cannot be serialized"))?;
        let Some(base) = base.as_object_mut() else {
            return Err(InMemoryControl::validation(
                "runtime config is not a JSON object",
            ));
        };
        base.extend(overrides);
        serde_json::from_value(serde_json::Value::Object(base.clone()))
            .map(Some)
            .map_err(|error| {
                InMemoryControl::validation(format!("invalid config_override: {error}"))
            })
    }

    fn rollback_session(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
        idempotency_key: Option<&IdempotencyKeyV1>,
    ) {
        let mut state = self
            .control
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.sessions.remove(session_id);
        state.run_status.remove(run_id);
        state.events.remove(session_id);
        if let Some(key) = idempotency_key {
            state.idempotency.remove(key);
        }
        self.runtime_host.remove_session(&session_id.0);
    }

    fn trace_sink(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
    ) -> Result<Option<Arc<dyn gestalt_core::trace::TraceSink>>, ControlErrorV1> {
        #[cfg(feature = "trace")]
        {
            let Some(trace_directory) = self.trace_directory.as_ref() else {
                return Ok(None);
            };
            let (sink, paths) = crate::trace::JsonlTraceSink::create_run(
                trace_directory,
                &session_id.0,
                &run_id.0,
                None,
            )
            .map_err(|error| ControlErrorV1 {
                code: ControlErrorCodeV1::InternalFailure,
                message: format!("failed to create trace: {error}"),
                retryable: false,
                details: None,
                correlation_id: None,
            })?;
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .trace_paths
                .insert(run_id.clone(), paths.trace);
            Ok(Some(Arc::new(sink)))
        }
        #[cfg(not(feature = "trace"))]
        {
            let _ = &self.trace_directory;
            let _ = (session_id, run_id);
            Ok(None)
        }
    }

    fn control_run(&self, session_id: &SessionIdV1) -> Result<RunIdV1, ControlErrorV1> {
        self.control
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .get(session_id)
            .and_then(|session| session.active_run.clone())
            .ok_or_else(|| InMemoryControl::not_found("active run not found"))
    }

    fn project_agent_event(&self, session_id: &SessionIdV1, run_id: &RunIdV1, event: AgentEvent) {
        let logical_artifact_id = match &event {
            AgentEvent::ArtifactCreated { path, .. } => {
                self.store_tool_artifact(session_id, std::path::Path::new(path))
            }
            _ => None,
        };
        let mut state = self
            .control
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let payload = match event {
            AgentEvent::PolicyDecision {
                tool_call_id,
                tool_name,
                input_hash,
                risk,
                decision,
                reason,
                matched_rule,
                policy_source,
                ..
            } => {
                let tool_call_id = ToolCallIdV1(tool_call_id);
                state.policy_projections.insert(
                    tool_call_id.clone(),
                    PolicyProjectionV1 {
                        tool_call_id,
                        canonical_tool_id: tool_name.unwrap_or_else(|| "unknown".to_string()),
                        input_hash: input_hash.unwrap_or_default(),
                        risk_level: risk.map_or(RiskLevelV1::Low, Self::risk),
                        execution_backend: crate::control::contract::ExecutionBackendV1::Local,
                        decision: match decision {
                            PolicyStatus::Allowed => PolicyDecisionV1::Allow,
                            PolicyStatus::Denied => PolicyDecisionV1::Deny,
                            PolicyStatus::Confirm => PolicyDecisionV1::RequiresApproval,
                        },
                        reason,
                        matched_rule,
                        source: Some(policy_source),
                    },
                );
                EventPayloadV1::Unknown
            }
            AgentEvent::ApprovalRequested {
                tool_call_id,
                tool_name,
                input,
                risk,
            } => {
                let approval_id = ApprovalIdV1(tool_call_id.clone());
                let policy = state
                    .policy_projections
                    .get(&ToolCallIdV1(tool_call_id.clone()))
                    .cloned();
                state.approvals.insert(
                    approval_id.clone(),
                    ApprovalProjectionV1 {
                        approval_id: approval_id.clone(),
                        tool_call_id: ToolCallIdV1(tool_call_id),
                        correlation_id: Some(crate::control::contract::CorrelationIdV1(
                            session_id.0.clone(),
                        )),
                        summary: format!("Approve {tool_name}"),
                        editable_input_rules: Some(serde_json::json!({"type": "object"})),
                        original_hash: gestalt_core::hash_input(&input),
                        edited_hash: None,
                        expires_at: None,
                        is_cancelled: false,
                        session_grant_terms: Some(SessionGrantTermsV1 {
                            tool_name,
                            input_hash: gestalt_core::hash_input(&input),
                            risk_ceiling: Self::risk(risk),
                            matched_rule: policy
                                .as_ref()
                                .and_then(|projection| projection.matched_rule.clone())
                                .unwrap_or_else(|| "runtime_policy".to_string()),
                            policy_source: policy
                                .and_then(|projection| projection.source)
                                .unwrap_or_else(|| "runtime".to_string()),
                            expires_in_turns: self.runtime_host.config.max_turns.max(1),
                        }),
                    },
                );
                EventPayloadV1::ApprovalRequested { approval_id }
            }
            AgentEvent::ArtifactCreated { .. } => logical_artifact_id
                .map_or(EventPayloadV1::Unknown, |artifact_id| {
                    EventPayloadV1::ArtifactCreated { artifact_id }
                }),
            _ => EventPayloadV1::Unknown,
        };
        self.control
            .inner
            .push_event(&mut state, session_id, run_id, payload);
    }

    const fn risk(risk: RiskLevel) -> RiskLevelV1 {
        match risk {
            RiskLevel::Low => RiskLevelV1::Low,
            RiskLevel::Medium => RiskLevelV1::Medium,
            RiskLevel::High => RiskLevelV1::High,
            RiskLevel::Critical => RiskLevelV1::Critical,
        }
    }

    async fn execute(
        self,
        runtime_session: RuntimeSession,
        session_id: SessionIdV1,
        run_id: RunIdV1,
        message: String,
        cancel_token: gestalt_core::CancelToken,
    ) {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
        let event_host = self.clone();
        let event_session_id = session_id.clone();
        let event_run_id = run_id.clone();
        let event_task = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                event_host.project_agent_event(&event_session_id, &event_run_id, event);
            }
        });

        let result = {
            let mut session = runtime_session.session.lock().await;
            runtime_session
                .runtime
                .append_user_message(&mut session, message, Some(&event_tx));
            runtime_session
                .runtime
                .run_session(&mut session, &cancel_token, Some(event_tx), None)
                .await
        };
        let _ = event_task.await;
        let result = match result {
            Ok(run) => match runtime_session
                .runtime
                .trace_sink
                .as_ref()
                .map(|sink| sink.flush())
                .transpose()
            {
                Ok(_) => Ok(run),
                Err(error) => Err(RuntimeError::Harness(error.into())),
            },
            error => error,
        };

        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancellations
            .remove(&run_id);

        if cancel_token.is_cancelled() {
            return;
        }
        match result {
            Ok(_) => {
                let _ = self.control.complete_run(&session_id, &run_id).await;
            }
            Err(error) => {
                let mut state = self
                    .control
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(status) = state.run_status.get_mut(&run_id) {
                    *status = RunStatusV1::Failed;
                }
                if let Some(session) = state.sessions.get_mut(&session_id) {
                    session.active_run = None;
                }
                self.control.inner.push_event(
                    &mut state,
                    &session_id,
                    &run_id,
                    EventPayloadV1::RunFailed {
                        message: error.to_string(),
                    },
                );
            }
        }
    }

    fn artifact_metadata(name: String, data: &[u8]) -> ArtifactMetadataV1 {
        ArtifactMetadataV1 {
            logical_id: ArtifactIdV1(name.clone()),
            display_path: name,
            size: data.len() as u64,
            media_type: "application/octet-stream".to_string(),
            integrity: format!("{:x}", sha2::Sha256::digest(data)),
        }
    }

    fn store_tool_artifact(
        &self,
        session_id: &SessionIdV1,
        path: &std::path::Path,
    ) -> Option<ArtifactIdV1> {
        let content = std::fs::read(path).ok()?;
        let file_name = path.file_name()?.to_str()?;
        InMemoryControl::validate_artifact_display_path(file_name).ok()?;
        let integrity = format!("{:x}", sha2::Sha256::digest(&content));
        let logical_id = format!("{}-{file_name}", &integrity[..16]);
        self.runtime_host
            .artifact_store()
            .put_artifact(&session_id.0, &logical_id, &content)
            .ok()?;
        Some(ArtifactIdV1(logical_id))
    }

    fn map_queue_ack(ack: QueueAck) -> Result<bool, ControlErrorV1> {
        match ack {
            QueueAck::Queued | QueueAck::Duplicate => Ok(true),
            QueueAck::Full => Err(ControlErrorV1 {
                code: ControlErrorCodeV1::QueueFull,
                message: "session queue is full".to_string(),
                retryable: true,
                details: None,
                correlation_id: None,
            }),
            QueueAck::Conflict => Err(InMemoryControl::conflict(
                "idempotency key was already used with different input",
            )),
            QueueAck::SessionNotActive | QueueAck::SessionClosing => Err(
                InMemoryControl::conflict("session is not accepting messages"),
            ),
        }
    }
}

#[async_trait::async_trait]
impl SessionControlV1 for RuntimeBackedControlHost {
    async fn start_session(
        &self,
        req: StartSessionRequestV1,
    ) -> Result<StartSessionResponseV1, ControlErrorV1> {
        let _guard = self.session_lock.lock().await;
        let config_override = self.merge_config_override(req.config_override.clone())?;
        let response = self.control.start_session(req.clone()).await?;
        if self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .contains_key(&response.session_id)
        {
            return Ok(response);
        }

        let trace_sink = match self.trace_sink(&response.session_id, &response.run_id) {
            Ok(trace_sink) => trace_sink,
            Err(error) => {
                self.rollback_session(
                    &response.session_id,
                    &response.run_id,
                    req.idempotency_key.as_ref(),
                );
                return Err(error);
            }
        };
        if let Err(error) = self.runtime_host.spawn_session_with_trace_sink(
            &response.session_id.0,
            config_override,
            trace_sink,
        ) {
            self.rollback_session(
                &response.session_id,
                &response.run_id,
                req.idempotency_key.as_ref(),
            );
            return Err(Self::runtime_error(error));
        }
        let runtime = match self.runtime_host.session_runtime(&response.session_id.0) {
            Ok(runtime) => runtime,
            Err(error) => {
                self.rollback_session(
                    &response.session_id,
                    &response.run_id,
                    req.idempotency_key.as_ref(),
                );
                return Err(Self::runtime_error(error));
            }
        };
        let session = match runtime
            .create_session(response.session_id.0.clone(), None, None)
            .await
        {
            Ok(session) => session,
            Err(error) => {
                self.rollback_session(
                    &response.session_id,
                    &response.run_id,
                    req.idempotency_key.as_ref(),
                );
                return Err(Self::runtime_error(error));
            }
        };
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .insert(
                response.session_id.clone(),
                RuntimeSession {
                    runtime,
                    session: Arc::new(tokio::sync::Mutex::new(session)),
                },
            );
        Ok(response)
    }

    async fn continue_session(
        &self,
        req: ContinueSessionRequestV1,
    ) -> Result<ContinueSessionResponseV1, ControlErrorV1> {
        if req.message.trim().is_empty() {
            return Err(InMemoryControl::validation("message must not be empty"));
        }
        let request = serde_json::to_value(&req)
            .map_err(|_| InMemoryControl::validation("request cannot be serialized"))?;
        let response = ContinueSessionResponseV1 {
            session_id: req.session_id.clone(),
            run_id: req.run_id.clone(),
            acknowledged: true,
            correlation_id: None,
        };
        let (runtime_session, cancel_token) = {
            let mut real = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(key) = req.idempotency_key.as_ref() {
                if let Some((previous, response)) = real.continue_idempotency.get(key) {
                    if previous != &request {
                        return Err(InMemoryControl::conflict(
                            "idempotency key was already used with different input",
                        ));
                    }
                    return Ok(response.clone());
                }
            }
            if self.control_run(&req.session_id)? != req.run_id {
                return Err(InMemoryControl::conflict(
                    "run is not active for this session",
                ));
            }
            if real.cancellations.contains_key(&req.run_id) {
                return Err(InMemoryControl::conflict(
                    "session already has an active execution",
                ));
            }
            let runtime_session = real
                .sessions
                .get(&req.session_id)
                .cloned()
                .ok_or_else(|| InMemoryControl::not_found("runtime session not found"))?;
            let cancel_token = gestalt_core::CancelToken::new();
            real.cancellations
                .insert(req.run_id.clone(), cancel_token.clone());
            if let Some(key) = req.idempotency_key.clone() {
                real.continue_idempotency
                    .insert(key, (request, response.clone()));
            }
            (runtime_session, cancel_token)
        };

        {
            let mut state = self
                .control
                .inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let message_id = MessageIdV1(InMemoryControl::next_id(&mut state, "message"));
            self.control.inner.push_event(
                &mut state,
                &req.session_id,
                &req.run_id,
                EventPayloadV1::MessageQueued {
                    message_id: message_id.clone(),
                },
            );
        }

        tokio::spawn(self.clone().execute(
            runtime_session,
            req.session_id,
            req.run_id,
            req.message,
            cancel_token,
        ));
        Ok(response)
    }

    async fn resume_session(
        &self,
        req: ResumeSessionRequestV1,
    ) -> Result<ResumeSessionResponseV1, ControlErrorV1> {
        self.control.resume_session(req).await
    }

    async fn branch_session(
        &self,
        req: BranchSessionRequestV1,
    ) -> Result<BranchSessionResponseV1, ControlErrorV1> {
        let _guard = self.session_lock.lock().await;
        let response = self.control.branch_session(req.clone()).await?;
        if self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .contains_key(&response.new_session_id)
        {
            return Ok(response);
        }
        let parent = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .get(&req.parent_session_id)
            .cloned()
            .ok_or_else(|| InMemoryControl::not_found("parent runtime session not found"))?;
        let trace_sink = match self.trace_sink(&response.new_session_id, &response.new_run_id) {
            Ok(trace_sink) => trace_sink,
            Err(error) => {
                self.rollback_session(
                    &response.new_session_id,
                    &response.new_run_id,
                    req.idempotency_key.as_ref(),
                );
                return Err(error);
            }
        };
        if let Err(error) = self.runtime_host.spawn_session_with_trace_sink(
            &response.new_session_id.0,
            None,
            trace_sink,
        ) {
            self.rollback_session(
                &response.new_session_id,
                &response.new_run_id,
                req.idempotency_key.as_ref(),
            );
            return Err(Self::runtime_error(error));
        }
        let runtime = match self
            .runtime_host
            .session_runtime(&response.new_session_id.0)
        {
            Ok(runtime) => runtime,
            Err(error) => {
                self.rollback_session(
                    &response.new_session_id,
                    &response.new_run_id,
                    req.idempotency_key.as_ref(),
                );
                return Err(Self::runtime_error(error));
            }
        };
        let mut session = parent.session.lock().await.clone();
        session.id = response.new_session_id.0.clone();
        session.message_namespace = uuid::Uuid::new_v4().to_string();
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .insert(
                response.new_session_id.clone(),
                RuntimeSession {
                    runtime,
                    session: Arc::new(tokio::sync::Mutex::new(session)),
                },
            );
        Ok(response)
    }

    async fn submit_message(
        &self,
        req: SubmitMessageRequestV1,
    ) -> Result<SubmitMessageResponseV1, ControlErrorV1> {
        let run_id = self.control_run(&req.session_id)?;
        let runtime_session = {
            let real = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if real.cancellations.contains_key(&run_id) {
                Some(
                    real.sessions
                        .get(&req.session_id)
                        .cloned()
                        .ok_or_else(|| InMemoryControl::not_found("runtime session not found"))?,
                )
            } else {
                None
            }
        };
        let Some(runtime_session) = runtime_session else {
            return self.control.submit_message(req).await;
        };
        let request = serde_json::to_value(&req)
            .map_err(|_| InMemoryControl::validation("request cannot be serialized"))?;
        let message_id = MessageIdV1(format!("message-{}", uuid::Uuid::new_v4()));
        let response = SubmitMessageResponseV1 {
            session_id: req.session_id.clone(),
            message_id: message_id.clone(),
            acknowledged: true,
            correlation_id: None,
        };
        if let Some(key) = req.idempotency_key.as_ref() {
            let mut real = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some((previous, response)) = real.submit_idempotency.get(key) {
                if previous != &request {
                    return Err(InMemoryControl::conflict(
                        "idempotency key was already used with different input",
                    ));
                }
                return Ok(response.clone());
            }
            real.submit_idempotency
                .insert(key.clone(), (request.clone(), response.clone()));
        }
        let ack = runtime_session
            .runtime
            .enqueue_message_record(
                req.session_id.0.clone(),
                QueuedSessionMessage {
                    id: message_id.0.clone(),
                    content: req.message,
                    source: MessageSource::User,
                    idempotency_key: req.idempotency_key.as_ref().map(|key| key.0.clone()),
                    injected_at_turn: None,
                },
            )
            .await
            .map_err(Self::runtime_error);
        let acknowledged = match ack.and_then(Self::map_queue_ack) {
            Ok(acknowledged) => acknowledged,
            Err(error) => {
                if let Some(key) = req.idempotency_key {
                    self.state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .submit_idempotency
                        .remove(&key);
                }
                return Err(error);
            }
        };
        let mut response = response;
        response.acknowledged = acknowledged;
        let mut state = self
            .control
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.control.inner.push_event(
            &mut state,
            &req.session_id,
            &run_id,
            EventPayloadV1::MessageQueued { message_id },
        );
        Ok(response)
    }

    async fn cancel_run(
        &self,
        req: CancelRunRequestV1,
    ) -> Result<CancelRunResponseV1, ControlErrorV1> {
        let token = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancellations
            .get(&req.run_id)
            .cloned();
        if let Some(token) = token {
            token.cancel();
        }
        self.control.cancel_run(req).await
    }

    async fn inspect_session(
        &self,
        req: InspectSessionRequestV1,
    ) -> Result<InspectSessionResponseV1, ControlErrorV1> {
        self.control.inspect_session(req).await
    }
}

#[async_trait::async_trait]
impl RunQueryV1 for RuntimeBackedControlHost {
    async fn list_sessions(
        &self,
        req: ListSessionsRequestV1,
    ) -> Result<ListSessionsResponseV1, ControlErrorV1> {
        self.control.list_sessions(req).await
    }

    async fn list_runs(
        &self,
        req: ListRunsRequestV1,
    ) -> Result<ListRunsResponseV1, ControlErrorV1> {
        self.control.list_runs(req).await
    }

    async fn inspect_run(
        &self,
        req: InspectRunRequestV1,
    ) -> Result<InspectRunResponseV1, ControlErrorV1> {
        self.control.inspect_run(req).await
    }
}

#[async_trait::async_trait]
impl ApprovalControlV1 for RuntimeBackedControlHost {
    async fn list_pending_approvals(
        &self,
        req: ListPendingApprovalsRequestV1,
    ) -> Result<ListPendingApprovalsResponseV1, ControlErrorV1> {
        self.control
            .inspect_session(InspectSessionRequestV1 {
                session_id: req.session_id.clone(),
            })
            .await?;
        let approvals = self
            .control
            .list_pending_approvals(req.clone())
            .await?
            .approvals
            .into_iter()
            .filter(|approval| {
                approval
                    .correlation_id
                    .as_ref()
                    .map_or(true, |correlation| correlation.0 == req.session_id.0)
            })
            .collect();
        Ok(ListPendingApprovalsResponseV1 { approvals })
    }

    async fn respond_to_approval(
        &self,
        req: RespondToApprovalRequestV1,
    ) -> Result<RespondToApprovalResponseV1, ControlErrorV1> {
        let is_runtime = self
            .runtime_host
            .approval_broker
            .contains(&req.approval_id.0);
        let response = self.control.respond_to_approval(req.clone()).await?;
        if is_runtime {
            let decision = match req.decision {
                ApprovalDecisionV1::Approve => gestalt_core::ApprovalDecision::Approve,
                ApprovalDecisionV1::Deny => gestalt_core::ApprovalDecision::Deny,
                ApprovalDecisionV1::Edit(value) => gestalt_core::ApprovalDecision::Edit(value),
                ApprovalDecisionV1::AlwaysAllowForSession => {
                    gestalt_core::ApprovalDecision::AlwaysAllowForSession
                }
            };
            self.runtime_host
                .approval_broker
                .respond(&req.approval_id.0, decision)
                .map_err(Self::runtime_error)?;
        }
        Ok(response)
    }

    async fn get_policy_projection(
        &self,
        tool_call_id: ToolCallIdV1,
    ) -> Result<PolicyProjectionV1, ControlErrorV1> {
        self.control.get_policy_projection(tool_call_id).await
    }
}

#[async_trait::async_trait]
impl EventSourceV1 for RuntimeBackedControlHost {
    async fn poll_events(
        &self,
        req: PollEventsRequestV1,
    ) -> Result<PollEventsResponseV1, ControlErrorV1> {
        self.control.poll_events(req).await
    }
}

#[async_trait::async_trait]
impl ArtifactAccessV1 for RuntimeBackedControlHost {
    async fn list_artifacts(
        &self,
        req: ListArtifactsRequestV1,
    ) -> Result<ListArtifactsResponseV1, ControlErrorV1> {
        self.control
            .inspect_session(InspectSessionRequestV1 {
                session_id: req.session_id.clone(),
            })
            .await?;
        let mut names = self
            .runtime_host
            .list_artifacts(&req.session_id.0)
            .await
            .map_err(Self::runtime_error)?;
        names.sort();
        let mut artifacts = Vec::with_capacity(names.len());
        for name in names {
            let data = self
                .runtime_host
                .read_artifact(&req.session_id.0, &name)
                .await
                .map_err(Self::runtime_error)?;
            artifacts.push(Self::artifact_metadata(name, &data));
        }
        Ok(ListArtifactsResponseV1 {
            artifacts,
            next_cursor: None,
        })
    }

    async fn describe_artifact(
        &self,
        req: DescribeArtifactRequestV1,
    ) -> Result<DescribeArtifactResponseV1, ControlErrorV1> {
        let data = self
            .runtime_host
            .read_artifact(&req.session_id.0, &req.artifact_id.0)
            .await
            .map_err(|_| InMemoryControl::not_found("artifact not found"))?;
        Ok(DescribeArtifactResponseV1 {
            metadata: Self::artifact_metadata(req.artifact_id.0, &data),
        })
    }

    async fn read_artifact_range(
        &self,
        req: ReadArtifactRangeRequestV1,
    ) -> Result<ReadArtifactRangeResponseV1, ControlErrorV1> {
        if req.length > self.control.inner.options.max_artifact_read_bytes {
            return Err(InMemoryControl::validation("artifact range exceeds limit"));
        }
        let data = self
            .runtime_host
            .read_artifact(&req.session_id.0, &req.artifact_id.0)
            .await
            .map_err(|_| InMemoryControl::not_found("artifact not found"))?;
        let metadata = Self::artifact_metadata(req.artifact_id.0, &data);
        let start = usize::try_from(req.offset)
            .map_err(|_| InMemoryControl::validation("artifact offset is invalid"))?;
        let length = usize::try_from(req.length)
            .map_err(|_| InMemoryControl::validation("artifact length is invalid"))?;
        let end = start
            .checked_add(length)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| InMemoryControl::validation("artifact range is invalid"))?;
        Ok(ReadArtifactRangeResponseV1 {
            metadata,
            offset: req.offset,
            length: req.length,
            data: data[start..end].to_vec(),
        })
    }

    async fn create_artifact(
        &self,
        req: CreateArtifactRequestV1,
    ) -> Result<CreateArtifactResponseV1, ControlErrorV1> {
        InMemoryControl::validate_artifact_display_path(&req.display_path)?;
        self.control
            .inspect_session(InspectSessionRequestV1 {
                session_id: req.session_id.clone(),
            })
            .await?;
        if self
            .runtime_host
            .list_artifacts(&req.session_id.0)
            .await
            .map_err(Self::runtime_error)?
            .iter()
            .any(|name| name == &req.display_path)
        {
            return Err(InMemoryControl::conflict("artifact already exists"));
        }
        self.runtime_host
            .create_artifact(&req.session_id.0, &req.display_path, &req.data)
            .await
            .map_err(Self::runtime_error)?;
        let metadata = Self::artifact_metadata(req.display_path, &req.data);
        let run_id = self
            .control
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .get(&req.session_id)
            .and_then(|session| {
                session
                    .active_run
                    .clone()
                    .or_else(|| session.runs.last().cloned())
            })
            .ok_or_else(|| InMemoryControl::not_found("run not found"))?;
        let mut state = self
            .control
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.control.inner.push_event(
            &mut state,
            &req.session_id,
            &run_id,
            EventPayloadV1::ArtifactCreated {
                artifact_id: metadata.logical_id.clone(),
            },
        );
        Ok(CreateArtifactResponseV1 { metadata })
    }
}

#[async_trait::async_trait]
impl RuntimeInspectionV1 for RuntimeBackedControlHost {
    async fn inspect_runtime(
        &self,
        _req: InspectRuntimeRequestV1,
    ) -> Result<InspectRuntimeResponseV1, ControlErrorV1> {
        let extension_health = self
            .runtime_host
            .extension_manager
            .combined_health(&self.runtime_host.extension_manager.active_snapshot())
            .into_iter()
            .map(|health| ExtensionHealthV1 {
                instance_id: health.instance_id,
                status: format!("{:?}", health.status).to_lowercase(),
                details: health.message,
            })
            .collect();
        Ok(InspectRuntimeResponseV1 {
            generation: self
                .runtime_host
                .extension_manager
                .current_generation()
                .0
                .to_string(),
            extension_health,
            active_sessions_count: self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .sessions
                .len(),
        })
    }
}

impl RuntimeControlV1 for RuntimeBackedControlHost {}
