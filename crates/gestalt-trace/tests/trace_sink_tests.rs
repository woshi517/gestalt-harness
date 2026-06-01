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

    sink.emit(AgentEvent::ModelRequest {
        provider: "openai".to_string(),
        model: "gpt-4o-mini".to_string(),
    })
    .expect("emit model request");
    sink.emit(AgentEvent::Text {
        delta: "token sk-test-secret".to_string(),
    })
    .expect("emit text");
    sink.emit(AgentEvent::PolicyDecision {
        tool_call_id: "call-1".to_string(),
        decision: gestalt_core::PolicyStatus::Allowed,
        reason: Some("safe".to_string()),
        policy_source: "paths.allow_read".to_string(),
    })
    .expect("emit policy");
    sink.flush().expect("flush");

    let events = read_trace(paths.trace).expect("trace readable");
    assert_eq!(events[0].seq, 1);
    assert_eq!(events[1].seq, 2);
    assert!(events[1].redacted);
    assert!(
        matches!(&events[2].event, AgentEvent::PolicyDecision { policy_source, .. } if policy_source == "paths.allow_read")
    );
}
