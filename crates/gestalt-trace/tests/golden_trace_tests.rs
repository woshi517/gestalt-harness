use gestalt_core::event::AgentEvent;
use gestalt_trace::{EventEnvelope, GoldenTrace, GoldenTraceRunner};
use std::path::PathBuf;

fn get_trace_fixtures_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Manifest is in crates/gestalt-trace, tests is at workspace root
    path.pop(); // crates
    path.pop(); // root
    path.join("tests").join("fixtures").join("traces")
}

async fn run_and_assert_fixture(name: &str) {
    let dir = get_trace_fixtures_dir().join(name);
    let golden = GoldenTrace::load(&dir).expect(&format!("Failed to load {}", name));

    let (actual, actual_packet) = GoldenTraceRunner::run_golden(&golden)
        .await
        .expect(&format!("Failed to run {}", name));

    if std::env::var("UPDATE_GOLDEN_TRACES").is_ok() {
        // Write expected.jsonl
        let expected_path = dir.join("expected.jsonl");
        let mut file = std::fs::File::create(&expected_path).expect("create expected.jsonl");
        for env in &actual {
            let mut env = env.clone();
            env.ts = chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc);
            serde_json::to_writer(&mut file, &env).unwrap();
            use std::io::Write;
            file.write_all(b"\n").unwrap();
        }

        // Write context.json
        let context_path = dir.join("context.json");
        let context_file = std::fs::File::create(&context_path).expect("create context.json");
        serde_json::to_writer_pretty(context_file, &actual_packet).unwrap();
        return;
    }

    if let Err(err) = GoldenTraceRunner::assert_golden(&golden, &actual, &actual_packet) {
        panic!("Golden trace assert failed for {}: {}", name, err);
    }
}

#[tokio::test]
async fn test_confirm_bash_golden() {
    run_and_assert_fixture("confirm-bash-golden").await;
}

#[tokio::test]
async fn test_deny_read_secret_golden() {
    run_and_assert_fixture("deny-read-secret-golden").await;
}

#[tokio::test]
async fn test_yolo_bash_allowlist_golden() {
    run_and_assert_fixture("yolo-bash-allowlist-golden").await;
}

#[tokio::test]
async fn test_assert_golden_negative_cases() {
    let dir = get_trace_fixtures_dir().join("deny-read-secret-golden");
    let golden = GoldenTrace::load(&dir).expect("Failed to load deny-read-secret-golden");

    let (actual, actual_packet) = GoldenTraceRunner::run_golden(&golden)
        .await
        .expect("Failed to run golden deny-read-secret");

    // 1. ContextPacket mismatch
    {
        let mut bad_packet = actual_packet.clone();
        bad_packet.packet_hash = "different_hash".to_string();
        let res = GoldenTraceRunner::assert_golden(&golden, &actual, &bad_packet);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("ContextPacket mismatch"));
    }

    // 2. PolicyDecision mismatch (dropped decision / count mismatch)
    {
        let mut bad_actual = actual.clone();
        bad_actual.push(EventEnvelope {
            v: 1,
            session_id: "session-1".to_string(),
            run_id: "run-1".to_string(),
            turn_id: 1,
            seq: 999,
            ts: chrono::Utc::now(),
            event: AgentEvent::PolicyDecision {
                tool_call_id: "fake".to_string(),
                tool_name: Some("fake".to_string()),
                input_hash: Some("fake".to_string()),
                risk: Some(gestalt_core::tool::RiskLevel::Low),
                mode: Some(gestalt_core::session::ExecutionMode::Confirm),
                matched_rule: Some("fake".to_string()),
                decision: gestalt_core::event::PolicyStatus::Allowed,
                reason: Some("fake".to_string()),
                policy_source: "fake".to_string(),
            },
            redacted: false,
            workspace_snapshot: None,
            snapshot_id: None,
        });
        let res = GoldenTraceRunner::assert_golden(&golden, &bad_actual, &actual_packet);
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("Policy decision count mismatch"));
    }

    // 3. Event ordering/types mismatch (misordered events)
    {
        if actual.len() >= 2 {
            let mut bad_actual = actual.clone();
            let len = bad_actual.len();
            bad_actual.swap(0, len - 1);
            let res = GoldenTraceRunner::assert_golden(&golden, &bad_actual, &actual_packet);
            assert!(res.is_err());
            let err = res.unwrap_err();
            assert!(err.contains("Event type mismatch") || err.contains("Sequence ID mismatch"));
        }
    }

    // 4. Mismatching tool execution results
    {
        let mut bad_actual = actual.clone();
        let mut modified = false;
        for env in &mut bad_actual {
            if let AgentEvent::ToolResult {
                ref mut output_hash,
                ..
            } = env.event
            {
                *output_hash = Some("wrong_hash".to_string());
                modified = true;
                break;
            }
        }
        if modified {
            let res = GoldenTraceRunner::assert_golden(&golden, &bad_actual, &actual_packet);
            assert!(res.is_err());
            assert!(res.unwrap_err().contains("output_hash mismatch"));
        }
    }

    // 5. Corrupted Golden Expectation (corrupt the golden expected record)
    {
        let mut bad_golden = golden.clone();
        if !bad_golden.expected.is_empty() {
            // Alter the expected event type or payload in the golden structure
            bad_golden.expected[0].event = AgentEvent::UserMessage {
                content: "unexpected content".to_string(),
            };
            let res = GoldenTraceRunner::assert_golden(&bad_golden, &actual, &actual_packet);
            assert!(res.is_err());
        }
    }
}
