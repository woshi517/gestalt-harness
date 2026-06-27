use crate::{
    read_prompt_snapshot, read_trace,
    run_manifest::{LifecycleState, RunManifest},
};
use gestalt_core::{
    context::{ContextProjectionState, SessionMessage, TokenBudget},
    snapshot::WorkspaceSnapshot,
    PromptSnapshot,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    CompletedHead,
    FailedWithCheckpoint,
    InterruptedSafe,
    InterruptedContextBuild,
    InterruptedProviderStream,
    InterruptedPolicyEval,
    InterruptedPendingApproval,
    InterruptedAmbiguousTool,
    InterruptedAmbiguousHook,
    WorkspaceDrift,
    IncompatibleTrace,
    UnfinalizedRunning,
    LegacyNonResumable,
    IncompatibleFingerprint,
}

#[derive(Debug, Clone)]
pub struct ResumeAnalysis {
    pub status: RecoveryStatus,
    pub session_id: String,
    pub run_id: String,
    pub history: Vec<SessionMessage>,
    pub context_state: ContextProjectionState,
    pub token_budget: TokenBudget,
    pub last_checkpoint_seq: Option<u64>,
    pub snapshot_hash: Option<String>,
    pub prompt_snapshot: Option<PromptSnapshot>,
    pub resolved_model: Option<gestalt_core::ResolvedModelSnapshot>,
}

impl ResumeAnalysis {
    pub fn is_safe_to_resume(&self) -> bool {
        matches!(
            self.status,
            RecoveryStatus::InterruptedSafe
                | RecoveryStatus::InterruptedContextBuild
                | RecoveryStatus::InterruptedProviderStream
                | RecoveryStatus::InterruptedPolicyEval
                | RecoveryStatus::InterruptedPendingApproval
        )
    }

    pub fn is_safe_to_continue(&self) -> bool {
        matches!(self.status, RecoveryStatus::CompletedHead)
    }
}

pub struct ResumeAnalyzer;

impl ResumeAnalyzer {
    pub fn analyze(
        run_dir: &Path,
        current_snapshot: Option<&WorkspaceSnapshot>,
        expected_fingerprint: Option<&crate::run_manifest::CompatibilityFingerprint>,
    ) -> ResumeAnalysis {
        let manifest_path = run_dir.join("run.json");
        let trace_path = run_dir.join("trace.jsonl");

        if !manifest_path.exists() {
            return ResumeAnalysis {
                status: RecoveryStatus::LegacyNonResumable,
                session_id: String::new(),
                run_id: String::new(),
                history: Vec::new(),
                context_state: ContextProjectionState::default(),
                token_budget: TokenBudget::default(),
                last_checkpoint_seq: None,
                snapshot_hash: None,
                prompt_snapshot: None,
                resolved_model: None,
            };
        }

        let manifest = match RunManifest::load_from(&manifest_path) {
            Ok(m) => m,
            Err(_) => {
                return ResumeAnalysis {
                    status: RecoveryStatus::IncompatibleTrace,
                    session_id: String::new(),
                    run_id: String::new(),
                    history: Vec::new(),
                    context_state: ContextProjectionState::default(),
                    token_budget: TokenBudget::default(),
                    last_checkpoint_seq: None,
                    snapshot_hash: None,
                    prompt_snapshot: None,
                    resolved_model: None,
                };
            }
        };

        if let Some(expected) = expected_fingerprint {
            if manifest.compatibility_fingerprint != *expected {
                return ResumeAnalysis {
                    status: RecoveryStatus::IncompatibleFingerprint,
                    session_id: manifest.session_id.clone(),
                    run_id: manifest.run_id.clone(),
                    history: Vec::new(),
                    context_state: ContextProjectionState::default(),
                    token_budget: TokenBudget::default(),
                    last_checkpoint_seq: None,
                    snapshot_hash: None,
                    prompt_snapshot: None,
                    resolved_model: manifest.resolved_model.clone(),
                };
            }
        }

        if !trace_path.exists() {
            return ResumeAnalysis {
                status: RecoveryStatus::IncompatibleTrace,
                session_id: manifest.session_id.clone(),
                run_id: manifest.run_id.clone(),
                history: Vec::new(),
                context_state: ContextProjectionState::default(),
                token_budget: TokenBudget::default(),
                last_checkpoint_seq: None,
                snapshot_hash: None,
                prompt_snapshot: None,
                resolved_model: manifest.resolved_model.clone(),
            };
        }

        let envelopes = match read_trace(&trace_path) {
            Ok(e) => e,
            Err(_) => {
                return ResumeAnalysis {
                    status: RecoveryStatus::IncompatibleTrace,
                    session_id: manifest.session_id.clone(),
                    run_id: manifest.run_id.clone(),
                    history: Vec::new(),
                    context_state: ContextProjectionState::default(),
                    token_budget: TokenBudget::default(),
                    last_checkpoint_seq: None,
                    snapshot_hash: None,
                    prompt_snapshot: None,
                    resolved_model: manifest.resolved_model.clone(),
                };
            }
        };

        let trace_has_prompt_snapshot_events = envelopes.iter().any(|env| {
            matches!(
                env.event,
                gestalt_core::AgentEvent::PromptSnapshotCreated { .. }
                    | gestalt_core::AgentEvent::PromptSnapshotLoaded { .. }
                    | gestalt_core::AgentEvent::PromptSnapshotReused { .. }
                    | gestalt_core::AgentEvent::PromptCachePlanGenerated { .. }
            )
        });

        if trace_has_prompt_snapshot_events
            && (manifest.prompt_snapshot_hash.is_none() || manifest.prompt_snapshot_path.is_none())
        {
            return ResumeAnalysis {
                status: RecoveryStatus::IncompatibleTrace,
                session_id: manifest.session_id.clone(),
                run_id: manifest.run_id.clone(),
                history: Vec::new(),
                context_state: ContextProjectionState::default(),
                token_budget: TokenBudget::default(),
                last_checkpoint_seq: None,
                snapshot_hash: None,
                prompt_snapshot: None,
                resolved_model: manifest.resolved_model.clone(),
            };
        }

        let prompt_snapshot = match (
            manifest.prompt_snapshot_hash.as_ref(),
            manifest.prompt_snapshot_path.as_ref(),
        ) {
            (Some(expected_hash), Some(relative_path)) => {
                let snapshot_path = run_dir.join(relative_path);
                let snapshot = match read_prompt_snapshot(&snapshot_path) {
                    Ok(snapshot) => snapshot,
                    Err(_) => {
                        return ResumeAnalysis {
                            status: RecoveryStatus::IncompatibleTrace,
                            session_id: manifest.session_id.clone(),
                            run_id: manifest.run_id.clone(),
                            history: Vec::new(),
                            context_state: ContextProjectionState::default(),
                            token_budget: TokenBudget::default(),
                            last_checkpoint_seq: None,
                            snapshot_hash: None,
                            prompt_snapshot: None,
                            resolved_model: manifest.resolved_model.clone(),
                        };
                    }
                };

                if snapshot.snapshot_hash != *expected_hash {
                    return ResumeAnalysis {
                        status: RecoveryStatus::IncompatibleTrace,
                        session_id: manifest.session_id.clone(),
                        run_id: manifest.run_id.clone(),
                        history: Vec::new(),
                        context_state: ContextProjectionState::default(),
                        token_budget: TokenBudget::default(),
                        last_checkpoint_seq: None,
                        snapshot_hash: None,
                        prompt_snapshot: None,
                        resolved_model: manifest.resolved_model.clone(),
                    };
                }

                Some(snapshot)
            }
            (None, None) => None,
            _ => {
                return ResumeAnalysis {
                    status: RecoveryStatus::IncompatibleTrace,
                    session_id: manifest.session_id.clone(),
                    run_id: manifest.run_id.clone(),
                    history: Vec::new(),
                    context_state: ContextProjectionState::default(),
                    token_budget: TokenBudget::default(),
                    last_checkpoint_seq: None,
                    snapshot_hash: None,
                    prompt_snapshot: None,
                    resolved_model: manifest.resolved_model.clone(),
                };
            }
        };

        let mut last_checkpoint = None;
        let mut last_checkpoint_index = None;
        for (i, env) in envelopes.iter().enumerate() {
            if matches!(env.event, gestalt_core::AgentEvent::Checkpoint { .. }) {
                last_checkpoint = Some(env);
                last_checkpoint_index = Some(i);
            }
        }

        let (history, context_state, token_budget) = match last_checkpoint {
            Some(env) => match &env.event {
                gestalt_core::AgentEvent::Checkpoint {
                    history,
                    context_state,
                    token_budget,
                    ..
                } => (
                    history.clone(),
                    ContextProjectionState::clone(context_state),
                    token_budget.clone(),
                ),
                _ => (
                    Vec::new(),
                    ContextProjectionState::default(),
                    TokenBudget::default(),
                ),
            },
            None => (
                Vec::new(),
                ContextProjectionState::default(),
                TokenBudget::default(),
            ),
        };

        if last_checkpoint.is_none() {
            return ResumeAnalysis {
                status: RecoveryStatus::LegacyNonResumable,
                session_id: manifest.session_id.clone(),
                run_id: manifest.run_id.clone(),
                history,
                context_state,
                token_budget,
                last_checkpoint_seq: None,
                snapshot_hash: None,
                prompt_snapshot: prompt_snapshot.clone(),
                resolved_model: manifest.resolved_model.clone(),
            };
        }

        // Check workspace snapshot hash
        let mut recorded_snapshot_hash = None;
        for env in &envelopes {
            if let Some(ref snapshot) = env.workspace_snapshot {
                recorded_snapshot_hash = Some(snapshot.content_hash.clone());
                break;
            } else if let Some(ref snapshot_id) = env.snapshot_id {
                recorded_snapshot_hash = Some(snapshot_id.clone());
                break;
            }
        }

        let has_drift =
            if let (Some(cur), Some(ref rec)) = (current_snapshot, &recorded_snapshot_hash) {
                if rec.len() == 12 {
                    !cur.content_hash.starts_with(rec)
                } else {
                    cur.content_hash != *rec
                }
            } else {
                false
            };

        if has_drift {
            return ResumeAnalysis {
                status: RecoveryStatus::WorkspaceDrift,
                session_id: manifest.session_id.clone(),
                run_id: manifest.run_id.clone(),
                history,
                context_state,
                token_budget,
                last_checkpoint_seq: last_checkpoint.map(|e| e.seq),
                snapshot_hash: recorded_snapshot_hash,
                prompt_snapshot: prompt_snapshot.clone(),
                resolved_model: manifest.resolved_model.clone(),
            };
        }

        // Check in-flight operations after last checkpoint
        let mut in_flight_context = false;
        let mut in_flight_provider = false;
        let mut in_flight_policy = false;
        let mut in_flight_approval = false;
        let mut in_flight_tools = std::collections::HashSet::new();
        let mut in_flight_hook = false;

        for env in &envelopes[last_checkpoint_index.unwrap() + 1..] {
            match &env.event {
                gestalt_core::AgentEvent::ContextBuildStarted => {
                    in_flight_context = true;
                }
                gestalt_core::AgentEvent::ContextBuilt { .. }
                | gestalt_core::AgentEvent::ContextBuildFailed { .. } => {
                    in_flight_context = false;
                }

                gestalt_core::AgentEvent::ModelResponseStarted { .. } => {
                    in_flight_provider = true;
                }
                gestalt_core::AgentEvent::ModelResponseStreamCompleted { .. }
                | gestalt_core::AgentEvent::ModelResponseStreamFailed { .. }
                | gestalt_core::AgentEvent::ModelResponseStreamInterrupted { .. } => {
                    in_flight_provider = false;
                }

                gestalt_core::AgentEvent::PolicyEvaluationStarted { .. } => {
                    in_flight_policy = true;
                }
                gestalt_core::AgentEvent::PolicyDecision { .. }
                | gestalt_core::AgentEvent::PolicyEvaluationFailed { .. }
                | gestalt_core::AgentEvent::PolicyEvaluationCancelled { .. } => {
                    in_flight_policy = false;
                }

                gestalt_core::AgentEvent::ApprovalRequested { .. } => {
                    in_flight_approval = true;
                }
                gestalt_core::AgentEvent::ApprovalDecision { .. }
                | gestalt_core::AgentEvent::ApprovalCancelled { .. } => {
                    in_flight_approval = false;
                }

                gestalt_core::AgentEvent::ToolExecutionStarted { id, .. } => {
                    in_flight_tools.insert(id.clone());
                }
                gestalt_core::AgentEvent::ToolResult { id, .. } => {
                    in_flight_tools.remove(id);
                }

                gestalt_core::AgentEvent::HookStarted { .. } => {
                    in_flight_hook = true;
                }
                gestalt_core::AgentEvent::HookCompleted { .. }
                | gestalt_core::AgentEvent::HookFailed { .. } => {
                    in_flight_hook = false;
                }
                _ => {}
            }
        }

        let status = if !in_flight_tools.is_empty() {
            RecoveryStatus::InterruptedAmbiguousTool
        } else if in_flight_hook {
            RecoveryStatus::InterruptedAmbiguousHook
        } else if in_flight_approval {
            RecoveryStatus::InterruptedPendingApproval
        } else if in_flight_policy {
            RecoveryStatus::InterruptedPolicyEval
        } else if in_flight_provider {
            RecoveryStatus::InterruptedProviderStream
        } else if in_flight_context {
            RecoveryStatus::InterruptedContextBuild
        } else {
            match manifest.lifecycle_state {
                LifecycleState::Completed => RecoveryStatus::CompletedHead,
                LifecycleState::Interrupted => RecoveryStatus::InterruptedSafe,
                LifecycleState::Failed => RecoveryStatus::FailedWithCheckpoint,
                LifecycleState::Running => RecoveryStatus::UnfinalizedRunning,
            }
        };

        ResumeAnalysis {
            status,
            session_id: manifest.session_id,
            run_id: manifest.run_id,
            history,
            context_state,
            token_budget,
            last_checkpoint_seq: last_checkpoint.map(|e| e.seq),
            snapshot_hash: recorded_snapshot_hash,
            prompt_snapshot,
            resolved_model: manifest.resolved_model,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_manifest::{
        CompatibilityFingerprint, LifecycleState, RunKind, RunManifest,
        PROMPT_SNAPSHOT_RELATIVE_PATH,
    };
    use crate::{write_prompt_snapshot, EventEnvelope};
    use gestalt_core::snapshot::WorkspaceSnapshot;
    use gestalt_core::{AgentEvent, Message, PromptSnapshot};
    use std::fs;
    use std::path::PathBuf;

    fn temp_run_dir() -> PathBuf {
        let temp =
            std::env::temp_dir().join(format!("gestalt-test-resume-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp).unwrap();
        temp
    }

    fn default_fingerprint() -> CompatibilityFingerprint {
        CompatibilityFingerprint {
            context_pipeline_version: "v1".to_string(),
            tool_schema_hash: "hash".to_string(),
            policy_fingerprint: "policy".to_string(),
            hook_contract_hash: "hook".to_string(),
            execution_mode: "Yolo".to_string(),
            skill_fingerprint: None,
            workspace_context_snapshot_hash: None,
        }
    }

    fn default_snapshot(content_hash: &str) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            workspace_root: PathBuf::from("."),
            git_sha: None,
            git_dirty: Some(false),
            untracked_count: None,
            content_hash: content_hash.to_string(),
            captured_at: chrono::Utc::now(),
        }
    }

    fn write_manifest(
        dir: &std::path::Path,
        lifecycle: LifecycleState,
        fp: CompatibilityFingerprint,
    ) {
        let manifest = RunManifest {
            v: 1,
            session_id: "session-1".to_string(),
            run_id: "run-1".to_string(),
            parent_run_id: None,
            base_checkpoint: None,
            run_kind: RunKind::New,
            created_at: chrono::Utc::now(),
            lifecycle_state: lifecycle,
            finalized_at: None,
            failure_kind: None,
            interrupted_phase: None,
            prompt_snapshot_hash: None,
            prompt_snapshot_path: None,
            resolved_model: None,
            compatibility_fingerprint: fp,
        };
        manifest.save_to(&dir.join("run.json")).unwrap();
    }

    fn write_manifest_with_snapshot(
        dir: &std::path::Path,
        lifecycle: LifecycleState,
        fp: CompatibilityFingerprint,
        snapshot_hash: String,
    ) {
        let manifest = RunManifest {
            v: 1,
            session_id: "session-1".to_string(),
            run_id: "run-1".to_string(),
            parent_run_id: None,
            base_checkpoint: None,
            run_kind: RunKind::New,
            created_at: chrono::Utc::now(),
            lifecycle_state: lifecycle,
            finalized_at: None,
            failure_kind: None,
            interrupted_phase: None,
            prompt_snapshot_hash: Some(snapshot_hash),
            prompt_snapshot_path: Some(PROMPT_SNAPSHOT_RELATIVE_PATH.to_string()),
            resolved_model: None,
            compatibility_fingerprint: fp,
        };
        manifest.save_to(&dir.join("run.json")).unwrap();
    }

    fn write_trace(dir: &std::path::Path, events: Vec<AgentEvent>) {
        let trace_path = dir.join("trace.jsonl");
        let mut file_content = String::new();
        let checkpoint_env = EventEnvelope {
            v: 1,
            session_id: "session-1".to_string(),
            run_id: "run-1".to_string(),
            turn_id: 1,
            seq: 1,
            ts: chrono::Utc::now(),
            event: AgentEvent::Checkpoint {
                history: Vec::new(),
                context_state: Box::new(ContextProjectionState::default()),
                token_budget: TokenBudget::default(),
                latest_projection_id: None,
                packet_hash: None,
                prompt_source: None,
            },
            redacted: false,
            workspace_snapshot: Some(default_snapshot("snapshot-hash-initial")),
            snapshot_id: None,
        };
        file_content.push_str(&serde_json::to_string(&checkpoint_env).unwrap());
        file_content.push('\n');

        for (i, ev) in events.into_iter().enumerate() {
            let env = EventEnvelope {
                v: 1,
                session_id: "session-1".to_string(),
                run_id: "run-1".to_string(),
                turn_id: 1,
                seq: (i + 2) as u64,
                ts: chrono::Utc::now(),
                event: ev,
                redacted: false,
                workspace_snapshot: None,
                snapshot_id: None,
            };
            file_content.push_str(&serde_json::to_string(&env).unwrap());
            file_content.push('\n');
        }
        fs::write(trace_path, file_content).unwrap();
    }

    #[test]
    fn test_legacy_non_resumable_missing_files() {
        let dir = temp_run_dir();
        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::LegacyNonResumable);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_invalid_manifest() {
        let dir = temp_run_dir();
        fs::write(dir.join("run.json"), "corrupt json").unwrap();
        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::IncompatibleTrace);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_workspace_drift() {
        let dir = temp_run_dir();
        write_manifest(&dir, LifecycleState::Interrupted, default_fingerprint());
        write_trace(&dir, vec![]);

        let current_snap = default_snapshot("snapshot-hash-drifted");

        let analysis = ResumeAnalyzer::analyze(&dir, Some(&current_snap), None);
        assert_eq!(analysis.status, RecoveryStatus::WorkspaceDrift);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_fingerprint_mismatch() {
        let dir = temp_run_dir();
        write_manifest(&dir, LifecycleState::Interrupted, default_fingerprint());
        write_trace(&dir, vec![]);

        let expected_fp = CompatibilityFingerprint {
            context_pipeline_version: "v2-different".to_string(),
            ..default_fingerprint()
        };

        let analysis = ResumeAnalyzer::analyze(&dir, None, Some(&expected_fp));
        assert_eq!(analysis.status, RecoveryStatus::IncompatibleFingerprint);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_skill_fingerprint_mismatch_rejects_resume() {
        // The original run had a skill fingerprint derived from manifest
        // content "hash-pdf-v1". The user edited the skill's SKILL.md, so
        // the current fingerprint is "hash-pdf-v2". Resume must be rejected
        // even though every other fingerprint field matches, because the
        // plan requires replay-safety against active-skill content drift.
        let dir = temp_run_dir();
        let mut original = default_fingerprint();
        original.skill_fingerprint = Some("hash-pdf-v1".to_string());
        write_manifest(&dir, LifecycleState::Interrupted, original);
        write_trace(&dir, vec![]);

        let mut expected = default_fingerprint();
        expected.skill_fingerprint = Some("hash-pdf-v2".to_string());

        let analysis = ResumeAnalyzer::analyze(&dir, None, Some(&expected));
        assert_eq!(analysis.status, RecoveryStatus::IncompatibleFingerprint);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_skill_fingerprint_match_allows_resume() {
        // Symmetric case: same active-skill manifest content on both sides
        // must allow resume. This guards against the fingerprint accidentally
        // being over-strict (e.g. including timestamps).
        let dir = temp_run_dir();
        let mut original = default_fingerprint();
        original.skill_fingerprint = Some("hash-pdf-v1".to_string());
        write_manifest(&dir, LifecycleState::Interrupted, original);
        write_trace(&dir, vec![]);

        let mut expected = default_fingerprint();
        expected.skill_fingerprint = Some("hash-pdf-v1".to_string());

        let analysis = ResumeAnalyzer::analyze(&dir, None, Some(&expected));
        assert_ne!(analysis.status, RecoveryStatus::IncompatibleFingerprint);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ambiguous_parallel_tools() {
        let dir = temp_run_dir();
        write_manifest(&dir, LifecycleState::Interrupted, default_fingerprint());

        write_trace(
            &dir,
            vec![
                AgentEvent::ToolExecutionStarted {
                    id: "tool-1".to_string(),
                    tool_name: "bash".to_string(),
                    input_hash: "hash".to_string(),
                    policy_source: "policy".to_string(),
                    working_dir: ".".to_string(),
                    parallel_group_id: Some("group-1".to_string()),
                    parallel_safe: true,
                },
                AgentEvent::ToolExecutionStarted {
                    id: "tool-2".to_string(),
                    tool_name: "bash".to_string(),
                    input_hash: "hash".to_string(),
                    policy_source: "policy".to_string(),
                    working_dir: ".".to_string(),
                    parallel_group_id: Some("group-1".to_string()),
                    parallel_safe: true,
                },
                AgentEvent::ToolResult {
                    id: "tool-1".to_string(),
                    output: "success".to_string(),
                    is_error: false,
                    truncated: false,
                    tool_name: None,
                    working_dir: None,
                    duration_ms: None,
                    output_hash: None,
                    artifact_refs: None,
                    policy_source: None,
                    failure: None,
                },
            ],
        );

        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::InterruptedAmbiguousTool);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ambiguous_hook() {
        let dir = temp_run_dir();
        write_manifest(&dir, LifecycleState::Interrupted, default_fingerprint());
        write_trace(
            &dir,
            vec![AgentEvent::HookStarted {
                hook_type: "session".to_string(),
                name: "on_session_end".to_string(),
            }],
        );

        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::InterruptedAmbiguousHook);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_interrupted_phases() {
        let dir = temp_run_dir();
        write_manifest(&dir, LifecycleState::Interrupted, default_fingerprint());

        // Test context build
        write_trace(&dir, vec![AgentEvent::ContextBuildStarted]);
        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::InterruptedContextBuild);

        // Test provider stream
        write_trace(
            &dir,
            vec![AgentEvent::ModelResponseStarted {
                provider_request_hash: "req".to_string(),
            }],
        );
        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::InterruptedProviderStream);

        // Test policy eval
        write_trace(
            &dir,
            vec![AgentEvent::PolicyEvaluationStarted {
                tool_call_id: "call".to_string(),
            }],
        );
        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::InterruptedPolicyEval);

        // Test approval pending
        write_trace(
            &dir,
            vec![AgentEvent::ApprovalRequested {
                tool_call_id: "call".to_string(),
                tool_name: "bash".to_string(),
                input: serde_json::Value::Null,
                risk: gestalt_core::tool::RiskLevel::High,
            }],
        );
        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::InterruptedPendingApproval);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resume_loads_prompt_snapshot() {
        let dir = temp_run_dir();
        let snapshot = PromptSnapshot::new(
            vec![Message::System {
                content: "stable prefix".to_string(),
            }],
            0,
        );
        write_manifest_with_snapshot(
            &dir,
            LifecycleState::Completed,
            default_fingerprint(),
            snapshot.snapshot_hash.clone(),
        );
        write_prompt_snapshot(dir.join(PROMPT_SNAPSHOT_RELATIVE_PATH), &snapshot).unwrap();
        write_trace(&dir, vec![]);

        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::CompletedHead);
        assert_eq!(analysis.prompt_snapshot, Some(snapshot));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resume_rejects_missing_prompt_snapshot_file() {
        let dir = temp_run_dir();
        write_manifest_with_snapshot(
            &dir,
            LifecycleState::Completed,
            default_fingerprint(),
            "missing-hash".to_string(),
        );
        write_trace(&dir, vec![]);

        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::IncompatibleTrace);
        assert!(analysis.prompt_snapshot.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resume_restores_projection_state_from_checkpoint() {
        let dir = temp_run_dir();
        write_manifest(&dir, LifecycleState::Interrupted, default_fingerprint());

        let checkpoint_env = EventEnvelope {
            v: 1,
            session_id: "session-1".to_string(),
            run_id: "run-1".to_string(),
            turn_id: 1,
            seq: 1,
            ts: chrono::Utc::now(),
            event: AgentEvent::Checkpoint {
                history: Vec::new(),
                context_state: Box::new(ContextProjectionState {
                    active_checkpoint: Some(gestalt_core::CompactionCheckpointRef {
                        checkpoint_id: "cp-1".to_string(),
                        source_range: gestalt_core::HistoryRange::new(0, 2),
                        source_hash: "range-hash".to_string(),
                        artifact: Some(gestalt_core::ArtifactRef {
                            run_id: "session-1".to_string(),
                            relative_path: "checkpoint_cp-1.json".to_string(),
                            content_hash: "range-hash".to_string(),
                        }),
                    }),
                    cleared_tool_results: std::collections::BTreeMap::from([(
                        "tool-1".to_string(),
                        gestalt_core::ClearedToolResultRef {
                            tool_use_id: "tool-1".to_string(),
                            message_id: gestalt_core::MessageId {
                                origin_session_id: "session-1".to_string(),
                                origin_message_namespace: "ns-1".to_string(),
                                sequence: 1,
                            },
                            output_hash: "output-hash".to_string(),
                            artifact: None,
                        },
                    )]),
                    prompt_snapshot: None,
                    context_epoch: 3,
                    policy_fingerprint: Some("policy-fp".to_string()),
                }),
                token_budget: TokenBudget::default(),
                latest_projection_id: Some("manifest-1".to_string()),
                packet_hash: None,
                prompt_source: None,
            },
            redacted: false,
            workspace_snapshot: Some(default_snapshot("snapshot-hash-initial")),
            snapshot_id: None,
        };

        fs::write(
            dir.join("trace.jsonl"),
            format!("{}\n", serde_json::to_string(&checkpoint_env).unwrap()),
        )
        .unwrap();

        let analysis = ResumeAnalyzer::analyze(&dir, None, None);
        assert_eq!(analysis.status, RecoveryStatus::InterruptedSafe);
        assert_eq!(analysis.context_state.context_epoch, 3);
        assert_eq!(
            analysis
                .context_state
                .active_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.checkpoint_id.as_str()),
            Some("cp-1")
        );
        assert!(analysis
            .context_state
            .cleared_tool_results
            .contains_key("tool-1"));

        let _ = fs::remove_dir_all(&dir);
    }
}
