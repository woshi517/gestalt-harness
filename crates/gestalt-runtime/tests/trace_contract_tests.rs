use gestalt_core::context::HistoryRange;
use gestalt_runtime::context::projection::CompactionCheckpoint;
use gestalt_runtime::context::{ContextManagementPolicy, ProjectionManifest};
use gestalt_runtime::run_manifest::{LifecycleState, RunKind, RunManifest};
use gestalt_runtime::{
    load_checkpoint, load_manifest, read_trace, ClientEventRecordV1, TraceEvent,
    TRACE_EVENT_SCHEMA_VERSION,
};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn write_temp_file(name: &str, content: &str) -> std::path::PathBuf {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    fs::write(&path, content).expect("write temp file");
    std::mem::forget(dir);
    path
}

#[test]
fn read_trace_skips_unknown_event_kinds() {
    let path = write_temp_file(
        "trace.jsonl",
        r#"{"v":1,"session_id":"session-1","run_id":"run-1","turn_id":0,"seq":1,"ts":"2026-06-01T00:00:00Z","event":{"type":"user_message","content":"hello"},"redacted":false}
{"v":1,"session_id":"session-1","run_id":"run-1","turn_id":0,"seq":2,"ts":"2026-06-01T00:00:01Z","event":{"type":"future_kind","payload":"ignored"},"redacted":false}"#,
    );

    let trace = read_trace(&path).expect("read trace");
    assert_eq!(trace.len(), 1);
    assert!(matches!(trace[0].event, TraceEvent::UserMessage { .. }));
}

#[test]
fn read_trace_rejects_unsupported_version() {
    let path = write_temp_file(
        "trace.jsonl",
        r#"{"v":2,"session_id":"session-1","run_id":"run-1","turn_id":0,"seq":1,"ts":"2026-06-01T00:00:00Z","event":{"type":"user_message","content":"hello"},"redacted":false}"#,
    );

    let err = read_trace(&path).expect_err("unsupported version must fail");
    assert!(matches!(
        err,
        gestalt_core::TraceError::InvalidFormat { .. }
    ));
}

#[test]
fn run_manifest_reader_rejects_unsupported_version() {
    let path = write_temp_file(
        "run.json",
        &serde_json::to_string(&RunManifest {
            v: 2,
            session_id: "session-1".to_string(),
            run_id: "run-1".to_string(),
            parent_run_id: None,
            base_checkpoint: None,
            run_kind: RunKind::New,
            created_at: chrono::Utc::now(),
            lifecycle_state: LifecycleState::Running,
            finalized_at: None,
            failure_kind: None,
            interrupted_phase: None,
            prompt_snapshot_hash: None,
            prompt_snapshot_path: None,
            resolved_model: None,
            compatibility_fingerprint: gestalt_runtime::run_manifest::CompatibilityFingerprint {
                context_pipeline_version: "pipeline-v1".to_string(),
                tool_schema_hash: "tool".to_string(),
                policy_fingerprint: "policy".to_string(),
                hook_contract_hash: "hook".to_string(),
                execution_mode: "Yolo".to_string(),
                skill_fingerprint: None,
                workspace_context_snapshot_hash: None,
            },
        })
        .expect("serialize run manifest"),
    );

    let err = RunManifest::load_from(&path).expect_err("unsupported version must fail");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn context_artifact_readers_reject_unsupported_versions() {
    let manifest_dir = tempfile::tempdir().expect("tempdir");
    let manifest = ProjectionManifest {
        v: 2,
        manifest_id: "manifest-1".to_string(),
        session_id: "session-1".to_string(),
        run_id: "run-1".to_string(),
        turn_id: 0,
        timestamp: chrono::Utc::now(),
        policy: ContextManagementPolicy::default(),
        token_estimate: 0,
        stable_prefix_hash: None,
        checkpoint_ref: None,
        cleared_results: Vec::new(),
        omitted_messages: Vec::new(),
        messages_metadata: Vec::new(),
        retention_fingerprint: None,
        context_report_ref: None,
    };
    fs::write(
        manifest_dir
            .path()
            .join("projection_manifest_manifest-1.json"),
        serde_json::to_string(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
    let manifest_err = load_manifest("manifest-1", manifest_dir.path()).expect_err("must fail");
    assert!(matches!(
        manifest_err,
        gestalt_core::TraceError::ReadFailed { .. }
    ));

    let checkpoint_dir = tempfile::tempdir().expect("tempdir");
    let checkpoint = CompactionCheckpoint {
        v: 2,
        checkpoint_id: "checkpoint-1".to_string(),
        history_range: HistoryRange { start: 0, end: 1 },
        history_range_hash: "range-hash".to_string(),
        policy_version: "policy-v1".to_string(),
        compactor_model: "model".to_string(),
        prompt_hash: "prompt-hash".to_string(),
        created_at: chrono::Utc::now(),
        goal: "goal".to_string(),
        constraints: Vec::new(),
        completed_work: Vec::new(),
        in_progress_work: Vec::new(),
        blocked_items: Vec::new(),
        key_decisions: Vec::new(),
        next_steps: Vec::new(),
        critical_context: "context".to_string(),
        relevant_references: Vec::new(),
    };
    fs::write(
        checkpoint_dir.path().join("checkpoint_checkpoint-1.json"),
        serde_json::to_string(&checkpoint).expect("serialize checkpoint"),
    )
    .expect("write checkpoint");
    let checkpoint_err =
        load_checkpoint("checkpoint-1", checkpoint_dir.path()).expect_err("must fail");
    assert!(matches!(
        checkpoint_err,
        gestalt_core::TraceError::ReadFailed { .. }
    ));
}

#[test]
fn client_projection_omits_workspace_snapshot() {
    let envelope = gestalt_runtime::EventEnvelope {
        v: TRACE_EVENT_SCHEMA_VERSION,
        session_id: "session-1".to_string(),
        run_id: "run-1".to_string(),
        turn_id: 0,
        seq: 1,
        ts: chrono::Utc::now(),
        event: TraceEvent::UserMessage {
            content: "hello".to_string(),
        },
        redacted: false,
        workspace_snapshot: Some(gestalt_core::snapshot::WorkspaceSnapshot {
            workspace_root: PathBuf::from("."),
            git_sha: None,
            git_dirty: Some(false),
            untracked_count: None,
            content_hash: "snapshot-hash".to_string(),
            captured_at: chrono::Utc::now(),
        }),
        snapshot_id: Some("snapshot".to_string()),
    };

    let client = ClientEventRecordV1::from(&envelope);
    let value = serde_json::to_value(&client).expect("serialize client record");
    assert!(value.get("workspace_snapshot").is_none());
    assert!(value.get("snapshot_id").is_none());
    assert_eq!(value["v"], json!(1));
}
