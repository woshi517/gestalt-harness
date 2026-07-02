use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use gestalt_core::session_queue::{
    MessageSource, QueueAck, QueueLifecycle, QueuedSessionMessage, SteeringQueue,
};
use sha2::Digest;

use super::contract::{
    ApprovalControlV1, ApprovalDecisionV1, ApprovalIdV1, ApprovalProjectionV1, ArtifactAccessV1,
    ArtifactIdV1, ArtifactMetadataV1, BranchSessionRequestV1, BranchSessionResponseV1,
    CancelRunRequestV1, CancelRunResponseV1, ContinueSessionRequestV1, ContinueSessionResponseV1,
    ControlErrorCodeV1, ControlErrorV1, CreateArtifactRequestV1, CreateArtifactResponseV1,
    CursorV1, DescribeArtifactRequestV1, DescribeArtifactResponseV1, EventEnvelopeV1,
    EventPayloadV1, EventSourceV1, IdempotencyKeyV1, InspectRunRequestV1, InspectRunResponseV1,
    InspectRuntimeRequestV1, InspectRuntimeResponseV1, InspectSessionRequestV1,
    InspectSessionResponseV1, ListArtifactsRequestV1, ListArtifactsResponseV1,
    ListPendingApprovalsRequestV1, ListPendingApprovalsResponseV1, ListRunsRequestV1,
    ListRunsResponseV1, ListSessionsRequestV1, ListSessionsResponseV1, MessageIdV1,
    PolicyProjectionV1, PollEventsRequestV1, PollEventsResponseV1, ReadArtifactRangeRequestV1,
    ReadArtifactRangeResponseV1, RespondToApprovalRequestV1, RespondToApprovalResponseV1,
    ResumeSessionRequestV1, ResumeSessionResponseV1, RunIdV1, RunQueryV1, RunStatusV1,
    RuntimeControlV1, RuntimeInspectionV1, SessionControlV1, SessionIdV1, StartSessionRequestV1,
    StartSessionResponseV1, SubmitMessageRequestV1, SubmitMessageResponseV1, ToolCallIdV1,
};
use crate::session_queue::InMemorySteeringQueue;

mod runtime_backed;

pub use runtime_backed::RuntimeBackedControlHost;

pub const DEFAULT_CONTROL_QUEUE_CAPACITY: usize = 64;
pub const MAX_ARTIFACT_READ_BYTES: u64 = 1024 * 1024;
const CURSOR_TTL_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone)]
pub struct ControlHostOptions {
    pub queue_capacity: usize,
    pub event_retention: usize,
    pub max_artifact_read_bytes: u64,
}

impl Default for ControlHostOptions {
    fn default() -> Self {
        Self {
            queue_capacity: DEFAULT_CONTROL_QUEUE_CAPACITY,
            event_retention: 4096,
            max_artifact_read_bytes: MAX_ARTIFACT_READ_BYTES,
        }
    }
}

struct SessionState {
    runs: Vec<RunIdV1>,
    active_run: Option<RunIdV1>,
    queue: Arc<InMemorySteeringQueue>,
}

struct ArtifactState {
    metadata: ArtifactMetadataV1,
    data: Vec<u8>,
}

struct HostState {
    sessions: HashMap<SessionIdV1, SessionState>,
    run_status: HashMap<RunIdV1, RunStatusV1>,
    idempotency: HashMap<IdempotencyKeyV1, (serde_json::Value, serde_json::Value)>,
    approvals: HashMap<ApprovalIdV1, ApprovalProjectionV1>,
    answered_approvals: HashMap<ApprovalIdV1, RespondToApprovalResponseV1>,
    policy_projections: HashMap<ToolCallIdV1, PolicyProjectionV1>,
    events: HashMap<SessionIdV1, VecDeque<EventEnvelopeV1>>,
    artifacts: HashMap<SessionIdV1, HashMap<ArtifactIdV1, ArtifactState>>,
    next_id: u64,
    next_failure: Option<ControlErrorV1>,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            run_status: HashMap::new(),
            idempotency: HashMap::new(),
            approvals: HashMap::new(),
            answered_approvals: HashMap::new(),
            policy_projections: HashMap::new(),
            events: HashMap::new(),
            artifacts: HashMap::new(),
            next_id: 1,
            next_failure: None,
        }
    }
}

struct InMemoryControl {
    state: Mutex<HostState>,
    options: ControlHostOptions,
}

impl InMemoryControl {
    fn new(options: ControlHostOptions) -> Self {
        Self {
            state: Mutex::new(HostState::default()),
            options,
        }
    }

    fn validation(message: impl Into<String>) -> ControlErrorV1 {
        ControlErrorV1 {
            code: ControlErrorCodeV1::Validation,
            message: message.into(),
            retryable: false,
            details: None,
            correlation_id: None,
        }
    }

    fn validate_artifact_display_path(path: &str) -> Result<(), ControlErrorV1> {
        let has_invalid_segment = path
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."));
        let has_windows_prefix = path.as_bytes().get(1) == Some(&b':')
            && path.as_bytes().first().is_some_and(u8::is_ascii_alphabetic);
        if path.is_empty()
            || path.starts_with('/')
            || path.contains('\\')
            || has_invalid_segment
            || has_windows_prefix
            || path.chars().any(char::is_control)
        {
            return Err(Self::validation(
                "artifact display path must be a safe, non-empty logical path",
            ));
        }
        Ok(())
    }

    fn not_found(message: impl Into<String>) -> ControlErrorV1 {
        ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: message.into(),
            retryable: false,
            details: None,
            correlation_id: None,
        }
    }

    fn conflict(message: impl Into<String>) -> ControlErrorV1 {
        ControlErrorV1 {
            code: ControlErrorCodeV1::Conflict,
            message: message.into(),
            retryable: false,
            details: None,
            correlation_id: None,
        }
    }

    fn next_id(state: &mut HostState, prefix: &str) -> String {
        let id = format!("{prefix}-{}", state.next_id);
        state.next_id += 1;
        id
    }

    fn cursor_stream_key(session_id: &SessionIdV1) -> String {
        format!("{:x}", sha2::Sha256::digest(session_id.0.as_bytes()))
    }

    fn cursor(session_id: &SessionIdV1, sequence: u64) -> CursorV1 {
        CursorV1::new(format!(
            "{}:{sequence}:{}",
            Self::cursor_stream_key(session_id),
            chrono::Utc::now().timestamp()
        ))
    }

    fn push_event(
        &self,
        state: &mut HostState,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
        payload: EventPayloadV1,
    ) {
        let events = state.events.entry(session_id.clone()).or_default();
        let sequence_number = events
            .back()
            .map_or(0, |event| event.sequence_number.saturating_add(1));
        events.push_back(EventEnvelopeV1 {
            schema_version: 1,
            sequence_number,
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            payload,
        });
        while events.len() > self.options.event_retention {
            events.pop_front();
        }
    }

    fn replay<T: serde::de::DeserializeOwned>(
        state: &HostState,
        key: Option<&IdempotencyKeyV1>,
        request: &serde_json::Value,
    ) -> Result<Option<T>, ControlErrorV1> {
        let Some(key) = key else {
            return Ok(None);
        };
        let Some((previous_request, response)) = state.idempotency.get(key) else {
            return Ok(None);
        };
        if previous_request != request {
            return Err(Self::conflict(
                "idempotency key was already used with different input",
            ));
        }
        serde_json::from_value(response.clone())
            .map(Some)
            .map_err(|_| Self::validation("cached response is invalid"))
    }

    fn remember<T: serde::Serialize>(
        state: &mut HostState,
        key: Option<IdempotencyKeyV1>,
        request: serde_json::Value,
        response: &T,
    ) -> Result<(), ControlErrorV1> {
        if let Some(key) = key {
            let response = serde_json::to_value(response)
                .map_err(|_| Self::validation("response cannot be serialized"))?;
            state.idempotency.insert(key, (request, response));
        }
        Ok(())
    }

    fn add_approval(&self, approval: ApprovalProjectionV1) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .approvals
            .insert(approval.approval_id.clone(), approval);
    }

    fn add_policy_projection(&self, projection: PolicyProjectionV1) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .policy_projections
            .insert(projection.tool_call_id.clone(), projection);
    }

    async fn complete_run(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
    ) -> Result<(), ControlErrorV1> {
        let queue = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions
            .get(session_id)
            .map(|session| session.queue.clone())
            .ok_or_else(|| Self::not_found("session not found"))?;
        queue
            .update_lifecycle(QueueLifecycle::Completed)
            .await
            .map_err(|error| Self::validation(error.to_string()))?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let status = state
            .run_status
            .get_mut(run_id)
            .ok_or_else(|| Self::not_found("run not found"))?;
        *status = RunStatusV1::Completed;
        if let Some(session) = state.sessions.get_mut(session_id) {
            session.active_run = None;
        }
        self.push_event(&mut state, session_id, run_id, EventPayloadV1::RunCompleted);
        Ok(())
    }

    fn fail_next(&self, error: ControlErrorV1) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_failure = Some(error);
    }
}

#[derive(Clone)]
pub struct InMemoryControlHost {
    inner: Arc<InMemoryControl>,
}

impl InMemoryControlHost {
    pub fn new() -> Self {
        Self::with_options(ControlHostOptions::default())
    }

    pub fn with_options(options: ControlHostOptions) -> Self {
        Self {
            inner: Arc::new(InMemoryControl::new(options)),
        }
    }

    pub(crate) fn seed_session(&self, session_id: SessionIdV1, run_id: RunIdV1) {
        let mut state = self.inner.state.lock().unwrap();
        state.sessions.insert(
            session_id.clone(),
            SessionState {
                runs: vec![run_id.clone()],
                active_run: None,
                queue: Arc::new(
                    crate::session_queue::InMemorySteeringQueue::active_with_capacity(
                        self.inner.options.queue_capacity,
                    ),
                ),
            },
        );
        state.run_status.insert(run_id, RunStatusV1::Completed);
    }

    pub fn add_approval(&self, approval: ApprovalProjectionV1) {
        self.inner.add_approval(approval);
    }

    pub fn add_policy_projection(&self, projection: PolicyProjectionV1) {
        self.inner.add_policy_projection(projection);
    }

    pub async fn complete_run(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
    ) -> Result<(), ControlErrorV1> {
        self.inner.complete_run(session_id, run_id).await
    }
}

impl Default for InMemoryControlHost {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct MockControlHost {
    inner: Arc<InMemoryControl>,
}

impl MockControlHost {
    pub fn new() -> Self {
        Self::with_options(ControlHostOptions::default())
    }

    pub fn with_options(options: ControlHostOptions) -> Self {
        Self {
            inner: Arc::new(InMemoryControl::new(options)),
        }
    }

    pub fn add_approval(&self, approval: ApprovalProjectionV1) {
        self.inner.add_approval(approval);
    }

    pub fn add_policy_projection(&self, projection: PolicyProjectionV1) {
        self.inner.add_policy_projection(projection);
    }

    pub async fn complete_run(
        &self,
        session_id: &SessionIdV1,
        run_id: &RunIdV1,
    ) -> Result<(), ControlErrorV1> {
        self.inner.complete_run(session_id, run_id).await
    }

    pub fn fail_next(&self, error: ControlErrorV1) {
        self.inner.fail_next(error);
    }
}

impl Default for MockControlHost {
    fn default() -> Self {
        Self::new()
    }
}

macro_rules! impl_control_host {
    ($host:ty) => {
        #[async_trait::async_trait]
        impl SessionControlV1 for $host {
            async fn start_session(
                &self,
                req: StartSessionRequestV1,
            ) -> Result<StartSessionResponseV1, ControlErrorV1> {
                let request = serde_json::to_value(&req)
                    .map_err(|_| InMemoryControl::validation("request cannot be serialized"))?;
                let queue = Arc::new(InMemorySteeringQueue::active_with_capacity(
                    self.inner.options.queue_capacity,
                ));
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(error) = state.next_failure.take() {
                    return Err(error);
                }
                if let Some(response) =
                    InMemoryControl::replay(&state, req.idempotency_key.as_ref(), &request)?
                {
                    return Ok(response);
                }
                let session_id = req.session_id.clone().unwrap_or_else(|| {
                    SessionIdV1(InMemoryControl::next_id(&mut state, "session"))
                });
                if state.sessions.contains_key(&session_id) {
                    return Err(InMemoryControl::conflict("session already exists"));
                }
                let run_id = RunIdV1(InMemoryControl::next_id(&mut state, "run"));
                state.sessions.insert(
                    session_id.clone(),
                    SessionState {
                        runs: vec![run_id.clone()],
                        active_run: Some(run_id.clone()),
                        queue,
                    },
                );
                state.run_status.insert(run_id.clone(), RunStatusV1::Running);
                self.inner.push_event(
                    &mut state,
                    &session_id,
                    &run_id,
                    EventPayloadV1::SessionStarted,
                );
                let response = StartSessionResponseV1 {
                    session_id,
                    run_id,
                    correlation_id: None,
                };
                InMemoryControl::remember(
                    &mut state,
                    req.idempotency_key,
                    request,
                    &response,
                )?;
                Ok(response)
            }

            async fn continue_session(
                &self,
                req: ContinueSessionRequestV1,
            ) -> Result<ContinueSessionResponseV1, ControlErrorV1> {
                let inspected = self
                    .inspect_session(InspectSessionRequestV1 {
                        session_id: req.session_id.clone(),
                    })
                    .await?;
                if inspected.active_run_id.as_ref() != Some(&req.run_id) {
                    return Err(InMemoryControl::conflict(
                        "run is not active for this session",
                    ));
                }
                let response = self
                    .submit_message(SubmitMessageRequestV1 {
                        session_id: req.session_id.clone(),
                        message: req.message,
                        idempotency_key: req.idempotency_key,
                    })
                    .await?;
                Ok(ContinueSessionResponseV1 {
                    session_id: req.session_id,
                    run_id: req.run_id,
                    acknowledged: response.acknowledged,
                    correlation_id: response.correlation_id,
                })
            }

            async fn resume_session(
                &self,
                req: ResumeSessionRequestV1,
            ) -> Result<ResumeSessionResponseV1, ControlErrorV1> {
                let request = serde_json::to_value(&req)
                    .map_err(|_| InMemoryControl::validation("request cannot be serialized"))?;
                let queue = {
                    let mut state = self
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(response) =
                        InMemoryControl::replay(&state, req.idempotency_key.as_ref(), &request)?
                    {
                        return Ok(response);
                    }
                    let can_resume = state
                        .sessions
                        .get(&req.session_id)
                        .is_some_and(|session| session.runs.contains(&req.run_id))
                        && state
                            .run_status
                            .get(&req.run_id)
                            .is_some_and(|status| *status == RunStatusV1::Completed);
                    if !can_resume {
                        return Err(InMemoryControl::conflict(
                            "run is not a resumable checkpoint for this session",
                        ));
                    }
                    let session = state
                        .sessions
                        .get_mut(&req.session_id)
                        .ok_or_else(|| InMemoryControl::not_found("session not found"))?;
                    if session.active_run.is_some() {
                        return Err(InMemoryControl::conflict(
                            "session already has an active run",
                        ));
                    }
                    session.active_run = Some(req.run_id.clone());
                    let queue = session.queue.clone();
                    state
                        .run_status
                        .insert(req.run_id.clone(), RunStatusV1::Running);
                    queue
                };
                queue
                    .update_lifecycle(QueueLifecycle::Active)
                    .await
                    .map_err(|error| InMemoryControl::validation(error.to_string()))?;
                let response = ResumeSessionResponseV1 {
                    session_id: req.session_id,
                    run_id: req.run_id,
                    correlation_id: None,
                };
                InMemoryControl::remember(
                    &mut self
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                    req.idempotency_key,
                    request,
                    &response,
                )?;
                Ok(response)
            }

            async fn branch_session(
                &self,
                req: BranchSessionRequestV1,
            ) -> Result<BranchSessionResponseV1, ControlErrorV1> {
                let request = serde_json::to_value(&req)
                    .map_err(|_| InMemoryControl::validation("request cannot be serialized"))?;
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(response) =
                    InMemoryControl::replay(&state, req.idempotency_key.as_ref(), &request)?
                {
                    return Ok(response);
                }
                if !state
                    .sessions
                    .get(&req.parent_session_id)
                    .is_some_and(|session| session.runs.contains(&req.parent_run_id))
                {
                    return Err(InMemoryControl::not_found("parent session not found"));
                }
                let new_session_id = req.new_session_id.unwrap_or_else(|| {
                    SessionIdV1(InMemoryControl::next_id(&mut state, "session"))
                });
                if state.sessions.contains_key(&new_session_id) {
                    return Err(InMemoryControl::conflict("branch session already exists"));
                }
                let new_run_id = RunIdV1(InMemoryControl::next_id(&mut state, "run"));
                state.sessions.insert(
                    new_session_id.clone(),
                    SessionState {
                        runs: vec![new_run_id.clone()],
                        active_run: Some(new_run_id.clone()),
                        queue: Arc::new(InMemorySteeringQueue::active_with_capacity(
                            self.inner.options.queue_capacity,
                        )),
                    },
                );
                state
                    .run_status
                    .insert(new_run_id.clone(), RunStatusV1::Running);
                let response = BranchSessionResponseV1 {
                    new_session_id,
                    new_run_id,
                    correlation_id: None,
                };
                InMemoryControl::remember(
                    &mut state,
                    req.idempotency_key,
                    request,
                    &response,
                )?;
                Ok(response)
            }

            async fn submit_message(
                &self,
                req: SubmitMessageRequestV1,
            ) -> Result<SubmitMessageResponseV1, ControlErrorV1> {
                let request = serde_json::to_value(&req)
                    .map_err(|_| InMemoryControl::validation("request cannot be serialized"))?;
                let (run_id, message_id, queue) = {
                    let mut state = self
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(response) =
                        InMemoryControl::replay(&state, req.idempotency_key.as_ref(), &request)?
                    {
                        return Ok(response);
                    }
                    let session = state
                        .sessions
                        .get(&req.session_id)
                        .ok_or_else(|| InMemoryControl::not_found("active session not found"))?;
                    let run_id = session
                        .active_run
                        .clone()
                        .ok_or_else(|| InMemoryControl::not_found("active run not found"))?;
                    let queue = session.queue.clone();
                    let message_id =
                        MessageIdV1(InMemoryControl::next_id(&mut state, "message"));
                    (run_id, message_id, queue)
                };
                let acknowledgement = queue
                    .enqueue(QueuedSessionMessage {
                        id: message_id.0.clone(),
                        content: req.message.clone(),
                        source: MessageSource::User,
                        idempotency_key: req
                            .idempotency_key
                            .as_ref()
                            .map(|key| key.0.clone()),
                        injected_at_turn: None,
                    })
                    .await
                    .map_err(|error| InMemoryControl::validation(error.to_string()))?;
                match acknowledgement {
                    QueueAck::Full => {
                        return Err(ControlErrorV1 {
                        code: ControlErrorCodeV1::QueueFull,
                        message: "session queue is full".to_string(),
                        retryable: true,
                        details: None,
                        correlation_id: None,
                        });
                    }
                    QueueAck::Conflict => {
                        return Err(InMemoryControl::conflict(
                            "idempotency key was already used with different input",
                        ));
                    }
                    QueueAck::SessionNotActive | QueueAck::SessionClosing => {
                        return Err(InMemoryControl::conflict(
                            "session is not accepting messages",
                        ));
                    }
                    QueueAck::Duplicate | QueueAck::Queued => {}
                }
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                self.inner.push_event(
                    &mut state,
                    &req.session_id,
                    &run_id,
                    EventPayloadV1::MessageQueued {
                        message_id: message_id.clone(),
                    },
                );
                let response = SubmitMessageResponseV1 {
                    session_id: req.session_id,
                    message_id,
                    acknowledged: true,
                    correlation_id: None,
                };
                InMemoryControl::remember(
                    &mut state,
                    req.idempotency_key,
                    request,
                    &response,
                )?;
                Ok(response)
            }

            async fn cancel_run(
                &self,
                req: CancelRunRequestV1,
            ) -> Result<CancelRunResponseV1, ControlErrorV1> {
                let (cancelled, queue) = {
                    let mut state = self
                        .inner
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let status = state
                        .run_status
                        .get_mut(&req.run_id)
                        .ok_or_else(|| InMemoryControl::not_found("run not found"))?;
                    let cancelled = !matches!(
                        status,
                        RunStatusV1::Completed | RunStatusV1::Failed | RunStatusV1::Cancelled
                    );
                    if cancelled {
                        *status = RunStatusV1::Cancelled;
                        if let Some(session) = state.sessions.get_mut(&req.session_id) {
                            session.active_run = None;
                        }
                        for approval in state.approvals.values_mut() {
                            if approval.correlation_id.as_ref().map(|id| id.0.as_str())
                                == Some(req.session_id.0.as_str())
                            {
                                approval.is_cancelled = true;
                            }
                        }
                        self.inner.push_event(
                            &mut state,
                            &req.session_id,
                            &req.run_id,
                            EventPayloadV1::RunCancelled,
                        );
                    }
                    let queue = state
                        .sessions
                        .get(&req.session_id)
                        .map(|session| session.queue.clone());
                    (cancelled, queue)
                };
                if cancelled {
                    if let Some(queue) = queue {
                        queue
                            .update_lifecycle(QueueLifecycle::Completed)
                            .await
                            .map_err(|error| InMemoryControl::validation(error.to_string()))?;
                    }
                }
                Ok(CancelRunResponseV1 {
                    session_id: req.session_id,
                    run_id: req.run_id,
                    cancelled,
                })
            }

            async fn inspect_session(
                &self,
                req: InspectSessionRequestV1,
            ) -> Result<InspectSessionResponseV1, ControlErrorV1> {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let session = state
                    .sessions
                    .get(&req.session_id)
                    .ok_or_else(|| InMemoryControl::not_found("session not found"))?;
                Ok(InspectSessionResponseV1 {
                    session_id: req.session_id,
                    active_run_id: session.active_run.clone(),
                })
            }
        }

        #[async_trait::async_trait]
        impl RunQueryV1 for $host {
            async fn list_sessions(
                &self,
                _req: ListSessionsRequestV1,
            ) -> Result<ListSessionsResponseV1, ControlErrorV1> {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Ok(ListSessionsResponseV1 {
                    sessions: state.sessions.keys().cloned().collect(),
                    next_cursor: None,
                })
            }

            async fn list_runs(
                &self,
                req: ListRunsRequestV1,
            ) -> Result<ListRunsResponseV1, ControlErrorV1> {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let session = state
                    .sessions
                    .get(&req.session_id)
                    .ok_or_else(|| InMemoryControl::not_found("session not found"))?;
                Ok(ListRunsResponseV1 {
                    runs: session.runs.clone(),
                    next_cursor: None,
                })
            }

            async fn inspect_run(
                &self,
                req: InspectRunRequestV1,
            ) -> Result<InspectRunResponseV1, ControlErrorV1> {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let status = state
                    .run_status
                    .get(&req.run_id)
                    .copied()
                    .ok_or_else(|| InMemoryControl::not_found("run not found"))?;
                Ok(InspectRunResponseV1 {
                    session_id: req.session_id,
                    run_id: req.run_id,
                    status,
                })
            }
        }

        #[async_trait::async_trait]
        impl ApprovalControlV1 for $host {
            async fn list_pending_approvals(
                &self,
                _req: ListPendingApprovalsRequestV1,
            ) -> Result<ListPendingApprovalsResponseV1, ControlErrorV1> {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Ok(ListPendingApprovalsResponseV1 {
                    approvals: state.approvals.values().cloned().collect(),
                })
            }

            async fn respond_to_approval(
                &self,
                req: RespondToApprovalRequestV1,
            ) -> Result<RespondToApprovalResponseV1, ControlErrorV1> {
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.answered_approvals.contains_key(&req.approval_id) {
                    return Err(InMemoryControl::conflict("approval already answered"));
                }
                let approval = state
                    .approvals
                    .remove(&req.approval_id)
                    .ok_or_else(|| InMemoryControl::not_found("approval not found"))?;
                if approval.is_cancelled {
                    return Err(InMemoryControl::conflict("approval was cancelled"));
                }
                if approval
                    .expires_at
                    .as_deref()
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .is_some_and(|expires| expires < chrono::Utc::now())
                {
                    return Err(InMemoryControl::conflict("approval expired"));
                }
                let edited_hash = match req.decision {
                    ApprovalDecisionV1::Edit(ref value) => {
                        if approval
                            .editable_input_rules
                            .as_ref()
                            .and_then(|rules| rules.get("type"))
                            .and_then(serde_json::Value::as_str)
                            == Some("object")
                            && !value.is_object()
                        {
                            return Err(InMemoryControl::validation(
                                "edited input violates editable input rules",
                            ));
                        }
                        Some(gestalt_core::hash_input(value))
                    }
                    _ => approval.edited_hash,
                };
                let response = RespondToApprovalResponseV1 {
                    success: true,
                    original_hash: approval.original_hash,
                    edited_hash,
                    session_grant_terms: matches!(
                        req.decision,
                        ApprovalDecisionV1::AlwaysAllowForSession
                    )
                    .then_some(approval.session_grant_terms)
                    .flatten(),
                };
                state
                    .answered_approvals
                    .insert(req.approval_id, response.clone());
                Ok(response)
            }

            async fn get_policy_projection(
                &self,
                tool_call_id: ToolCallIdV1,
            ) -> Result<PolicyProjectionV1, ControlErrorV1> {
                self.inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .policy_projections
                    .get(&tool_call_id)
                    .cloned()
                    .ok_or_else(|| InMemoryControl::not_found("policy projection not found"))
            }
        }

        #[async_trait::async_trait]
        impl EventSourceV1 for $host {
            async fn poll_events(
                &self,
                req: PollEventsRequestV1,
            ) -> Result<PollEventsResponseV1, ControlErrorV1> {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let events = state.events.get(&req.session_id).cloned().unwrap_or_default();
                let oldest = events.front().map_or(0, |event| event.sequence_number);
                let start = match req.cursor {
                    Some(cursor) => {
                        let (prefix, issued_at) = cursor
                            .as_str()
                            .rsplit_once(':')
                            .ok_or_else(|| InMemoryControl::validation("cursor is malformed"))?;
                        let (stream_key, sequence) = prefix
                            .rsplit_once(':')
                            .ok_or_else(|| InMemoryControl::validation("cursor is malformed"))?;
                        if stream_key != InMemoryControl::cursor_stream_key(&req.session_id) {
                            return Err(InMemoryControl::validation(
                                "cursor belongs to another stream",
                            ));
                        }
                        let issued_at = issued_at
                            .parse::<i64>()
                            .map_err(|_| InMemoryControl::validation("cursor is malformed"))?;
                        if chrono::Utc::now().timestamp().saturating_sub(issued_at)
                            > CURSOR_TTL_SECONDS
                        {
                            return Err(ControlErrorV1 {
                                code: ControlErrorCodeV1::ExpiredCursor,
                                message: "cursor has expired".to_string(),
                                retryable: false,
                                details: Some(serde_json::json!({
                                    "newest_safe_cursor":
                                        InMemoryControl::cursor(&req.session_id, oldest)
                                })),
                                correlation_id: None,
                            });
                        }
                        sequence
                            .parse::<u64>()
                            .map_err(|_| InMemoryControl::validation("cursor is malformed"))?
                    }
                    None => oldest,
                };
                if start < oldest {
                    return Err(ControlErrorV1 {
                        code: ControlErrorCodeV1::LaggedCursor,
                        message: "cursor is outside retained history".to_string(),
                        retryable: false,
                        details: Some(serde_json::json!({
                            "newest_safe_cursor":
                                InMemoryControl::cursor(&req.session_id, oldest)
                        })),
                        correlation_id: None,
                    });
                }
                let limit = req.limit.unwrap_or(100).min(1000);
                let filtered: Vec<_> = events
                    .into_iter()
                    .filter(|event| event.sequence_number >= start)
                    .filter(|event| {
                        req.kinds.as_ref().map_or(true, |kinds| {
                            let kind = match &event.payload {
                                EventPayloadV1::SessionStarted => "session_started",
                                EventPayloadV1::MessageQueued { .. } => "message_queued",
                                EventPayloadV1::RunCompleted => "run_completed",
                                EventPayloadV1::RunFailed { .. } => "run_failed",
                                EventPayloadV1::ApprovalRequested { .. } => "approval_requested",
                                EventPayloadV1::RunCancelled => "run_cancelled",
                                EventPayloadV1::ArtifactCreated { .. } => "artifact_created",
                                EventPayloadV1::RunStarted => "run_started",
                                EventPayloadV1::AssistantText { .. } => "assistant_text",
                                EventPayloadV1::ToolCallProposed { .. } => "tool_call_proposed",
                                EventPayloadV1::PolicyDecision { .. } => "policy_decision",
                                EventPayloadV1::ToolResult { .. } => "tool_result",
                                EventPayloadV1::Unknown => "unknown",
                            };
                            kinds.iter().any(|candidate| candidate == kind)
                        })
                    })
                    .take(limit)
                    .collect();
                let next = filtered
                    .last()
                    .map_or(start, |event| event.sequence_number.saturating_add(1));
                Ok(PollEventsResponseV1 {
                    events: filtered,
                    next_cursor: Some(InMemoryControl::cursor(&req.session_id, next)),
                })
            }
        }

        #[async_trait::async_trait]
        impl ArtifactAccessV1 for $host {
            async fn list_artifacts(
                &self,
                req: ListArtifactsRequestV1,
            ) -> Result<ListArtifactsResponseV1, ControlErrorV1> {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let artifacts = state
                    .artifacts
                    .get(&req.session_id)
                    .map(|items| items.values().map(|item| item.metadata.clone()).collect())
                    .unwrap_or_default();
                Ok(ListArtifactsResponseV1 {
                    artifacts,
                    next_cursor: None,
                })
            }

            async fn describe_artifact(
                &self,
                req: DescribeArtifactRequestV1,
            ) -> Result<DescribeArtifactResponseV1, ControlErrorV1> {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let artifact = state
                    .artifacts
                    .get(&req.session_id)
                    .and_then(|items| items.get(&req.artifact_id))
                    .ok_or_else(|| InMemoryControl::not_found("artifact not found"))?;
                Ok(DescribeArtifactResponseV1 {
                    metadata: artifact.metadata.clone(),
                })
            }

            async fn read_artifact_range(
                &self,
                req: ReadArtifactRangeRequestV1,
            ) -> Result<ReadArtifactRangeResponseV1, ControlErrorV1> {
                if req.length > self.inner.options.max_artifact_read_bytes {
                    return Err(InMemoryControl::validation("artifact range exceeds limit"));
                }
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let artifact = state
                    .artifacts
                    .get(&req.session_id)
                    .and_then(|items| items.get(&req.artifact_id))
                    .ok_or_else(|| InMemoryControl::not_found("artifact not found"))?;
                let start = usize::try_from(req.offset)
                    .map_err(|_| InMemoryControl::validation("artifact offset is invalid"))?;
                let length = usize::try_from(req.length)
                    .map_err(|_| InMemoryControl::validation("artifact length is invalid"))?;
                let end = start
                    .checked_add(length)
                    .filter(|end| *end <= artifact.data.len())
                    .ok_or_else(|| InMemoryControl::validation("artifact range is invalid"))?;
                Ok(ReadArtifactRangeResponseV1 {
                    metadata: artifact.metadata.clone(),
                    offset: req.offset,
                    length: req.length,
                    data: artifact.data[start..end].to_vec(),
                })
            }

            async fn create_artifact(
                &self,
                req: CreateArtifactRequestV1,
            ) -> Result<CreateArtifactResponseV1, ControlErrorV1> {
                InMemoryControl::validate_artifact_display_path(&req.display_path)?;
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.sessions.contains_key(&req.session_id) {
                    return Err(InMemoryControl::not_found("session not found"));
                }
                let artifact_id =
                    ArtifactIdV1(InMemoryControl::next_id(&mut state, "artifact"));
                let metadata = ArtifactMetadataV1 {
                    logical_id: artifact_id.clone(),
                    display_path: req.display_path,
                    size: req.data.len() as u64,
                    media_type: "application/octet-stream".to_string(),
                    integrity: format!("{:x}", sha2::Sha256::digest(&req.data)),
                };
                state.artifacts.entry(req.session_id.clone()).or_default().insert(
                    artifact_id.clone(),
                    ArtifactState {
                        metadata: metadata.clone(),
                        data: req.data,
                    },
                );
                let run_id = state
                    .sessions
                    .get(&req.session_id)
                    .and_then(|session| {
                        session
                            .active_run
                            .clone()
                            .or_else(|| session.runs.last().cloned())
                    })
                    .ok_or_else(|| InMemoryControl::not_found("run not found"))?;
                self.inner.push_event(
                    &mut state,
                    &req.session_id,
                    &run_id,
                    EventPayloadV1::ArtifactCreated { artifact_id },
                );
                Ok(CreateArtifactResponseV1 { metadata })
            }
        }

        #[async_trait::async_trait]
        impl RuntimeInspectionV1 for $host {
            async fn inspect_runtime(
                &self,
                _req: InspectRuntimeRequestV1,
            ) -> Result<InspectRuntimeResponseV1, ControlErrorV1> {
                let state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                Ok(InspectRuntimeResponseV1 {
                    generation: "local".to_string(),
                    extension_health: Vec::new(),
                    active_sessions_count: state.sessions.len(),
                })
            }
        }

        impl RuntimeControlV1 for $host {}
    };
}

impl_control_host!(InMemoryControlHost);
impl_control_host!(MockControlHost);
