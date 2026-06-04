use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

use gestalt_core::{
    context::TokenBudget,
    error::HarnessError,
    event::{AgentEvent, VerificationStatus},
    hook::SessionHook,
    session::{ExecutionMode, Session, SessionConfig},
    snapshot::WorkspaceSnapshot,
    tool::ToolContext,
    trace::TraceSink,
};
use gestalt_trace::{
    evaluator::EvaluatorHook, EvalResult, EvalStatus, EventEnvelope, GoldenTrace, JsonlTraceSink,
    NoopTraceEvaluator, TraceEvaluator,
};

fn get_trace_fixtures_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // crates
    path.pop(); // root
    path.join("tests").join("fixtures").join("traces")
}

#[tokio::test]
async fn test_noop_trace_evaluator() {
    let dir = get_trace_fixtures_dir().join("yolo-bash-allowlist-golden");
    let golden = GoldenTrace::load(&dir).expect("Failed to load yolo-bash-allowlist-golden");

    let evaluator = NoopTraceEvaluator;
    let res = evaluator
        .evaluate(&golden.expected, &golden)
        .await
        .expect("Noop evaluator fails");

    assert_eq!(res.status, EvalStatus::Skipped);
    assert!(res.score.is_none());
    assert!(res.feedback.is_none());
}

struct MockTraceEvaluator {
    status: EvalStatus,
}

#[async_trait]
impl TraceEvaluator for MockTraceEvaluator {
    async fn evaluate(
        &self,
        _trace: &[EventEnvelope],
        _golden: &GoldenTrace,
    ) -> Result<EvalResult, HarnessError> {
        Ok(EvalResult {
            status: self.status,
            score: Some(0.95),
            feedback: Some("Excellent run".to_string()),
        })
    }
}

#[tokio::test]
async fn test_evaluator_hook_on_session_end() {
    let temp_dir = std::env::temp_dir().join(format!("gestalt-eval-hook-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let golden_dir = get_trace_fixtures_dir().join("yolo-bash-allowlist-golden");
    let golden = GoldenTrace::load(&golden_dir).expect("Failed to load golden");

    // Write a trace.jsonl file
    let trace_path = temp_dir.join("trace.jsonl");
    let sink = JsonlTraceSink::new("session-1", "run-1", &trace_path, None).expect("create sink");

    sink.emit(AgentEvent::UserMessage {
        content: "hello yolo".to_string(),
    })
    .unwrap();
    sink.flush().unwrap();

    let session = Session::new(
        "session-1",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock-provider".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns: 2,
        },
        TokenBudget {
            model_limit: 1000,
            reserved_output: 10,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 8,
        },
        ToolContext {
            working_dir: temp_dir.clone(),
            workspace_root: Some(temp_dir.clone()),
            timeout: std::time::Duration::from_secs(5),
            allow_network: false,
            environment: std::collections::HashMap::new(),
            max_output_bytes: 100,
            artifact_dir: Some(temp_dir.join("artifacts")),
            current_tool_call_id: None,
        },
        ExecutionMode::Yolo,
        WorkspaceSnapshot {
            workspace_root: temp_dir.clone(),
            git_sha: None,
            git_dirty: None,
            untracked_count: None,
            content_hash: "dummy".to_string(),
            captured_at: chrono::Utc::now(),
        },
    );

    let evaluator = Arc::new(MockTraceEvaluator {
        status: EvalStatus::Passed,
    });
    let hook = EvaluatorHook::new(evaluator, Some(golden)).with_flush_trigger(Arc::new(move || {}));

    let events = hook
        .on_session_end(&session)
        .await
        .expect("on_session_end fails");
    assert_eq!(events.len(), 1);

    if let AgentEvent::VerificationResult {
        status,
        checks,
        failed,
        report,
        ..
    } = &events[0]
    {
        assert_eq!(*status, VerificationStatus::Passed);
        assert_eq!(*checks, 1);
        assert_eq!(*failed, 0);
        assert_eq!(report.as_deref(), Some("Excellent run"));
    } else {
        panic!("Expected VerificationResult event");
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
}
