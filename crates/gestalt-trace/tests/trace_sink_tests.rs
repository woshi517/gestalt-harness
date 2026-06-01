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
    let (sink, paths) = JsonlTraceSink::create_run(&dir, "session-1").expect("run paths");

    sink.emit(AgentEvent::ContextBuilt {
        packet_id: "session-1".to_string(),
        token_estimate: 42,
        packet_hash: Some("abcd1234efgh5678".to_string()),
        sources: None,
        omissions: None,
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
