use std::{fs, path::PathBuf};

use gestalt_core::{trace::TraceSink, AgentEvent};
use gestalt_trace::{read_trace, JsonlTraceSink};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gestalt-trace-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn jsonl_trace_sink_writes_monotonic_redacted_envelopes() {
    let dir = temp_dir("sink");
    let (sink, paths) = JsonlTraceSink::create_run(&dir, "session-1", None).expect("run paths");

    sink.emit(AgentEvent::ContextBuilt {
        packet_id: "session-1".to_string(),
        token_estimate: 42,
        packet_hash: Some("abcd1234efgh5678".to_string()),
        sources: None,
        omissions: None,
        prompt_source: None,
    })
    .expect("emit context built");
    sink.emit(AgentEvent::ModelRequest {
        provider: "openai".to_string(),
        model: "gpt-4o-mini".to_string(),
        packet_hash: None,
        temperature: None,
        max_tokens: None,
        provider_request_hash: None,
    })
    .expect("emit model request");
    sink.emit(AgentEvent::Text {
        delta: "token sk-test-secret".to_string(),
    })
    .expect("emit text");
    sink.emit(AgentEvent::PolicyDecision {
        tool_call_id: "call-1".to_string(),
        tool_name: Some("read".to_string()),
        input_hash: Some("abcdabcdabcdabcd".to_string()),
        risk: Some(gestalt_core::RiskLevel::Low),
        mode: Some(gestalt_core::ExecutionMode::Confirm),
        matched_rule: Some("paths.allow_read".to_string()),
        decision: gestalt_core::PolicyStatus::Allowed,
        reason: Some("safe".to_string()),
        policy_source: "paths.allow_read".to_string(),
    })
    .expect("emit policy");
    sink.flush().expect("flush");

    let events = read_trace(paths.trace).expect("trace readable");
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert_eq!(events[2].seq, 3);
    assert_eq!(events[3].seq, 4);
    assert_eq!(events[0].turn_id, 1);
    assert_eq!(events[1].turn_id, 1);
    assert_eq!(events[2].turn_id, 1);
    assert!(events[2].redacted);
    assert!(
        matches!(&events[3].event, AgentEvent::PolicyDecision { policy_source, .. } if policy_source == "paths.allow_read")
    );
}

#[test]
fn test_trace_sink_propagates_snapshot() {
    use gestalt_core::snapshot::WorkspaceSnapshot;
    use chrono::Utc;

    let dir = temp_dir("snapshot-prop");
    let snapshot1 = WorkspaceSnapshot {
        workspace_root: PathBuf::from("/mock/root"),
        git_sha: Some("abcdef123456".to_string()),
        git_dirty: Some(false),
        untracked_count: Some(0),
        content_hash: "1111111111111111111111111111111111111111111111111111111111111111".to_string(),
        captured_at: Utc::now(),
    };

    let (sink, paths) = JsonlTraceSink::create_run(&dir, "session-1", Some(snapshot1.clone())).expect("run paths");

    sink.emit(AgentEvent::UserMessage {
        content: "hello".to_string(),
    })
    .expect("emit message 1");

    let snapshot2 = WorkspaceSnapshot {
        workspace_root: PathBuf::from("/mock/root"),
        git_sha: Some("abcdef123456".to_string()),
        git_dirty: Some(true),
        untracked_count: Some(2),
        content_hash: "2222222222222222222222222222222222222222222222222222222222222222".to_string(),
        captured_at: Utc::now(),
    };
    sink.update_snapshot(snapshot2.clone());

    sink.emit(AgentEvent::UserMessage {
        content: "world".to_string(),
    })
    .expect("emit message 2");

    sink.flush().expect("flush");

    let envelopes = read_trace(paths.trace).expect("read trace");
    assert_eq!(envelopes.len(), 2);

    assert_eq!(envelopes[0].workspace_snapshot, Some(snapshot1));
    assert_eq!(envelopes[0].snapshot_id, Some("111111111111".to_string()));

    assert_eq!(envelopes[1].workspace_snapshot, Some(snapshot2));
    assert_eq!(envelopes[1].snapshot_id, Some("222222222222".to_string()));
}

#[test]
fn test_summary_includes_snapshot_id() {
    use gestalt_core::session::RunResult;
    use gestalt_core::event::StopReason;
    use gestalt_trace::write_summary;

    let dir = temp_dir("summary-test");
    let summary_path = dir.join("summary.md");

    let result = RunResult {
        session_id: "session-abc".to_string(),
        turns: 3,
        stop_reason: StopReason::EndTurn,
        total_input_tokens: 100,
        total_output_tokens: 50,
        artifacts: vec![],
        workspace_snapshot_id: Some("123456789012".to_string()),
    };

    write_summary(&summary_path, &result).expect("write summary");
    let content = fs::read_to_string(summary_path).expect("read summary");
    assert!(content.contains("- Workspace snapshot: 123456789012"));
}

