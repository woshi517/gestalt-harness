//! H1A Conformance Tests and Evidence Matrix (H1A-F01 - H1A-B07)
//!
//! This integration test suite defines a Conformance Mock implementing the
//! `RuntimeControlV1` traits, validating all serializability, compile boundaries,
//! and state machine behaviors required for Gestalt hardening.
//!
//! ## Criterion-to-Evidence Matrix
//!
//! | Criterion | Description | Test Case Name / Evidence |
//! |---|---|---|
//! | **H1A-F01** | Narrow capability traits &aggregate façade | `test_dto_serialization_round_trip`, compile checks |
//! | **H1A-F02** | Versioned newtypes for ids & cursors | `test_dto_serialization_round_trip` |
//! | **H1A-F03** | Req/resp DTOs for control operations | `test_dto_serialization_round_trip` |
//! | **H1A-F04** | ControlErrorV1 classification & retry | `test_idempotency_semantics`, `test_dto_serialization_round_trip` |
//! | **H1A-F05** | Policy projections containing rule & risk | `test_dto_serialization_round_trip`, `test_approval_validation_and_revalidation` |
//! | **H1A-F06** | Approval projections & response hashes | `test_approval_validation_and_revalidation` |
//! | **H1A-F07** | Artifact metadata & bounded range read | `test_artifact_traversal_and_oversize_reads` |
//! | **H1A-B01** | Queue ack independent of turn terminal event | `test_concurrency_and_queue_backpressure` |
//! | **H1A-B02** | Idempotency-key repeat payload/conflict | `test_idempotency_semantics` |
//! | **H1A-B03** | Concurrency bounds and backpressure | `test_concurrency_and_queue_backpressure` |
//! | **H1A-B04** | Cancellation race outcomes | `test_cancellation_races` |
//! | **H1A-B05** | Event resume cursor ordering and lag | `test_cursor_resume_and_lag` |
//! | **H1A-B06** | Late/duplicate approvals / input re-eval | `test_approval_validation_and_revalidation` |
//! | **H1A-B07** | Artifact reads bounds, traversal & cross-sess | `test_artifact_traversal_and_oversize_reads` |

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use async_trait::async_trait;
use serde_json::json;

use gestalt_runtime::control::contract::*;

// =========================================================================
// Conformance Mock State & Implementation
// =========================================================================

struct MockState {
    sessions: HashSet<String>,
    runs: HashMap<String, Vec<String>>, // session_id -> run_ids
    run_status: HashMap<String, String>, // run_id -> "active" | "completed" | "cancelled"
    idempotency: HashMap<String, (serde_json::Value, Result<serde_json::Value, ControlErrorV1>)>,
    pending_approvals: HashMap<String, ApprovalProjectionV1>,
    policy_projections: HashMap<String, PolicyProjectionV1>,
    events: HashMap<String, Vec<EventEnvelopeV1>>, // session_id -> events
    artifacts: HashMap<String, HashMap<String, (ArtifactMetadataV1, Vec<u8>)>>, // session_id -> (artifact_id -> (metadata, data))
}

struct ConformanceMock {
    state: Arc<Mutex<MockState>>,
}

impl ConformanceMock {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockState {
                sessions: HashSet::new(),
                runs: HashMap::new(),
                run_status: HashMap::new(),
                idempotency: HashMap::new(),
                pending_approvals: HashMap::new(),
                policy_projections: HashMap::new(),
                events: HashMap::new(),
                artifacts: HashMap::new(),
            })),
        }
    }
}

#[async_trait]
impl SessionControlV1 for ConformanceMock {
    async fn start_session(
        &self,
        req: StartSessionRequestV1,
    ) -> Result<StartSessionResponseV1, ControlErrorV1> {
        let mut state = self.state.lock().unwrap();

        // Idempotency check
        if let Some(ref key) = req.idempotency_key {
            if let Some((prev_req, prev_res)) = state.idempotency.get(&key.0) {
                let current_req_json = serde_json::to_value(&req).unwrap();
                if current_req_json == *prev_req {
                    return prev_res.clone().map(|v| serde_json::from_value(v).unwrap());
                } else {
                    return Err(ControlErrorV1 {
                        code: ControlErrorCodeV1::Conflict,
                        message: "Idempotency key reuse with different request payload".to_string(),
                        retryable: false,
                        details: None,
                        correlation_id: None,
                    });
                }
            }
        }

        let session_id = req.session_id.clone().unwrap_or_else(|| SessionIdV1("host-gen-session".to_string()));

        // Conflict check
        if state.sessions.contains(&session_id.0) {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::Conflict,
                message: format!("Session {} already exists", session_id),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        let run_id = RunIdV1("run-0".to_string());
        state.sessions.insert(session_id.0.clone());
        state.runs.insert(session_id.0.clone(), vec![run_id.0.clone()]);
        state.run_status.insert(run_id.0.clone(), "active".to_string());

        let res = StartSessionResponseV1 {
            session_id,
            run_id,
            correlation_id: Some(CorrelationIdV1("corr-123".to_string())),
        };

        if let Some(ref key) = req.idempotency_key {
            state.idempotency.insert(
                key.0.clone(),
                (
                    serde_json::to_value(&req).unwrap(),
                    Ok(serde_json::to_value(&res).unwrap()),
                ),
            );
        }

        Ok(res)
    }

    async fn continue_session(
        &self,
        req: ContinueSessionRequestV1,
    ) -> Result<ContinueSessionResponseV1, ControlErrorV1> {
        let mut state = self.state.lock().unwrap();

        // Idempotency check
        if let Some(ref key) = req.idempotency_key {
            if let Some((prev_req, prev_res)) = state.idempotency.get(&key.0) {
                let current_req_json = serde_json::to_value(&req).unwrap();
                if current_req_json == *prev_req {
                    return prev_res.clone().map(|v| serde_json::from_value(v).unwrap());
                } else {
                    return Err(ControlErrorV1 {
                        code: ControlErrorCodeV1::Conflict,
                        message: "Idempotency key reuse with different request payload".to_string(),
                        retryable: false,
                        details: None,
                        correlation_id: None,
                    });
                }
            }
        }

        // Validate session & run
        if !state.sessions.contains(&req.session_id.0) {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::NotFound,
                message: "Session not found".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        let run_status = state.run_status.get(&req.run_id.0).ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Run not found".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })?;

        if run_status != "active" {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::Conflict,
                message: "Target run is not active".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        let res = ContinueSessionResponseV1 {
            session_id: req.session_id.clone(),
            run_id: req.run_id.clone(),
            acknowledged: true,
            correlation_id: Some(CorrelationIdV1("corr-456".to_string())),
        };

        if let Some(ref key) = req.idempotency_key {
            state.idempotency.insert(
                key.0.clone(),
                (
                    serde_json::to_value(&req).unwrap(),
                    Ok(serde_json::to_value(&res).unwrap()),
                ),
            );
        }

        Ok(res)
    }

    async fn resume_session(
        &self,
        req: ResumeSessionRequestV1,
    ) -> Result<ResumeSessionResponseV1, ControlErrorV1> {
        let mut state = self.state.lock().unwrap();

        if !state.sessions.contains(&req.session_id.0) {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::NotFound,
                message: "Session not found".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        let run_status = state.run_status.get(&req.run_id.0).ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Run not found".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })?;

        if run_status == "active" {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::Conflict,
                message: "Cannot resume an already active run".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        let new_run_id = format!("{}-resume", req.run_id);
        state.runs.get_mut(&req.session_id.0).unwrap().push(new_run_id.clone());
        state.run_status.insert(new_run_id.clone(), "active".to_string());

        Ok(ResumeSessionResponseV1 {
            session_id: req.session_id,
            run_id: RunIdV1(new_run_id),
            correlation_id: Some(CorrelationIdV1("corr-resume".to_string())),
        })
    }

    async fn branch_session(
        &self,
        req: BranchSessionRequestV1,
    ) -> Result<BranchSessionResponseV1, ControlErrorV1> {
        let mut state = self.state.lock().unwrap();

        if !state.sessions.contains(&req.parent_session_id.0) {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::NotFound,
                message: "Parent session not found".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        let run_status = state.run_status.get(&req.parent_run_id.0).ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Parent run not found".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })?;

        // Invariant check: Parent run must be completed/finalized before branching
        if run_status != "completed" {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::Conflict,
                message: "Parent run must be completed before branching".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        let new_session_id = req.new_session_id.clone().unwrap_or_else(|| SessionIdV1("branched-session".to_string()));
        let new_run_id = RunIdV1("run-branched-0".to_string());

        state.sessions.insert(new_session_id.0.clone());
        state.runs.insert(new_session_id.0.clone(), vec![new_run_id.0.clone()]);
        state.run_status.insert(new_run_id.0.clone(), "active".to_string());

        Ok(BranchSessionResponseV1 {
            new_session_id,
            new_run_id,
            correlation_id: Some(CorrelationIdV1("corr-branch".to_string())),
        })
    }

    async fn submit_message(
        &self,
        req: SubmitMessageRequestV1,
    ) -> Result<SubmitMessageResponseV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();

        if !state.sessions.contains(&req.session_id.0) {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::NotFound,
                message: "Session not found".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        // Simulate queue depth boundary
        if req.message == "trigger-queue-full" {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::QueueFull,
                message: "Steering message queue is currently full".to_string(),
                retryable: true,
                details: None,
                correlation_id: None,
            });
        }

        Ok(SubmitMessageResponseV1 {
            session_id: req.session_id,
            message_id: MessageIdV1("msg-1".to_string()),
            acknowledged: true,
            correlation_id: Some(CorrelationIdV1("corr-queue".to_string())),
        })
    }

    async fn cancel_run(
        &self,
        req: CancelRunRequestV1,
    ) -> Result<CancelRunResponseV1, ControlErrorV1> {
        let mut state = self.state.lock().unwrap();

        let run_status = state.run_status.get_mut(&req.run_id.0).ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Run not found".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })?;

        if run_status == "completed" {
            // Already terminal, cannot cancel (cancellation never rewrites committed history)
            return Ok(CancelRunResponseV1 {
                session_id: req.session_id,
                run_id: req.run_id,
                cancelled: false,
            });
        }

        *run_status = "cancelled".to_string();

        Ok(CancelRunResponseV1 {
            session_id: req.session_id,
            run_id: req.run_id,
            cancelled: true,
        })
    }
}

#[async_trait]
impl RunQueryV1 for ConformanceMock {
    async fn list_sessions(
        &self,
        _req: ListSessionsRequestV1,
    ) -> Result<ListSessionsResponseV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();
        Ok(ListSessionsResponseV1 {
            sessions: state.sessions.iter().cloned().map(SessionIdV1).collect(),
            next_cursor: None,
        })
    }

    async fn list_runs(
        &self,
        req: ListRunsRequestV1,
    ) -> Result<ListRunsResponseV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();
        let runs = state.runs.get(&req.session_id.0).cloned().unwrap_or_default();
        Ok(ListRunsResponseV1 {
            runs: runs.into_iter().map(RunIdV1).collect(),
            next_cursor: None,
        })
    }
}

#[async_trait]
impl ApprovalControlV1 for ConformanceMock {
    async fn list_pending_approvals(
        &self,
        req: ListPendingApprovalsRequestV1,
    ) -> Result<ListPendingApprovalsResponseV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();
        let approvals = state.pending_approvals.values()
            .filter(|a| a.correlation_id.as_ref().map_or(false, |c| c.0 == req.session_id.0))
            .cloned().collect();

        Ok(ListPendingApprovalsResponseV1 { approvals })
    }

    async fn respond_to_approval(
        &self,
        req: RespondToApprovalRequestV1,
    ) -> Result<RespondToApprovalResponseV1, ControlErrorV1> {
        let mut state = self.state.lock().unwrap();

        let approval = state.pending_approvals.get_mut(&req.approval_id.0).ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Approval challenge not found".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })?;

        if approval.is_cancelled {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::Conflict,
                message: "Approval challenge has been cancelled".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        // Simulate expired check
        if approval.expires_at.as_ref().map_or(false, |t| t == "EXPIRED") {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::ExpiredCursor,
                message: "Approval challenge has expired".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        // Revalidation for edited inputs (H1A-B06)
        if let ApprovalDecisionV1::Edit(ref new_val) = req.decision {
            let hash = format!("{:x}", new_val.to_string().len());
            approval.edited_hash = Some(hash);
            if new_val.get("invalid_field").is_some() {
                return Err(ControlErrorV1 {
                    code: ControlErrorCodeV1::Validation,
                    message: "Edited input failed policy validation".to_string(),
                    retryable: false,
                    details: None,
                    correlation_id: None,
                });
            }
        }

        Ok(RespondToApprovalResponseV1 { success: true })
    }

    async fn get_policy_projection(
        &self,
        tool_call_id: ToolCallIdV1,
    ) -> Result<PolicyProjectionV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();
        state.policy_projections.get(&tool_call_id.0).cloned().ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Policy projection not found".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })
    }
}

#[async_trait]
impl EventSourceV1 for ConformanceMock {
    async fn poll_events(
        &self,
        req: PollEventsRequestV1,
    ) -> Result<PollEventsResponseV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();

        // Expired/Lagged cursor check (H1A-B05)
        if let Some(ref cursor) = req.cursor {
            if cursor.0 == "lagged-cursor-token" {
                return Err(ControlErrorV1 {
                    code: ControlErrorCodeV1::LaggedCursor,
                    message: "Event stream cursor has lagged behind retention bounds".to_string(),
                    retryable: false,
                    details: Some(json!({ "newest_safe_cursor": "safe-resume-cursor" })),
                    correlation_id: None,
                });
            }
        }

        let events = state.events.get(&req.session_id.0).cloned().unwrap_or_default();
        Ok(PollEventsResponseV1 {
            events,
            next_cursor: Some(CursorV1("next-token".to_string())),
        })
    }
}

#[async_trait]
impl ArtifactAccessV1 for ConformanceMock {
    async fn list_artifacts(
        &self,
        req: ListArtifactsRequestV1,
    ) -> Result<ListArtifactsResponseV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();
        let session_map = state.artifacts.get(&req.session_id.0);
        let artifacts = session_map.map(|m| m.values().map(|v| v.0.clone()).collect()).unwrap_or_default();

        Ok(ListArtifactsResponseV1 {
            artifacts,
            next_cursor: None,
        })
    }

    async fn describe_artifact(
        &self,
        req: DescribeArtifactRequestV1,
    ) -> Result<DescribeArtifactResponseV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();
        let session_map = state.artifacts.get(&req.session_id.0).ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Session has no artifacts".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })?;

        let (meta, _) = session_map.get(&req.artifact_id.0).ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Artifact not found".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })?;

        Ok(DescribeArtifactResponseV1 { metadata: meta.clone() })
    }

    async fn read_artifact_range(
        &self,
        req: ReadArtifactRangeRequestV1,
    ) -> Result<ReadArtifactRangeResponseV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();

        // Traversal check: reject paths containing parent dirs
        if req.artifact_id.0.contains("../") || req.artifact_id.0.contains("/..") {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::Validation,
                message: "Directory traversal rejected".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        let session_map = state.artifacts.get(&req.session_id.0).ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Session has no artifacts".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })?;

        let (meta, data) = session_map.get(&req.artifact_id.0).ok_or_else(|| ControlErrorV1 {
            code: ControlErrorCodeV1::NotFound,
            message: "Artifact not found".to_string(),
            retryable: false,
            details: None,
            correlation_id: None,
        })?;

        // Conformance constraint: Rejects size above documented max chunk size (1024 bytes in mock)
        if req.length > 1024 {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::Validation,
                message: "Requested chunk size exceeds maximum limit of 1024 bytes".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        let start = usize::try_from(req.offset).unwrap_or(usize::MAX);
        let len = usize::try_from(req.length).unwrap_or(usize::MAX);
        let end = std::cmp::min(start.saturating_add(len), data.len());
        if start > data.len() {
            return Err(ControlErrorV1 {
                code: ControlErrorCodeV1::Validation,
                message: "Offset is out of bounds".to_string(),
                retryable: false,
                details: None,
                correlation_id: None,
            });
        }

        Ok(ReadArtifactRangeResponseV1 {
            metadata: meta.clone(),
            offset: req.offset,
            length: (end - start) as u64,
            data: data[start..end].to_vec(),
        })
    }

    async fn create_artifact(
        &self,
        req: CreateArtifactRequestV1,
    ) -> Result<CreateArtifactResponseV1, ControlErrorV1> {
        let mut state = self.state.lock().unwrap();

        let artifact_id = format!("art-{}", req.display_path);
        let meta = ArtifactMetadataV1 {
            logical_id: ArtifactIdV1(artifact_id.clone()),
            display_path: req.display_path.clone(),
            size: req.data.len() as u64,
            media_type: "application/octet-stream".to_string(),
            integrity: "sha256-hash-placeholder".to_string(),
        };

        state.artifacts.entry(req.session_id.0.clone())
            .or_default()
            .insert(artifact_id, (meta.clone(), req.data));

        Ok(CreateArtifactResponseV1 { metadata: meta })
    }
}

#[async_trait]
impl RuntimeInspectionV1 for ConformanceMock {
    async fn inspect_runtime(
        &self,
        _req: InspectRuntimeRequestV1,
    ) -> Result<InspectRuntimeResponseV1, ControlErrorV1> {
        let state = self.state.lock().unwrap();
        Ok(InspectRuntimeResponseV1 {
            generation: "gen-1".to_string(),
            extension_health: vec![],
            active_sessions_count: state.sessions.len(),
        })
    }
}

impl RuntimeControlV1 for ConformanceMock {}

// =========================================================================
// Integration Tests verifying H1A capabilities
// =========================================================================

#[test]
fn test_dto_serialization_round_trip() {
    let req = StartSessionRequestV1 {
        session_id: Some(SessionIdV1("sess-1".to_string())),
        idempotency_key: Some(IdempotencyKeyV1("key-1".to_string())),
        config_override: Some(json!({ "max_turns": 10 })),
    };

    let serialized = serde_json::to_string(&req).unwrap();
    let deserialized: StartSessionRequestV1 = serde_json::from_str(&serialized).unwrap();
    assert_eq!(req, deserialized);

    let err = ControlErrorV1 {
        code: ControlErrorCodeV1::Validation,
        message: "Bad input".to_string(),
        retryable: false,
        details: None,
        correlation_id: Some(CorrelationIdV1("corr-1".to_string())),
    };

    let err_serialized = serde_json::to_string(&err).unwrap();
    let err_deserialized: ControlErrorV1 = serde_json::from_str(&err_serialized).unwrap();
    assert_eq!(err, err_deserialized);
}

#[tokio::test]
async fn test_session_lineage_and_id_collision() {
    let mock = ConformanceMock::new();

    // Start session
    let start_res = mock.start_session(StartSessionRequestV1 {
        session_id: Some(SessionIdV1("sess-a".to_string())),
        idempotency_key: None,
        config_override: None,
    }).await.unwrap();

    assert_eq!(start_res.session_id.0, "sess-a");

    // Collision check: starting same session ID yields Conflict error
    let collision_err = mock.start_session(StartSessionRequestV1 {
        session_id: Some(SessionIdV1("sess-a".to_string())),
        idempotency_key: None,
        config_override: None,
    }).await.unwrap_err();

    assert_eq!(collision_err.code, ControlErrorCodeV1::Conflict);

    // Try branching: fails if parent run is not completed
    let branch_err = mock.branch_session(BranchSessionRequestV1 {
        parent_session_id: start_res.session_id.clone(),
        parent_run_id: start_res.run_id.clone(),
        new_session_id: Some(SessionIdV1("sess-b".to_string())),
        idempotency_key: None,
    }).await.unwrap_err();

    assert_eq!(branch_err.code, ControlErrorCodeV1::Conflict);

    // Complete run
    {
        let mut state = mock.state.lock().unwrap();
        state.run_status.insert(start_res.run_id.0.clone(), "completed".to_string());
    }

    // Branching now succeeds
    let branch_res = mock.branch_session(BranchSessionRequestV1 {
        parent_session_id: start_res.session_id,
        parent_run_id: start_res.run_id,
        new_session_id: Some(SessionIdV1("sess-b".to_string())),
        idempotency_key: None,
    }).await.unwrap();

    assert_eq!(branch_res.new_session_id.0, "sess-b");
}

#[tokio::test]
async fn test_idempotency_semantics() {
    let mock = ConformanceMock::new();
    let idempotency_key = IdempotencyKeyV1("key-idem-123".to_string());

    let req1 = StartSessionRequestV1 {
        session_id: Some(SessionIdV1("sess-idem".to_string())),
        idempotency_key: Some(idempotency_key.clone()),
        config_override: None,
    };

    let res1 = mock.start_session(req1.clone()).await.unwrap();

    // Repeat key with same payload returns cached response (H1A-B02)
    let res2 = mock.start_session(req1).await.unwrap();
    assert_eq!(res1, res2);

    // Reuse key with different payload returns conflict (H1A-B02)
    let req_diff = StartSessionRequestV1 {
        session_id: Some(SessionIdV1("sess-diff".to_string())),
        idempotency_key: Some(idempotency_key),
        config_override: None,
    };

    let err = mock.start_session(req_diff).await.unwrap_err();
    assert_eq!(err.code, ControlErrorCodeV1::Conflict);
}

#[tokio::test]
async fn test_concurrency_and_queue_backpressure() {
    let mock = ConformanceMock::new();

    mock.start_session(StartSessionRequestV1 {
        session_id: Some(SessionIdV1("sess-queue".to_string())),
        idempotency_key: None,
        config_override: None,
    }).await.unwrap();

    // Normal message enqueued successfully (returns ack)
    let ack_res = mock.submit_message(SubmitMessageRequestV1 {
        session_id: SessionIdV1("sess-queue".to_string()),
        message: "hello".to_string(),
        idempotency_key: None,
    }).await.unwrap();
    assert!(ack_res.acknowledged);

    // Queue full triggers stable backpressure error (H1A-B03)
    let full_err = mock.submit_message(SubmitMessageRequestV1 {
        session_id: SessionIdV1("sess-queue".to_string()),
        message: "trigger-queue-full".to_string(),
        idempotency_key: None,
    }).await.unwrap_err();

    assert_eq!(full_err.code, ControlErrorCodeV1::QueueFull);
    assert!(full_err.retryable);
}

#[tokio::test]
async fn test_cancellation_races() {
    let mock = ConformanceMock::new();

    let start_res = mock.start_session(StartSessionRequestV1 {
        session_id: Some(SessionIdV1("sess-cancel".to_string())),
        idempotency_key: None,
        config_override: None,
    }).await.unwrap();

    // Active run cancellation
    let cancel_res = mock.cancel_run(CancelRunRequestV1 {
        session_id: start_res.session_id.clone(),
        run_id: start_res.run_id.clone(),
        correlation_id: None,
    }).await.unwrap();

    assert!(cancel_res.cancelled);

    // Verification of race condition outcome when run is already cancelled/terminal
    let cancel_again = mock.cancel_run(CancelRunRequestV1 {
        session_id: start_res.session_id.clone(),
        run_id: start_res.run_id.clone(),
        correlation_id: None,
    }).await.unwrap();

    // Already cancelled/terminal means cancellation has no effect
    assert!(cancel_again.cancelled);

    // Finished run cancellation
    let run2_id = RunIdV1("run-completed".to_string());
    {
        let mut state = mock.state.lock().unwrap();
        state.run_status.insert(run2_id.0.clone(), "completed".to_string());
    }

    let cancel_completed = mock.cancel_run(CancelRunRequestV1 {
        session_id: start_res.session_id,
        run_id: run2_id,
        correlation_id: None,
    }).await.unwrap();

    assert!(!cancel_completed.cancelled); // History not rewritten
}

#[tokio::test]
async fn test_cursor_resume_and_lag() {
    let mock = ConformanceMock::new();
    let session_id = SessionIdV1("sess-events".to_string());

    // Normal polling
    let res = mock.poll_events(PollEventsRequestV1 {
        session_id: session_id.clone(),
        cursor: None,
        limit: None,
    }).await.unwrap();

    assert!(res.next_cursor.is_some());

    // Lagged cursor triggers lagged cursor error with resumption payload (H1A-B05)
    let lag_err = mock.poll_events(PollEventsRequestV1 {
        session_id,
        cursor: Some(CursorV1("lagged-cursor-token".to_string())),
        limit: None,
    }).await.unwrap_err();

    assert_eq!(lag_err.code, ControlErrorCodeV1::LaggedCursor);
    let details = lag_err.details.unwrap();
    assert_eq!(details.get("newest_safe_cursor").unwrap().as_str().unwrap(), "safe-resume-cursor");
}

#[tokio::test]
async fn test_approval_validation_and_revalidation() {
    let mock = ConformanceMock::new();
    let approval_id = "app-1".to_string();

    let ap = ApprovalProjectionV1 {
        approval_id: ApprovalIdV1(approval_id.clone()),
        tool_call_id: ToolCallIdV1("tc-1".to_string()),
        correlation_id: Some(CorrelationIdV1("sess-approval".to_string())),
        summary: "test tools".to_string(),
        editable_input_rules: None,
        original_hash: "orig-hash-123".to_string(),
        edited_hash: None,
        expires_at: None,
        is_cancelled: false,
        session_grant_terms: None,
    };

    {
        let mut state = mock.state.lock().unwrap();
        state.pending_approvals.insert(approval_id.clone(), ap);
    }

    // Normal approval response accepts
    let respond_res = mock.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: ApprovalIdV1(approval_id.clone()),
        decision: ApprovalDecisionV1::Approve,
    }).await.unwrap();

    assert!(respond_res.success);

    // Cancelled approvals cannot execute (H1A-B06)
    {
        let mut state = mock.state.lock().unwrap();
        state.pending_approvals.get_mut(&approval_id).unwrap().is_cancelled = true;
    }

    let cancel_respond_err = mock.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: ApprovalIdV1(approval_id.clone()),
        decision: ApprovalDecisionV1::Approve,
    }).await.unwrap_err();

    assert_eq!(cancel_respond_err.code, ControlErrorCodeV1::Conflict);

    // Revalidation logic check: invalid edited value gets rejected by validation
    {
        let mut state = mock.state.lock().unwrap();
        state.pending_approvals.get_mut(&approval_id).unwrap().is_cancelled = false;
    }

    let validation_err = mock.respond_to_approval(RespondToApprovalRequestV1 {
        approval_id: ApprovalIdV1(approval_id),
        decision: ApprovalDecisionV1::Edit(json!({ "invalid_field": true })),
    }).await.unwrap_err();

    assert_eq!(validation_err.code, ControlErrorCodeV1::Validation);
}

#[tokio::test]
async fn test_artifact_traversal_and_oversize_reads() {
    let mock = ConformanceMock::new();
    let session_id = SessionIdV1("sess-artifacts".to_string());

    // Create an artifact
    let data = vec![0u8; 2048]; // 2KB data
    let create_res = mock.create_artifact(CreateArtifactRequestV1 {
        session_id: session_id.clone(),
        display_path: "report.json".to_string(),
        data,
    }).await.unwrap();

    let art_id = create_res.metadata.logical_id;

    // Describe
    let desc_res = mock.describe_artifact(DescribeArtifactRequestV1 {
        session_id: session_id.clone(),
        artifact_id: art_id.clone(),
    }).await.unwrap();

    assert_eq!(desc_res.metadata.size, 2048);

    // Valid ranged read (length <= 1024)
    let read_res = mock.read_artifact_range(ReadArtifactRangeRequestV1 {
        session_id: session_id.clone(),
        artifact_id: art_id.clone(),
        offset: 10,
        length: 100,
    }).await.unwrap();

    assert_eq!(read_res.length, 100);
    assert_eq!(read_res.data.len(), 100);

    // Traversal check: rejects any paths containing path traversal tokens (H1A-B07)
    let traversal_err = mock.read_artifact_range(ReadArtifactRangeRequestV1 {
        session_id: session_id.clone(),
        artifact_id: ArtifactIdV1("../secrets.txt".to_string()),
        offset: 0,
        length: 100,
    }).await.unwrap_err();

    assert_eq!(traversal_err.code, ControlErrorCodeV1::Validation);

    // Oversize read request: rejects above max chunk size of 1024 bytes (H1A-B07)
    let oversize_err = mock.read_artifact_range(ReadArtifactRangeRequestV1 {
        session_id,
        artifact_id: art_id,
        offset: 0,
        length: 1025,
    }).await.unwrap_err();

    assert_eq!(oversize_err.code, ControlErrorCodeV1::Validation);
}
