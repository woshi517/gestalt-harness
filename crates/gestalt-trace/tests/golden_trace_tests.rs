use gestalt_core::event::AgentEvent;
use gestalt_core::{
    AgentLoop, CancelToken, ExecutionMode, StopReason, Session, ToolError,
    message::Message,
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    context::{ContextPipeline, TokenBudget},
    tool::{RiskLevel, Tool, ToolCatalog, ToolContext, ToolOutput},
    policy::{PolicyEngine, PolicyRequest},
    session_queue::{MessageSource, QueueAck, QueueLifecycle, SteeringQueue},
    snapshot::WorkspaceSnapshot,
};
use gestalt_trace::{EventEnvelope, GoldenTrace, GoldenTraceRunner};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
async fn test_tool_calling_valid_read() {
    run_and_assert_fixture("tool-calling/valid_read").await;
}

#[tokio::test]
async fn test_tool_calling_invalid_input_repair() {
    run_and_assert_fixture("tool-calling/invalid_input_repair").await;
}

#[tokio::test]
async fn test_tool_calling_parallel_reads() {
    run_and_assert_fixture("tool-calling/parallel_reads").await;
}

#[tokio::test]
async fn test_tool_calling_write_requires_approval() {
    run_and_assert_fixture("tool-calling/write_requires_approval").await;
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

// Programmatic Steering Golden Tests

struct ProgrammaticMockProvider {
    mock_responses: Mutex<std::collections::VecDeque<Vec<AgentEvent>>>,
}

#[async_trait::async_trait]
impl Provider for ProgrammaticMockProvider {
    fn id(&self) -> &str { "mock" }
    fn display_name(&self) -> &str { "Mock" }
    fn default_model(&self) -> &str { "mock-model" }
    fn capabilities(&self) -> &ProviderCapabilities {
        static CAP: ProviderCapabilities = ProviderCapabilities {
            supports_tools: true,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: false,
            supports_prompt_caching: false,
            supports_usage_reporting: false,
            supports_streaming: false,
            supports_strict_schema: false,
        };
        &CAP
    }
    fn model_info(&self, _model: &str) -> Option<gestalt_core::model::ModelInfo> { None }
    fn count_tokens(&self, _model: &str, _messages: &[Message]) -> Result<usize, gestalt_core::error::HarnessError> { Ok(0) }
    async fn stream(&self, _request: ProviderRequest) -> Result<EventStream, gestalt_core::error::HarnessError> {
        let response = self.mock_responses.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![AgentEvent::Stop { reason: StopReason::EndTurn }]
        });
        let stream = futures::stream::iter(response.into_iter().map(Ok::<_, gestalt_core::error::HarnessError>));
        Ok(Box::pin(stream))
    }
}

struct ProgrammaticMockContextPipeline;
impl ContextPipeline for ProgrammaticMockContextPipeline {
    fn process(&self, history: &[Message], _budget: &TokenBudget) -> Vec<Message> {
        history.to_vec()
    }
    fn version(&self) -> &str { "mock" }
}

struct ProgrammaticMockToolCatalog {
    tools: std::collections::HashMap<String, Arc<dyn Tool>>,
}
impl ToolCatalog for ProgrammaticMockToolCatalog {
    fn schemas(&self) -> Vec<serde_json::Value> {
        self.tools.values().map(|t| t.schema()).collect()
    }
    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
}

struct ProgrammaticMockPolicyEngine {
    eval_count: Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait::async_trait]
impl PolicyEngine for ProgrammaticMockPolicyEngine {
    async fn evaluate(&self, _req: PolicyRequest) -> gestalt_core::policy::PolicyDecision {
        self.eval_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        gestalt_core::policy::PolicyDecision {
            status: gestalt_core::event::PolicyStatus::Allowed,
            reason: None,
            policy_source: "mock".to_string(),
        }
    }
}

struct ProgrammaticTestSteeringQueue {
    messages: Mutex<Vec<gestalt_core::session_queue::QueuedSessionMessage>>,
}

#[async_trait::async_trait]
impl SteeringQueue for ProgrammaticTestSteeringQueue {
    async fn enqueue(&self, message: gestalt_core::session_queue::QueuedSessionMessage) -> Result<QueueAck, gestalt_core::error::HarnessError> {
        self.messages.lock().unwrap().push(message);
        Ok(QueueAck::Queued)
    }
    async fn drain(&self) -> Result<Vec<gestalt_core::session_queue::QueuedSessionMessage>, gestalt_core::error::HarnessError> {
        let mut guard = self.messages.lock().unwrap();
        Ok(std::mem::take(&mut *guard))
    }
    async fn update_lifecycle(&self, _state: QueueLifecycle) -> Result<(), gestalt_core::error::HarnessError> { Ok(()) }
    async fn len(&self) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(self.messages.lock().unwrap().len())
    }
}

struct ProgrammaticDummyTool;
#[async_trait::async_trait]
impl Tool for ProgrammaticDummyTool {
    fn name(&self) -> &str { "dummy" }
    fn description(&self) -> &str { "dummy" }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "name": "dummy",
            "description": "dummy",
            "input_schema": {
                "type": "object",
                "properties": {}
            }
        })
    }
    fn risk(&self, _input: &serde_json::Value) -> RiskLevel { RiskLevel::Low }
    async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Text { content: "ok".to_string() })
    }
}

fn make_programmatic_session(max_turns: usize) -> Session {
    Session::new(
        "test-session",
        gestalt_core::session::SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 100,
            temperature: None,
            max_turns,
        },
        gestalt_core::context::TokenBudget {
            model_limit: 1000,
            reserved_output: 10,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 1,
        },
        gestalt_core::tool::ToolContext {
            working_dir: std::path::PathBuf::from("/"),
            workspace_root: None,
            timeout: std::time::Duration::from_secs(1),
            allow_network: false,
            environment: std::collections::HashMap::new(),
            max_output_bytes: 1000,
            artifact_dir: None,
            current_tool_call_id: None,
        },
        ExecutionMode::Yolo,
        WorkspaceSnapshot {
            workspace_root: std::path::PathBuf::from("/"),
            git_sha: None,
            git_dirty: Some(false),
            untracked_count: None,
            content_hash: "hash".to_string(),
            captured_at: chrono::Utc::now(),
        },
    )
}

#[tokio::test]
async fn test_steering_before_model_request_golden_order() {
    let queue = Arc::new(ProgrammaticTestSteeringQueue { messages: Mutex::new(vec![]) });
    let provider = Arc::new(ProgrammaticMockProvider {
        mock_responses: Mutex::new(std::collections::VecDeque::from(vec![
            vec![AgentEvent::Stop { reason: StopReason::EndTurn }]
        ])),
    });
    let loop_ = AgentLoop::new(
        provider.clone(),
        Arc::new(ProgrammaticMockToolCatalog { tools: std::collections::HashMap::new() }),
        Arc::new(ProgrammaticMockContextPipeline),
        Arc::new(ProgrammaticMockPolicyEngine { eval_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)) }),
        Arc::new(gestalt_core::approval::AutoApprovalProvider),
        1,
    ).with_steering_queue(queue.clone());

    let mut session = make_programmatic_session(1);
    let cancel = CancelToken::new();

    // Enqueue steering message
    queue.enqueue(gestalt_core::session_queue::QueuedSessionMessage {
        id: "msg-1".to_string(),
        content: "Steer content".to_string(),
        source: MessageSource::Operator,
        idempotency_key: None,
        injected_at_turn: None,
    }).await.unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    loop_.run(&mut session, &cancel, None, move |ev| {
        events_clone.lock().unwrap().push(ev);
    }).await.unwrap();

    let events_guard = events.lock().unwrap();

    // Find indices of interesting events
    let mut injected_idx = None;
    let mut checkpoint_idx = None;
    let mut build_started_idx = None;
    let mut context_built_idx = None;
    let mut model_request_idx = None;

    for (idx, ev) in events_guard.iter().enumerate() {
        match ev {
            AgentEvent::SessionMessageInjected { .. } => injected_idx = Some(idx),
            AgentEvent::Checkpoint { .. } => {
                // We want the checkpoint that happens right after injection, before context build started
                if build_started_idx.is_none() && injected_idx.is_some() {
                    checkpoint_idx = Some(idx);
                }
            }
            AgentEvent::ContextBuildStarted => build_started_idx = Some(idx),
            AgentEvent::ContextBuilt { .. } => context_built_idx = Some(idx),
            AgentEvent::ModelRequest { .. } => model_request_idx = Some(idx),
            _ => {}
        }
    }

    assert!(injected_idx.is_some());
    assert!(checkpoint_idx.is_some());
    assert!(build_started_idx.is_some());
    assert!(context_built_idx.is_some());
    assert!(model_request_idx.is_some());

    let inj = injected_idx.unwrap();
    let cp = checkpoint_idx.unwrap();
    let bs = build_started_idx.unwrap();
    let cb = context_built_idx.unwrap();
    let mr = model_request_idx.unwrap();

    // Assert exact event ordering: Injection -> Checkpoint -> ContextBuildStarted -> ContextBuilt -> ModelRequest
    assert!(inj < cp);
    assert!(cp < bs);
    assert!(bs < cb);
    assert!(cb < mr);
}

#[tokio::test]
async fn test_multiple_steering_messages_same_turn() {
    let queue = Arc::new(ProgrammaticTestSteeringQueue { messages: Mutex::new(vec![]) });
    let provider = Arc::new(ProgrammaticMockProvider {
        mock_responses: Mutex::new(std::collections::VecDeque::from(vec![
            vec![AgentEvent::Stop { reason: StopReason::EndTurn }]
        ])),
    });
    let loop_ = AgentLoop::new(
        provider.clone(),
        Arc::new(ProgrammaticMockToolCatalog { tools: std::collections::HashMap::new() }),
        Arc::new(ProgrammaticMockContextPipeline),
        Arc::new(ProgrammaticMockPolicyEngine { eval_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)) }),
        Arc::new(gestalt_core::approval::AutoApprovalProvider),
        1,
    ).with_steering_queue(queue.clone());

    let mut session = make_programmatic_session(1);
    let cancel = CancelToken::new();

    // Enqueue steering messages
    queue.enqueue(gestalt_core::session_queue::QueuedSessionMessage {
        id: "msg-1".to_string(),
        content: "First message".to_string(),
        source: MessageSource::Operator,
        idempotency_key: None,
        injected_at_turn: None,
    }).await.unwrap();

    queue.enqueue(gestalt_core::session_queue::QueuedSessionMessage {
        id: "msg-2".to_string(),
        content: "Second message".to_string(),
        source: MessageSource::FollowUp,
        idempotency_key: None,
        injected_at_turn: None,
    }).await.unwrap();

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    loop_.run(&mut session, &cancel, None, move |ev| {
        events_clone.lock().unwrap().push(ev);
    }).await.unwrap();

    let events_guard = events.lock().unwrap();

    let mut injected_msgs = Vec::new();
    for ev in events_guard.iter() {
        if let AgentEvent::SessionMessageInjected { message } = ev {
            injected_msgs.push(message.clone());
        }
    }

    assert_eq!(injected_msgs.len(), 2);
    assert_eq!(injected_msgs[0].id, "msg-1");
    assert_eq!(injected_msgs[1].id, "msg-2");
}

#[tokio::test]
async fn test_operator_correction_before_tool_use_followup() {
    let queue = Arc::new(ProgrammaticTestSteeringQueue { messages: Mutex::new(vec![]) });
    let provider = Arc::new(ProgrammaticMockProvider {
        mock_responses: Mutex::new(std::collections::VecDeque::from(vec![
            // Turn 0: Model proposes a tool call
            vec![
                AgentEvent::ToolCallStreamed {
                    id: "call-1".to_string(),
                    name: "dummy".to_string(),
                    input_delta: "{}".to_string(),
                },
                AgentEvent::Stop { reason: StopReason::ToolUse },
            ],
            // Turn 1: Model ends turn
            vec![
                AgentEvent::Stop { reason: StopReason::EndTurn },
            ],
        ])),
    });

    let eval_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut tools = std::collections::HashMap::new();
    tools.insert("dummy".to_string(), Arc::new(ProgrammaticDummyTool) as Arc<dyn Tool>);

    let loop_ = AgentLoop::new(
        provider.clone(),
        Arc::new(ProgrammaticMockToolCatalog { tools }),
        Arc::new(ProgrammaticMockContextPipeline),
        Arc::new(ProgrammaticMockPolicyEngine { eval_count: eval_count.clone() }),
        Arc::new(gestalt_core::approval::AutoApprovalProvider),
        2,
    ).with_steering_queue(queue.clone());

    let mut session = make_programmatic_session(2);
    let cancel = CancelToken::new();

    // We want to enqueue a message dynamically before Turn 1 (follow-up turn).
    // How can we do this?
    // We can use a trace hook or we can simply enqueue the message when the first turn completes.
    // Wait, the steering queue is drained once per turn, *before* build_request.
    // So if we enqueue it before the entire loop starts, since there are 2 turns,
    // wait: if we enqueue it at the start, it will be drained on Turn 0.
    // Can we enqueue it in the middle of execution?
    // Yes! We can implement a trace hook in `HookRegistry` that gets called, and when it sees `ToolResult` for call-1, it enqueues the operator correction!
    // This is incredibly elegant and mirrors a real operator correction during/after tool use!

    struct TriggerSteeringHook {
        queue: Arc<ProgrammaticTestSteeringQueue>,
    }
    impl gestalt_core::hook::TraceHook for TriggerSteeringHook {
        fn on_trace_write(&self, event: &AgentEvent) -> std::result::Result<(), gestalt_core::TraceError> {
            if let AgentEvent::ToolResult { id, .. } = event {
                if id == "call-1" {
                    // Enqueue operator correction
                    self.queue.messages.lock().unwrap().push(gestalt_core::session_queue::QueuedSessionMessage {
                        id: "operator-correction".to_string(),
                        content: "Corrected instruction".to_string(),
                        source: MessageSource::Operator,
                        idempotency_key: None,
                        injected_at_turn: None,
                    });
                }
            }
            Ok(())
        }
    }

    let mut hooks = gestalt_core::HookRegistry::new();
    hooks.register_trace_hook(Arc::new(TriggerSteeringHook { queue: queue.clone() }));

    let loop_ = loop_.with_hooks(hooks);

    let events = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    loop_.run(&mut session, &cancel, None, move |ev| {
        events_clone.lock().unwrap().push(ev);
    }).await.unwrap();

    // Let's assert that the correction was injected in Turn 1
    let events_guard = events.lock().unwrap();

    let mut correction_injected = false;
    for ev in events_guard.iter() {
        if let AgentEvent::SessionMessageInjected { message } = ev {
            if message.id == "operator-correction" {
                assert_eq!(message.injected_at_turn, Some(1)); // Injected in Turn 1
                correction_injected = true;
            }
        }
    }

    assert!(correction_injected, "Operator correction should have been injected");
    // Assert that the policy evaluator was evaluated for the tool call in Turn 0
    assert_eq!(eval_count.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_persisted_steering_replay_and_resume() {
    use gestalt_trace::{
        JsonlTraceSink, resume::ResumeAnalyzer,
        run_manifest::{CompatibilityFingerprint, LifecycleState, RunKind, RunManifest},
    };
    use gestalt_core::trace::TraceSink;
    use std::fs;

    // Create temporary run directory
    let temp_root = std::env::temp_dir().join(format!(
        "gestalt-test-persisted-steer-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&temp_root).unwrap();

    let session_id = "steered-persisted-session";
    let run_id = "run-steered-1";

    let (sink, paths) = JsonlTraceSink::create_run(&temp_root, session_id, run_id, None).unwrap();
    let sink = Arc::new(sink);

    let queue = Arc::new(ProgrammaticTestSteeringQueue {
        messages: Mutex::new(vec![]),
    });
    let provider = Arc::new(ProgrammaticMockProvider {
        mock_responses: Mutex::new(std::collections::VecDeque::from(vec![vec![
            AgentEvent::Stop {
                reason: StopReason::EndTurn,
            },
        ]])),
    });

    let loop_ = AgentLoop::new(
        provider.clone(),
        Arc::new(ProgrammaticMockToolCatalog {
            tools: std::collections::HashMap::new(),
        }),
        Arc::new(ProgrammaticMockContextPipeline),
        Arc::new(ProgrammaticMockPolicyEngine {
            eval_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }),
        Arc::new(gestalt_core::approval::AutoApprovalProvider),
        1,
    )
    .with_steering_queue(queue.clone());

    let mut session = make_programmatic_session(1);
    let cancel = CancelToken::new();

    // Enqueue steering message
    queue
        .enqueue(gestalt_core::session_queue::QueuedSessionMessage {
            id: "msg-steered-123".to_string(),
            content: "Steered message content".to_string(),
            source: MessageSource::Operator,
            idempotency_key: None,
            injected_at_turn: None,
        })
        .await
        .unwrap();

    // Run the loop and output to the trace sink
    let sink_clone = sink.clone();
    loop_
        .run(&mut session, &cancel, None, move |ev| {
            sink_clone.emit(ev).unwrap();
        })
        .await
        .unwrap();

    sink.flush().unwrap();
    drop(sink); // Close the trace file

    // Save a completed run manifest
    let fp = CompatibilityFingerprint {
        context_pipeline_version: "mock".to_string(),
        tool_schema_hash: "mock".to_string(),
        policy_fingerprint: "mock".to_string(),
        hook_contract_hash: "mock".to_string(),
        execution_mode: "Yolo".to_string(),
        skill_fingerprint: None,
    };
    let manifest = RunManifest {
        v: 1,
        session_id: session_id.to_string(),
        run_id: run_id.to_string(),
        parent_run_id: None,
        base_checkpoint: None,
        run_kind: RunKind::New,
        created_at: chrono::Utc::now(),
        lifecycle_state: LifecycleState::Completed,
        finalized_at: Some(chrono::Utc::now()),
        failure_kind: None,
        interrupted_phase: None,
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        compatibility_fingerprint: fp,
    };
    manifest.save_to(&paths.root.join("run.json")).unwrap();

    // Now analyze the run directory using ResumeAnalyzer
    let analysis = ResumeAnalyzer::analyze(&paths.root, None, None);
    assert_eq!(analysis.session_id, session_id);
    assert_eq!(analysis.run_id, run_id);
    assert!(analysis.history.len() >= 1);
    let last_msg = &analysis.history[0];
    match last_msg {
        Message::User { content } => match &content[0] {
            gestalt_core::message::ContentBlock::Text { text } => {
                assert_eq!(text, "Steered message content");
            }
            _ => panic!("Expected text content block"),
        },
        _ => panic!("Expected user message in history"),
    }

    // Cleanup
    fs::remove_dir_all(&temp_root).unwrap();
}

#[tokio::test]
async fn test_persisted_steering_resume_flow() {
    use gestalt_trace::{
        JsonlTraceSink, resume::ResumeAnalyzer,
        run_manifest::{CompatibilityFingerprint, LifecycleState, RunKind, RunManifest},
    };
    use gestalt_core::trace::TraceSink;
    use std::fs;

    let temp_root = std::env::temp_dir().join(format!(
        "gestalt-test-resume-flow-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&temp_root).unwrap();

    let session_id = "steered-resume-session";
    let run_id = "run-steered-interrupted";

    let (sink, paths) = JsonlTraceSink::create_run(&temp_root, session_id, run_id, None).unwrap();
    let sink = Arc::new(sink);

    let queue = Arc::new(ProgrammaticTestSteeringQueue {
        messages: Mutex::new(vec![]),
    });
    
    let cancel = CancelToken::new();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CancelOnStreamProvider {
        cancel_token: cancel.clone(),
        requests: requests.clone(),
    });

    let loop_ = AgentLoop::new(
        provider.clone(),
        Arc::new(ProgrammaticMockToolCatalog {
            tools: std::collections::HashMap::new(),
        }),
        Arc::new(ProgrammaticMockContextPipeline),
        Arc::new(ProgrammaticMockPolicyEngine {
            eval_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }),
        Arc::new(gestalt_core::approval::AutoApprovalProvider),
        1,
    )
    .with_steering_queue(queue.clone());

    let mut session = make_programmatic_session(1);

    // Enqueue steering message
    queue
        .enqueue(gestalt_core::session_queue::QueuedSessionMessage {
            id: "msg-steered-456".to_string(),
            content: "Steered message for resume".to_string(),
            source: MessageSource::Operator,
            idempotency_key: None,
            injected_at_turn: None,
        })
        .await
        .unwrap();

    // Run the loop, it will cancel/interrupt during the stream method of provider
    let sink_clone = sink.clone();
    let run_res = loop_
        .run(&mut session, &cancel, None, move |ev| {
            sink_clone.emit(ev).unwrap();
        })
        .await;

    assert!(run_res.is_err()); // should return Err(Cancelled)

    sink.flush().unwrap();
    drop(sink);

    // Save an interrupted run manifest
    let fp = CompatibilityFingerprint {
        context_pipeline_version: "mock".to_string(),
        tool_schema_hash: "mock".to_string(),
        policy_fingerprint: "mock".to_string(),
        hook_contract_hash: "mock".to_string(),
        execution_mode: "Yolo".to_string(),
        skill_fingerprint: None,
    };
    let manifest = RunManifest {
        v: 1,
        session_id: session_id.to_string(),
        run_id: run_id.to_string(),
        parent_run_id: None,
        base_checkpoint: None,
        run_kind: RunKind::New,
        created_at: chrono::Utc::now(),
        lifecycle_state: LifecycleState::Interrupted,
        finalized_at: None,
        failure_kind: None,
        interrupted_phase: Some("provider_stream".to_string()),
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        compatibility_fingerprint: fp,
    };
    manifest.save_to(&paths.root.join("run.json")).unwrap();

    // Now analyze using ResumeAnalyzer
    let analysis = ResumeAnalyzer::analyze(&paths.root, None, None);
    assert_eq!(analysis.status, gestalt_trace::resume::RecoveryStatus::InterruptedSafe);
    assert!(analysis.history.len() >= 1);

    // Reconstruct and run a resumed session
    let resume_requests = Arc::new(Mutex::new(Vec::new()));
    let resume_provider = Arc::new(AssertResumeProvider {
        requests: resume_requests.clone(),
    });
    
    let resume_loop = AgentLoop::new(
        resume_provider,
        Arc::new(ProgrammaticMockToolCatalog {
            tools: std::collections::HashMap::new(),
        }),
        Arc::new(ProgrammaticMockContextPipeline),
        Arc::new(ProgrammaticMockPolicyEngine {
            eval_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }),
        Arc::new(gestalt_core::approval::AutoApprovalProvider),
        1,
    );

    let mut resume_session = make_programmatic_session(1);
    resume_session.history = analysis.history;

    let resume_cancel = CancelToken::new();
    let resume_res = resume_loop
        .run(&mut resume_session, &resume_cancel, None, |_ev| {})
        .await;

    assert!(resume_res.is_ok());

    // Verify the resume provider request contained the steered message exactly once in its history
    let resume_reqs = resume_requests.lock().unwrap().clone();
    assert_eq!(resume_reqs.len(), 1);
    let history = &resume_reqs[0].messages;
    
    // History should have exactly one message: the steered message
    assert_eq!(history.len(), 1);
    match &history[0] {
        Message::User { content } => match &content[0] {
            gestalt_core::message::ContentBlock::Text { text } => {
                assert_eq!(text, "Steered message for resume");
            }
            _ => panic!("Expected text block"),
        },
        _ => panic!("Expected user message in request history"),
    }

    fs::remove_dir_all(&temp_root).unwrap();
}

struct CancelOnStreamProvider {
    cancel_token: CancelToken,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

#[async_trait::async_trait]
impl Provider for CancelOnStreamProvider {
    fn id(&self) -> &str {
        "cancel-mock"
    }
    fn display_name(&self) -> &str {
        "Cancel Mock"
    }
    fn default_model(&self) -> &str {
        "mock"
    }
    fn capabilities(&self) -> &ProviderCapabilities {
        static CAP: ProviderCapabilities = ProviderCapabilities {
            supports_tools: false,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: false,
            supports_prompt_caching: false,
            supports_usage_reporting: false,
            supports_streaming: false,
            supports_strict_schema: false,
        };
        &CAP
    }
    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }
    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(0)
    }
    async fn stream(
        &self,
        request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        self.requests.lock().unwrap().push(request);
        self.cancel_token.cancel();
        Err(gestalt_core::error::HarnessError::Cancelled)
    }
}

struct AssertResumeProvider {
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

#[async_trait::async_trait]
impl Provider for AssertResumeProvider {
    fn id(&self) -> &str {
        "assert-mock"
    }
    fn display_name(&self) -> &str {
        "Assert Mock"
    }
    fn default_model(&self) -> &str {
        "mock"
    }
    fn capabilities(&self) -> &ProviderCapabilities {
        static CAP: ProviderCapabilities = ProviderCapabilities {
            supports_tools: false,
            supports_parallel_tools: false,
            supports_vision: false,
            supports_documents: false,
            supports_thinking: false,
            supports_json_schema_tools: false,
            supports_prompt_caching: false,
            supports_usage_reporting: false,
            supports_streaming: false,
            supports_strict_schema: false,
        };
        &CAP
    }
    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }
    fn count_tokens(
        &self,
        _model: &str,
        _messages: &[Message],
    ) -> Result<usize, gestalt_core::error::HarnessError> {
        Ok(0)
    }
    async fn stream(
        &self,
        request: ProviderRequest,
    ) -> Result<EventStream, gestalt_core::error::HarnessError> {
        self.requests.lock().unwrap().push(request);
        let events = vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }];
        let stream = futures::stream::iter(
            events
                .into_iter()
                .map(Ok::<_, gestalt_core::error::HarnessError>),
        );
        Ok(Box::pin(stream))
    }
}

