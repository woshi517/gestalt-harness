use std::{
    collections::{HashMap, VecDeque},
    error::Error as _,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures::stream;
use gestalt_core::{
    agent::AgentLoop,
    approval::{
        ApprovalDecision, ApprovalProvider, ApprovalRequest, AutoApprovalProvider, SessionGrant,
    },
    context::{ContextPipeline, TokenBudget},
    error::{HarnessError, ProviderError},
    event::{AgentEvent, ApprovalOutcome, PolicyStatus, StopReason, VerificationStatus},
    hook::{HookRegistry, ToolHook},
    message::{ContentBlock, Message},
    policy::{PolicyDecision, PolicyEngine, PolicyRequest},
    provider::{EventStream, Provider, ProviderCapabilities, ProviderRequest},
    session::{ExecutionMode, RunResult, Session, SessionConfig},
    tool::{RiskLevel, Tool, ToolCatalog, ToolContext, ToolOutput, ToolSchema},
    turn::TurnAccumulator,
};
use serde_json::{json, Value};

#[test]
fn contract_types_round_trip_through_serde() {
    let message = Message::Assistant {
        content: vec![
            ContentBlock::Text {
                text: "hello".to_string(),
            },
            ContentBlock::ToolUse {
                id: "call-1".to_string(),
                name: "read".to_string(),
                input: json!({"path":"README.md"}),
            },
        ],
    };
    let encoded = serde_json::to_string(&message).expect("message encodes");
    let decoded: Message = serde_json::from_str(&encoded).expect("message decodes");
    assert_eq!(message, decoded);

    let event = AgentEvent::ToolResult {
        id: "call-1".to_string(),
        output: "done".to_string(),
        is_error: false,
        truncated: false,
        tool_name: None,
        working_dir: None,
        duration_ms: None,
        output_hash: None,
        artifact_refs: None,
        policy_source: None,
    };
    let encoded = serde_json::to_string(&event).expect("event encodes");
    let decoded: AgentEvent = serde_json::from_str(&encoded).expect("event decodes");
    assert_eq!(event, decoded);

    let result = RunResult {
        session_id: "session-1".to_string(),
        turns: 2,
        stop_reason: StopReason::EndTurn,
        total_input_tokens: 11,
        total_output_tokens: 7,
        artifacts: vec!["artifact.txt".to_string()],
        workspace_snapshot_id: None,
    };
    let encoded = serde_json::to_string(&result).expect("run result encodes");
    let decoded: RunResult = serde_json::from_str(&encoded).expect("run result decodes");
    assert_eq!(result, decoded);
}

#[test]
fn contract_traits_are_object_safe() {
    let provider = mock_provider(vec![]);
    let tools = Arc::new(MockCatalog::default());
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    accepts_trait_objects(provider, tools, pipeline, policy, approval);
}

#[test]
fn error_display_and_source_are_preserved() {
    let io_error = std::io::Error::other("boom");
    let err = HarnessError::Provider(ProviderError::Transport(io_error));

    assert!(format!("{err}").contains("provider error"));
    assert!(err.source().is_some());
}

#[tokio::test]
async fn test_workspace_snapshotter_captures_correctly() {
    use gestalt_core::snapshot::{GitWorkspaceSnapshotter, WorkspaceSnapshotter};
    use std::fs;
    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-snapshot-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    fs::write(temp_dir.join("file1.txt"), "hello world").unwrap();
    fs::write(temp_dir.join("file2.txt"), "rust is cool").unwrap();
    fs::create_dir_all(temp_dir.join(".gestalt/runs")).unwrap();
    fs::write(temp_dir.join(".gestalt/runs/trace.jsonl"), "trace").unwrap();

    let snapshotter = GitWorkspaceSnapshotter;
    let snapshot = snapshotter.capture(&temp_dir).await.unwrap();

    assert_eq!(
        snapshot.workspace_root.canonicalize().unwrap(),
        temp_dir.canonicalize().unwrap()
    );
    assert!(!snapshot.content_hash.is_empty());
    assert!(snapshot.git_sha.is_none());

    let mut session = make_session(ExecutionMode::Yolo);
    session.tool_ctx.workspace_root = Some(temp_dir.clone());
    session.refresh_snapshot(&snapshotter, None).await.unwrap();
    assert_eq!(
        session.snapshot.workspace_root.canonicalize().unwrap(),
        temp_dir.canonicalize().unwrap()
    );

    let _ = fs::remove_dir_all(&temp_dir);
}

#[tokio::test]
async fn test_workspace_snapshotter_git_path() {
    use gestalt_core::snapshot::{GitWorkspaceSnapshotter, WorkspaceSnapshotter};
    use std::fs;
    use std::process::Command;

    let temp_dir =
        std::env::temp_dir().join(format!("gestalt-snapshot-git-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let run_cmd = |args: &[&str]| {
        let status = Command::new(args[0])
            .args(&args[1..])
            .current_dir(&temp_dir)
            .status();
        status.is_ok() && status.unwrap().success()
    };

    assert!(run_cmd(&["git", "init"]), "failed to run git init");

    run_cmd(&["git", "config", "user.name", "Test User"]);
    run_cmd(&["git", "config", "user.email", "test@example.com"]);

    let file1_path = temp_dir.join("file1.txt");
    fs::write(&file1_path, "initial content").unwrap();
    run_cmd(&["git", "add", "file1.txt"]);
    run_cmd(&["git", "commit", "-m", "first commit"]);

    let snapshotter = GitWorkspaceSnapshotter;

    // Clean check
    let snapshot = snapshotter.capture(&temp_dir).await.unwrap();
    assert!(snapshot.git_sha.is_some());
    assert_eq!(snapshot.git_dirty, Some(false));
    assert_eq!(snapshot.untracked_count, Some(0));
    let hash_clean = snapshot.content_hash.clone();

    // Tracked dirty check
    fs::write(&file1_path, "modified content").unwrap();
    let snapshot_dirty = snapshotter.capture(&temp_dir).await.unwrap();
    assert_eq!(snapshot_dirty.git_dirty, Some(true));
    assert_ne!(snapshot_dirty.content_hash, hash_clean);

    // Commit changes to make clean again
    run_cmd(&["git", "add", "file1.txt"]);
    run_cmd(&["git", "commit", "-m", "commit modifications"]);
    let snapshot_clean2 = snapshotter.capture(&temp_dir).await.unwrap();
    assert_eq!(snapshot_clean2.git_dirty, Some(false));
    let hash_clean2 = snapshot_clean2.content_hash.clone();

    // Untracked-only dirty check
    let file2_path = temp_dir.join("file2.txt");
    fs::write(&file2_path, "untracked content").unwrap();
    let snapshot_untracked = snapshotter.capture(&temp_dir).await.unwrap();
    assert_eq!(snapshot_untracked.untracked_count, Some(1));
    assert_eq!(snapshot_untracked.git_dirty, Some(true));
    assert_eq!(snapshot_untracked.content_hash, hash_clean2);

    let mut session = make_session(ExecutionMode::Yolo);
    session.tool_ctx.workspace_root = Some(temp_dir.clone());

    struct MockTraceSink {
        events: Mutex<Vec<AgentEvent>>,
        snapshot: Mutex<Option<gestalt_core::snapshot::WorkspaceSnapshot>>,
    }
    impl gestalt_core::trace::TraceSink for MockTraceSink {
        fn emit(&self, event: AgentEvent) -> Result<(), gestalt_core::TraceError> {
            self.events.lock().unwrap().push(event);
            Ok(())
        }
        fn flush(&self) -> Result<(), gestalt_core::TraceError> {
            Ok(())
        }
        fn update_snapshot(&self, snapshot: gestalt_core::snapshot::WorkspaceSnapshot) {
            *self.snapshot.lock().unwrap() = Some(snapshot);
        }
    }

    let sink = MockTraceSink {
        events: Mutex::new(Vec::new()),
        snapshot: Mutex::new(None),
    };

    session
        .refresh_snapshot(&snapshotter, Some(&sink))
        .await
        .unwrap();

    let updated_snapshot = sink.snapshot.lock().unwrap().clone().unwrap();
    assert_eq!(updated_snapshot.content_hash, hash_clean2);

    let events = sink.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        AgentEvent::WorkspaceSnapshotCaptured { snapshot_id, dirty } => {
            assert_eq!(snapshot_id, &hash_clean2[..12]);
            assert_eq!(dirty, &true);
        }
        _ => panic!("Expected WorkspaceSnapshotCaptured event"),
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn turn_accumulator_collects_streamed_tool_calls() {
    let mut accumulator = TurnAccumulator::default();

    accumulator
        .record(&AgentEvent::Text {
            delta: "hello".to_string(),
        })
        .expect("text accumulates");
    accumulator
        .record(&AgentEvent::ToolCallStreamed {
            id: "call-1".to_string(),
            name: "read".to_string(),
            input_delta: "{\"path\":\"".to_string(),
        })
        .expect("tool call accumulates");
    accumulator
        .record(&AgentEvent::ToolCallStreamed {
            id: "call-1".to_string(),
            name: "read".to_string(),
            input_delta: "README.md\"}".to_string(),
        })
        .expect("tool call completes");
    accumulator
        .record(&AgentEvent::Stop {
            reason: StopReason::ToolUse,
        })
        .expect("stop records");

    let turn = accumulator.finish().expect("turn finalizes");
    assert_eq!(turn.full_text(), "hello");
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].id, "call-1");
    assert_eq!(turn.tool_calls[0].name, "read");
    assert_eq!(turn.tool_calls[0].input, json!({"path":"README.md"}));
}

#[tokio::test]
async fn agent_loop_handles_text_only_turn() {
    let provider = mock_provider(vec![vec![
        AgentEvent::Text {
            delta: "final answer".to_string(),
        },
        AgentEvent::Stop {
            reason: StopReason::EndTurn,
        },
    ]]);
    let tools = Arc::new(MockCatalog::default());
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);
    session.history.push(Message::User {
        content: vec![ContentBlock::Text {
            text: "question".to_string(),
        }],
    });

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.turns, 1);
    assert_eq!(session.history.len(), 2);
}

#[tokio::test]
async fn agent_loop_executes_single_tool_call() {
    let tool = Arc::new(MockTool::new("read", true, "tool result"));
    let provider = mock_provider(vec![
        vec![
            AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "read".to_string(),
                input_delta: "{\"path\":\"README.md\"}".to_string(),
            },
            AgentEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.turns, 2);
    assert_eq!(tool.executed_inputs.lock().expect("lock").len(), 1);
    assert!(session.history.iter().any(|message| matches!(message, Message::ToolResult { tool_use_id, content, is_error } if tool_use_id == "call-1" && content == "tool result" && !is_error)));
}

#[tokio::test]
async fn agent_loop_emits_rich_context_model_and_tool_metadata() {
    let tool = Arc::new(MockTool::new("read", true, "tool result"));
    let provider = mock_provider(vec![
        vec![
            AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "read".to_string(),
                input_delta: "{\"path\":\"README.md\"}".to_string(),
            },
            AgentEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);
    let events = capture_events();

    let _ = loop_
        .run(&mut session, {
            let events = events.clone();
            move |event| events.lock().expect("lock").push(event)
        })
        .await
        .expect("run succeeds");

    let recorded = events.lock().expect("lock");

    let context = recorded
        .iter()
        .find_map(|event| match event {
            AgentEvent::ContextBuilt {
                packet_hash,
                sources,
                omissions,
                ..
            } => Some((packet_hash, sources, omissions)),
            _ => None,
        })
        .expect("context built event present");
    assert!(context.0.as_ref().is_some_and(|hash| !hash.is_empty()));
    assert!(context.1.is_some());
    assert!(context.2.is_some());

    let model = recorded
        .iter()
        .find_map(|event| match event {
            AgentEvent::ModelRequest {
                packet_hash,
                max_tokens,
                provider_request_hash,
                ..
            } => Some((packet_hash, max_tokens, provider_request_hash)),
            _ => None,
        })
        .expect("model request event present");
    assert!(model.0.as_ref().is_some_and(|hash| !hash.is_empty()));
    assert!(model.1.is_some_and(|value| value > 0));
    assert!(model.2.as_ref().is_some_and(|hash| !hash.is_empty()));

    let tool_result = recorded
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult {
                tool_name,
                duration_ms,
                output_hash,
                policy_source,
                ..
            } => Some((tool_name, duration_ms, output_hash, policy_source)),
            _ => None,
        })
        .expect("tool result event present");
    assert_eq!(tool_result.0.as_deref(), Some("read"));
    assert!(tool_result.1.is_some());
    assert!(tool_result.2.as_ref().is_some_and(|hash| !hash.is_empty()));
    assert_eq!(tool_result.3.as_deref(), Some("allow-all"));

    drop(recorded);
}

#[tokio::test]
async fn agent_loop_preserves_original_tool_result_order() {
    let first = Arc::new(MockTool::new("alpha", true, "alpha result"));
    let second = Arc::new(MockTool::new("beta", false, "beta result"));
    let provider = mock_provider(vec![
        vec![
            AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "beta".to_string(),
                input_delta: "{\"value\":\"b\"}".to_string(),
            },
            AgentEvent::ToolCallStreamed {
                id: "call-2".to_string(),
                name: "alpha".to_string(),
                input_delta: "{\"value\":\"a\"}".to_string(),
            },
            AgentEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![first.clone(), second.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.turns, 2);
    let tool_results = session
        .history
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some((tool_use_id.clone(), content.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_results,
        vec![
            ("call-1".to_string(), "beta result".to_string()),
            ("call-2".to_string(), "alpha result".to_string()),
        ]
    );
}

#[tokio::test]
async fn agent_loop_denies_tool_call_as_error_result() {
    let tool = Arc::new(MockTool::new("write", false, "should not run"));
    let provider = mock_provider(vec![vec![
        AgentEvent::ToolCallStreamed {
            id: "call-1".to_string(),
            name: "write".to_string(),
            input_delta: "{\"path\":\"file.txt\"}".to_string(),
        },
        AgentEvent::Stop {
            reason: StopReason::ToolUse,
        },
    ]]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::deny_all("blocked by policy"));
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.turns, 2);
    assert!(tool.executed_inputs.lock().expect("lock").is_empty());
    assert!(session
        .history
        .iter()
        .any(|message| matches!(message, Message::ToolResult { tool_use_id, is_error, .. } if tool_use_id == "call-1" && *is_error)));
}

#[tokio::test]
async fn agent_loop_routes_confirm_calls_through_approval() {
    let tool = Arc::new(MockTool::new("edit", true, "approved"));
    let provider = mock_provider(vec![
        vec![
            AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "edit".to_string(),
                input_delta: "{\"value\":\"original\"}".to_string(),
            },
            AgentEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::confirm_all("confirm required"));
    let approval = Arc::new(MockApproval::approve_all());
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Confirm);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.turns, 2);
    assert_eq!(tool.executed_inputs.lock().expect("lock").len(), 1);
}

#[tokio::test]
async fn agent_loop_stops_on_max_turns() {
    let provider = mock_provider(vec![
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);
    let tools = Arc::new(MockCatalog::default());
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 1);
    let mut session = make_session(ExecutionMode::Yolo);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.stop_reason, StopReason::MaxTurns);
}

#[tokio::test]
async fn agent_loop_stops_on_budget_exhaustion() {
    let provider = mock_provider(vec![vec![AgentEvent::Stop {
        reason: StopReason::EndTurn,
    }]]);
    let tools = Arc::new(MockCatalog::default());
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Yolo);
    session.token_budget = TokenBudget {
        model_limit: 16,
        reserved_output: 8,
        used_system: 0,
        used_history: 0,
        used_sources: 0,
        used_tools: 0,
        used_memory: 0,
        minimum_turn_budget: 16,
    };

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");

    assert_eq!(result.stop_reason, StopReason::BudgetExhausted);
}

fn capture_events() -> Arc<Mutex<Vec<AgentEvent>>> {
    Arc::new(Mutex::new(Vec::new()))
}

fn approval_decisions(events: &[AgentEvent]) -> Vec<&AgentEvent> {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ApprovalDecision { .. }))
        .collect()
}

fn bash_call_event_stream(call_id: &str, command: &str, with_stop: StopReason) -> Vec<AgentEvent> {
    let mut events = vec![AgentEvent::ToolCallStreamed {
        id: call_id.to_string(),
        name: "bash".to_string(),
        input_delta: format!("{{\"command\":\"{command}\"}}"),
    }];
    events.push(AgentEvent::Stop { reason: with_stop });
    events
}

#[tokio::test]
async fn session_grant_auto_approves_same_input_with_session_grant_source() {
    let tool = Arc::new(RiskAwareMockTool::new(
        "bash",
        |input| {
            input
                .get("command")
                .and_then(|v| v.as_str())
                .map_or(RiskLevel::Low, |c| {
                    if c == "rm" {
                        RiskLevel::High
                    } else {
                        RiskLevel::Low
                    }
                })
        },
        "ran",
    ));
    let provider = mock_provider(vec![
        bash_call_event_stream("call-1", "safe-ls", StopReason::ToolUse),
        bash_call_event_stream("call-2", "safe-ls", StopReason::EndTurn),
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::confirm_all("confirm-required"));
    let approval = Arc::new(MockApproval::always_allow());
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Confirm);
    let events = capture_events();

    let _ = loop_
        .run(&mut session, {
            let events = events.clone();
            move |event| events.lock().expect("lock").push(event)
        })
        .await
        .expect("run succeeds");

    let recorded = events.lock().expect("lock");
    let policy_decisions: Vec<&AgentEvent> = recorded
        .iter()
        .filter(|e| matches!(e, AgentEvent::PolicyDecision { .. }))
        .collect();
    assert_eq!(
        policy_decisions.len(),
        2,
        "two policy decisions expected (call 1 + call 2)"
    );
    let AgentEvent::PolicyDecision {
        policy_source: first_source,
        ..
    } = policy_decisions[0]
    else {
        unreachable!()
    };
    assert_eq!(
        first_source, "confirm-all",
        "first call has no prior grant, so policy_source is confirm-all"
    );
    let AgentEvent::PolicyDecision {
        policy_source: second_source,
        ..
    } = policy_decisions[1]
    else {
        unreachable!()
    };
    assert!(
        second_source.starts_with("session_grant:"),
        "expected second policy_source to start with session_grant:, got {second_source}"
    );
    drop(policy_decisions);
    drop(recorded);

    assert_eq!(
        tool.executed_inputs.lock().expect("lock").len(),
        2,
        "both calls should have executed (second auto-approved via the session grant)"
    );

    let recorded = events.lock().expect("lock");
    let approval_events = approval_decisions(&recorded);
    assert_eq!(approval_events.len(), 1, "only the first call was approved");
    drop(approval_events);
    drop(recorded);
}
#[tokio::test]
async fn session_grant_does_not_apply_to_different_input() {
    let tool = Arc::new(RiskAwareMockTool::new("bash", |_| RiskLevel::Medium, "ran"));
    let provider = mock_provider(vec![
        bash_call_event_stream("call-1", "ls", StopReason::ToolUse),
        bash_call_event_stream("call-2", "rm", StopReason::EndTurn),
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::confirm_all("confirm-required"));
    let approval = Arc::new(MockApproval::always_allow());
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Confirm);
    let events = capture_events();

    let _ = loop_
        .run(&mut session, {
            let events = events.clone();
            move |event| events.lock().expect("lock").push(event)
        })
        .await
        .expect("run succeeds");

    let recorded = events.lock().expect("lock");
    let policy_sources: Vec<&str> = recorded
        .iter()
        .filter_map(|e| match e {
            AgentEvent::PolicyDecision { policy_source, .. } => Some(policy_source.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(policy_sources.len(), 2, "two policy decisions expected");
    assert_eq!(
        policy_sources[0], "confirm-all",
        "first call has no prior grant, so policy_source must be confirm-all (got {})",
        policy_sources[0]
    );
    assert_eq!(
        policy_sources[1], "confirm-all",
        "second call has different input, so the grant must NOT apply and policy must re-confirm (got {})",
        policy_sources[1]
    );
    drop(policy_sources);
    let grants: Vec<&SessionGrant> = recorded
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ApprovalDecision {
                grant_terms: Some(g),
                ..
            } => Some(g),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 2, "both calls went through approval");
    assert_ne!(grants[0].input_hash, grants[1].input_hash);
    assert_ne!(grants[0].input_hash, "");
    drop(grants);
    drop(recorded);

    assert_eq!(
        tool.executed_inputs.lock().expect("lock").len(),
        2,
        "both calls approved, both should have executed"
    );
}

#[tokio::test]
async fn session_grant_blocks_riskier_call_after_low_risk_approval() {
    let tool = Arc::new(RiskAwareMockTool::new(
        "bash",
        |input| {
            input
                .get("command")
                .and_then(|v| v.as_str())
                .map_or(RiskLevel::Low, |c| {
                    if c == "rm" {
                        RiskLevel::Critical
                    } else {
                        RiskLevel::Low
                    }
                })
        },
        "ran",
    ));
    let provider = mock_provider(vec![
        bash_call_event_stream("call-1", "ls", StopReason::ToolUse),
        bash_call_event_stream("call-2", "rm", StopReason::EndTurn),
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(DenyOnCriticalPolicy);
    let approval = Arc::new(MockApproval::always_allow());
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Confirm);
    let events = capture_events();

    let _ = loop_
        .run(&mut session, {
            let events = events.clone();
            move |event| events.lock().expect("lock").push(event)
        })
        .await
        .expect("run succeeds");

    let recorded = events.lock().expect("lock");
    let policy_decisions: Vec<&AgentEvent> = recorded
        .iter()
        .filter(|e| matches!(e, AgentEvent::PolicyDecision { .. }))
        .collect();
    assert_eq!(policy_decisions.len(), 2);
    let AgentEvent::PolicyDecision {
        decision,
        policy_source,
        ..
    } = policy_decisions[1]
    else {
        unreachable!()
    };
    assert_eq!(*decision, PolicyStatus::Denied);
    assert_eq!(policy_source, "critical-denied");
    drop(policy_decisions);
    drop(recorded);

    let executed = tool.executed_inputs.lock().expect("lock");
    assert_eq!(
        executed.len(),
        1,
        "only the first call should have executed"
    );
    assert_eq!(executed[0], json!({"command": "ls"}));
    drop(executed);
}

#[tokio::test]
async fn unknown_tool_still_logs_a_policy_decision() {
    let provider = mock_provider(vec![bash_call_event_stream(
        "call-1",
        "ghost",
        StopReason::EndTurn,
    )]);
    let tools = Arc::new(MockCatalog::default());
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::confirm_all("confirm-required"));
    let approval = Arc::new(MockApproval::approve_all());
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Confirm);
    let events = capture_events();

    let _ = loop_
        .run(&mut session, {
            let events = events.clone();
            move |event| events.lock().expect("lock").push(event)
        })
        .await
        .expect("run succeeds");

    let recorded = events.lock().expect("lock");
    let policy_events: Vec<&AgentEvent> = recorded
        .iter()
        .filter(|e| matches!(e, AgentEvent::PolicyDecision { .. }))
        .collect();
    assert_eq!(
        policy_events.len(),
        1,
        "unknown tools should still log policy"
    );

    let AgentEvent::PolicyDecision {
        tool_name,
        input_hash,
        risk,
        mode,
        matched_rule,
        decision,
        policy_source,
        ..
    } = policy_events[0]
    else {
        unreachable!()
    };
    assert_eq!(tool_name.as_deref(), Some("bash"));
    assert!(input_hash.as_deref().is_some_and(|hash| !hash.is_empty()));
    assert_eq!(risk, &Some(RiskLevel::Critical));
    assert_eq!(mode, &Some(ExecutionMode::Confirm));
    assert_eq!(matched_rule.as_deref(), Some("tool.not_found"));
    assert_eq!(*decision, PolicyStatus::Denied);
    assert_eq!(policy_source, "tool.not_found");

    assert!(recorded.iter().any(|event| matches!(
        event,
        AgentEvent::ToolResult {
            output,
            is_error: true,
            ..
        } if output.contains("Tool not found: bash")
    )));

    drop(recorded);
}

#[tokio::test]
async fn approval_decision_events_cover_approve_and_deny() {
    let run = |approval: MockApproval| async move {
        let tool = Arc::new(RiskAwareMockTool::new("bash", |_| RiskLevel::Low, "ran"));
        let provider = mock_provider(vec![bash_call_event_stream(
            "call-1",
            "ls",
            StopReason::EndTurn,
        )]);
        let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
        let pipeline = Arc::new(MockPipeline::default());
        let policy = Arc::new(MockPolicy::confirm_all("confirm-required"));
        let approval = Arc::new(approval);
        let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
        let mut session = make_session(ExecutionMode::Confirm);
        let events = capture_events();

        let _ = loop_
            .run(&mut session, {
                let events = events.clone();
                move |event| events.lock().expect("lock").push(event)
            })
            .await
            .expect("run succeeds");

        (tool, events)
    };

    let (approved_tool, approved_events) = run(MockApproval::approve_all()).await;
    let approved = {
        let approved_recorded = approved_events.lock().expect("lock");
        approval_decisions(&approved_recorded)
            .into_iter()
            .map(|event| match event {
                AgentEvent::ApprovalDecision { decision, .. } => *decision,
                _ => unreachable!(),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(approved.len(), 1);
    assert_eq!(approved[0], ApprovalOutcome::Approve);
    assert_eq!(approved_tool.executed_inputs.lock().expect("lock").len(), 1);

    let (denied_tool, denied_events) = run(MockApproval::deny_all()).await;
    let denied = {
        let denied_recorded = denied_events.lock().expect("lock");
        approval_decisions(&denied_recorded)
            .into_iter()
            .map(|event| match event {
                AgentEvent::ApprovalDecision { decision, .. } => *decision,
                _ => unreachable!(),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0], ApprovalOutcome::Deny);
    assert_eq!(denied_tool.executed_inputs.lock().expect("lock").len(), 0);
}

#[tokio::test]
async fn session_grant_emits_approval_decision_event_with_grant_terms() {
    let tool = Arc::new(RiskAwareMockTool::new("bash", |_| RiskLevel::Low, "ran"));
    let provider = mock_provider(vec![bash_call_event_stream(
        "call-1",
        "ls",
        StopReason::EndTurn,
    )]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::confirm_all("confirm-required"));
    let approval = Arc::new(MockApproval::always_allow());
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Confirm);
    let events = capture_events();

    let _ = loop_
        .run(&mut session, {
            let events = events.clone();
            move |event| events.lock().expect("lock").push(event)
        })
        .await
        .expect("run succeeds");

    let recorded = events.lock().expect("lock");
    let approvals = approval_decisions(&recorded);
    assert_eq!(approvals.len(), 1);
    let AgentEvent::ApprovalDecision {
        tool_call_id,
        decision,
        grant_terms,
        original_input_hash,
        edited_input_hash,
    } = approvals[0]
    else {
        unreachable!()
    };
    assert_eq!(tool_call_id, "call-1");
    assert_eq!(*decision, ApprovalOutcome::AlwaysAllow);
    assert!(grant_terms.is_some());
    assert!(!original_input_hash.is_empty());
    assert!(edited_input_hash.is_none());
    let grant = grant_terms.as_ref().expect("grant present");
    assert_eq!(grant.tool_name, "bash");
    assert_eq!(grant.matched_rule, "confirm-all");
    assert_eq!(grant.policy_source, "session_grant");
    assert_eq!(grant.risk_ceiling, RiskLevel::Low);
    assert_eq!(grant.granted_at_turn, 0);
    assert_eq!(grant.expires_in_turns, 3);
    drop(approvals);
    drop(recorded);
}

#[tokio::test]
async fn session_grant_edit_re_evaluates_policy_and_emits_edited_hash() {
    let tool = Arc::new(RiskAwareMockTool::new(
        "bash",
        |input| {
            input
                .get("command")
                .and_then(|v| v.as_str())
                .map_or(RiskLevel::Medium, |c| {
                    if c == "ls" {
                        RiskLevel::Low
                    } else {
                        RiskLevel::Medium
                    }
                })
        },
        "ran",
    ));
    let provider = mock_provider(vec![bash_call_event_stream(
        "call-1",
        "bad-cmd",
        StopReason::EndTurn,
    )]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(AllowLowRiskConfirmHighPolicy);
    let approval = Arc::new(MockApproval::edit_to_input(json!({"command": "ls"})));
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Confirm);
    let events = capture_events();

    let _ = loop_
        .run(&mut session, {
            let events = events.clone();
            move |event| events.lock().expect("lock").push(event)
        })
        .await
        .expect("run succeeds");

    let recorded = events.lock().expect("lock");
    let policy_count = recorded
        .iter()
        .filter(|e| matches!(e, AgentEvent::PolicyDecision { .. }))
        .count();
    assert_eq!(
        policy_count, 2,
        "two policy decisions: first for original, second for edited"
    );
    let approvals = approval_decisions(&recorded);
    assert_eq!(approvals.len(), 1);
    let AgentEvent::ApprovalDecision {
        decision,
        original_input_hash,
        edited_input_hash,
        grant_terms,
        ..
    } = approvals[0]
    else {
        unreachable!()
    };
    assert_eq!(*decision, ApprovalOutcome::Edit);
    assert!(!original_input_hash.is_empty());
    assert!(edited_input_hash.is_some());
    assert_ne!(
        original_input_hash.as_str(),
        edited_input_hash.as_deref().expect("edit present")
    );
    assert!(grant_terms.is_none());
    drop(approvals);
    drop(recorded);

    assert_eq!(
        tool.executed_inputs.lock().expect("lock").len(),
        1,
        "edited input passed policy re-evaluation"
    );
}

#[tokio::test]
async fn session_grant_records_distinct_grants_for_different_inputs_under_repeated_approval() {
    let tool = Arc::new(RiskAwareMockTool::new(
        "bash",
        |input| {
            input
                .get("command")
                .and_then(|v| v.as_str())
                .map_or(RiskLevel::Low, |c| {
                    if c == "rm" {
                        RiskLevel::High
                    } else {
                        RiskLevel::Low
                    }
                })
        },
        "ran",
    ));
    let provider = mock_provider(vec![
        bash_call_event_stream("call-1", "ls", StopReason::ToolUse),
        bash_call_event_stream("call-2", "rm", StopReason::EndTurn),
    ]);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::confirm_all("confirm-required"));
    let approval = Arc::new(MockApproval::always_allow());
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3);
    let mut session = make_session(ExecutionMode::Confirm);
    let events = capture_events();

    let _ = loop_
        .run(&mut session, {
            let events = events.clone();
            move |event| events.lock().expect("lock").push(event)
        })
        .await
        .expect("run succeeds");

    let recorded = events.lock().expect("lock");
    let policy_decision_count = recorded
        .iter()
        .filter(|e| matches!(e, AgentEvent::PolicyDecision { .. }))
        .count();
    assert_eq!(policy_decision_count, 2);
    let grants: Vec<&SessionGrant> = recorded
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ApprovalDecision {
                grant_terms: Some(g),
                ..
            } => Some(g),
            _ => None,
        })
        .collect();
    assert_eq!(grants.len(), 2, "both calls produced grants");
    assert_eq!(grants[0].risk_ceiling, RiskLevel::Low);
    assert_eq!(grants[1].risk_ceiling, RiskLevel::High);
    assert_ne!(grants[0].input_hash, grants[1].input_hash);
    drop(grants);
    drop(recorded);

    let executed = tool.executed_inputs.lock().expect("lock");
    assert_eq!(
        executed.len(),
        2,
        "both calls approved by user, both executed"
    );
    drop(executed);
}

#[derive(Default)]
struct MockWriteTool;

#[async_trait::async_trait]
impl Tool for MockWriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "mock write"
    }

    fn schema(&self) -> ToolSchema {
        json!({"name": "write"})
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Medium
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::ToolError> {
        let path = input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = input
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let target = if std::path::Path::new(path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            ctx.working_dir.join(path)
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(gestalt_core::ToolError::ExecutionFailed)?;
        }
        std::fs::write(&target, content.as_bytes())
            .map_err(gestalt_core::ToolError::ExecutionFailed)?;

        Ok(ToolOutput::Text {
            content: json!({
                "path": path,
                "bytes_written": content.len(),
            })
            .to_string(),
        })
    }
}

#[tokio::test]
async fn agent_loop_runs_real_verification_hook_and_emits_verification_result() {
    let root = std::env::temp_dir().join(format!(
        "phase1-verification-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create temp workspace");

    let provider = mock_provider(vec![
        vec![
            AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "write".to_string(),
                input_delta: "{\"path\":\"notes.md\",\"content\":\"# Title\\n\\nBody\"}"
                    .to_string(),
            },
            AgentEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);

    let tool = Arc::new(MockWriteTool);
    let tools = Arc::new(MockCatalog::with_tools(vec![tool.clone()]));

    let pipeline = Arc::new(MockPipeline);
    let policy = Arc::new(MockPolicy {
        decision_for: Arc::new(|_| PolicyDecision::allowed(None)),
    });
    let approval = Arc::new(AutoApprovalProvider);

    let mut verifier_registry = gestalt_verify::VerifierRegistry::new();
    verifier_registry.register(Box::new(gestalt_verify::FileExistsVerifier));
    verifier_registry.register(Box::new(gestalt_verify::MarkdownStructureVerifier));

    let mut hooks = HookRegistry::new();
    hooks.register_tool_hook(Arc::new(gestalt_verify::VerificationToolHook::new(
        verifier_registry,
    )));

    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3).with_hooks(hooks);

    let mut session = make_session(ExecutionMode::Yolo);
    session.tool_ctx.working_dir = root.clone();
    session.tool_ctx.workspace_root = Some(root.clone());

    let mut events = Vec::new();
    let result = loop_
        .run(&mut session, |event| {
            events.push(event);
            Ok(())
        })
        .await;

    assert!(result.is_ok());

    let verification_events: Vec<_> = events
        .iter()
        .filter(|ev| matches!(ev, AgentEvent::VerificationResult { .. }))
        .collect();
    assert_eq!(
        verification_events.len(),
        2,
        "real registry should emit one result per verifier"
    );
    for ev in verification_events {
        if let AgentEvent::VerificationResult { status, failed, .. } = ev {
            assert_eq!(*status, VerificationStatus::Passed);
            assert_eq!(*failed, 0);
        }
    }
    assert!(
        root.join("notes.md").exists(),
        "mock write tool should create the file"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[derive(Clone)]
struct RecordingToolOrderHook {
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl ToolHook for RecordingToolOrderHook {
    async fn before_tool_execution(
        &self,
        session: &Session,
        _tool_name: &str,
        _input: &Value,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        let call_id = session
            .tool_ctx
            .current_tool_call_id
            .as_deref()
            .unwrap_or("missing");
        self.log
            .lock()
            .expect("lock")
            .push(format!("before:{call_id}"));
        Ok(vec![])
    }

    async fn after_tool_execution(
        &self,
        _session: &Session,
        _tool_name: &str,
        _result: &gestalt_core::tool::ToolExecutionResult,
    ) -> gestalt_core::error::Result<Vec<AgentEvent>> {
        Ok(vec![])
    }
}

#[derive(Clone)]
struct RecordingToolOrderTool {
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait::async_trait]
impl Tool for RecordingToolOrderTool {
    fn name(&self) -> &str {
        "record"
    }

    fn description(&self) -> &str {
        "record ordering"
    }

    fn schema(&self) -> ToolSchema {
        json!({"name": "record"})
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        RiskLevel::Medium
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(
        &self,
        _input: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::ToolError> {
        let call_id = ctx
            .current_tool_call_id
            .as_deref()
            .unwrap_or("missing")
            .to_string();
        self.log
            .lock()
            .expect("lock")
            .push(format!("tool:{call_id}"));
        Ok(ToolOutput::Text {
            content: format!("executed:{call_id}"),
        })
    }
}

#[tokio::test]
async fn agent_loop_runs_tool_hook_immediately_before_each_tool_execution() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let provider = mock_provider(vec![
        vec![
            AgentEvent::ToolCallStreamed {
                id: "call-1".to_string(),
                name: "record".to_string(),
                input_delta: "{}".to_string(),
            },
            AgentEvent::ToolCallStreamed {
                id: "call-2".to_string(),
                name: "record".to_string(),
                input_delta: "{}".to_string(),
            },
            AgentEvent::Stop {
                reason: StopReason::ToolUse,
            },
        ],
        vec![AgentEvent::Stop {
            reason: StopReason::EndTurn,
        }],
    ]);

    let tool = Arc::new(RecordingToolOrderTool { log: log.clone() });
    let tools = Arc::new(MockCatalog::with_tools(vec![tool]));
    let pipeline = Arc::new(MockPipeline::default());
    let policy = Arc::new(MockPolicy::allow_all());
    let approval = Arc::new(AutoApprovalProvider);

    let mut hooks = HookRegistry::new();
    hooks.register_tool_hook(Arc::new(RecordingToolOrderHook { log: log.clone() }));

    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, 3).with_hooks(hooks);
    let mut session = make_session(ExecutionMode::Yolo);

    let result = loop_.run(&mut session, |_| {}).await.expect("run succeeds");
    assert_eq!(result.turns, 2);

    let recorded = log.lock().expect("lock").clone();
    assert_eq!(
        recorded,
        vec![
            "before:call-1",
            "tool:call-1",
            "before:call-2",
            "tool:call-2"
        ]
    );
}

fn accepts_trait_objects(
    _provider: Arc<dyn Provider>,
    _tools: Arc<dyn ToolCatalog>,
    _pipeline: Arc<dyn ContextPipeline>,
    _policy: Arc<dyn PolicyEngine>,
    _approval: Arc<dyn ApprovalProvider>,
) {
}

fn make_session(mode: ExecutionMode) -> Session {
    let snapshot = gestalt_core::snapshot::WorkspaceSnapshot {
        workspace_root: std::env::current_dir().expect("cwd"),
        git_sha: None,
        git_dirty: None,
        untracked_count: None,
        content_hash: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        captured_at: chrono::Utc::now(),
    };
    Session::new(
        "session-1",
        SessionConfig {
            model: "mock-model".to_string(),
            provider: "mock".to_string(),
            max_tokens: 128,
            temperature: Some(0.0),
            max_turns: 3,
        },
        TokenBudget {
            model_limit: 256,
            reserved_output: 32,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 8,
        },
        ToolContext {
            working_dir: std::env::current_dir().expect("cwd"),
            workspace_root: Some(std::env::current_dir().expect("cwd")),
            timeout: Duration::from_secs(1),
            allow_network: true,
            environment: HashMap::new(),
            max_output_bytes: 1024,
            artifact_dir: None,
            current_tool_call_id: None,
        },
        mode,
        snapshot,
    )
}

fn mock_provider(turns: Vec<Vec<AgentEvent>>) -> Arc<MockProvider> {
    Arc::new(MockProvider {
        turns: Mutex::new(turns.into_iter().collect()),
        capabilities: ProviderCapabilities::default(),
    })
}

struct MockProvider {
    turns: Mutex<VecDeque<Vec<AgentEvent>>>,
    capabilities: ProviderCapabilities,
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn id(&self) -> &str {
        "mock"
    }

    fn display_name(&self) -> &str {
        "Mock Provider"
    }

    fn default_model(&self) -> &str {
        "mock-model"
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }

    fn model_info(&self, _model: &str) -> Option<gestalt_core::ModelInfo> {
        None
    }

    fn count_tokens(&self, _model: &str, messages: &[Message]) -> Result<usize, HarnessError> {
        Ok(messages.len().saturating_mul(8))
    }

    async fn stream(&self, _request: ProviderRequest) -> Result<EventStream, HarnessError> {
        let events = self
            .turns
            .lock()
            .expect("lock")
            .pop_front()
            .unwrap_or_else(|| {
                vec![AgentEvent::Stop {
                    reason: StopReason::EndTurn,
                }]
            });

        let stream = stream::iter(events.into_iter().map(Ok::<_, HarnessError>));
        Ok(Box::pin(stream))
    }
}

struct MockTool {
    name: String,
    parallel_safe: bool,
    output: String,
    executed_inputs: Arc<Mutex<Vec<Value>>>,
}

impl MockTool {
    fn new(name: impl Into<String>, parallel_safe: bool, output: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            parallel_safe,
            output: output.into(),
            executed_inputs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "mock tool"
    }

    fn schema(&self) -> ToolSchema {
        json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            }
        })
    }

    fn risk(&self, _input: &Value) -> RiskLevel {
        if self.parallel_safe {
            RiskLevel::Low
        } else {
            RiskLevel::Medium
        }
    }

    fn can_run_in_parallel(&self, _input: &Value) -> bool {
        self.parallel_safe
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::ToolError> {
        self.executed_inputs.lock().expect("lock").push(input);
        Ok(ToolOutput::Text {
            content: self.output.clone(),
        })
    }
}

#[derive(Default)]
struct MockCatalog {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl MockCatalog {
    fn with_tools<T: Tool + 'static>(tools: Vec<Arc<T>>) -> Self {
        let mut catalog = Self::default();
        for tool in tools {
            catalog
                .tools
                .insert(tool.name().to_string(), tool as Arc<dyn Tool>);
        }
        catalog
    }
}

#[async_trait::async_trait]
impl ToolCatalog for MockCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|tool| tool.schema()).collect()
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
}

#[derive(Default)]
struct MockPipeline;

impl ContextPipeline for MockPipeline {
    fn process(&self, history: &[Message], _budget: &TokenBudget) -> Vec<Message> {
        history.to_vec()
    }

    fn version(&self) -> &str {
        "mock-pipeline"
    }
}

struct MockPolicy {
    decision_for: Arc<dyn Fn(&PolicyRequest) -> PolicyDecision + Send + Sync>,
}

impl MockPolicy {
    fn allow_all() -> Self {
        Self {
            decision_for: Arc::new(|_request| PolicyDecision {
                status: PolicyStatus::Allowed,
                reason: None,
                policy_source: "allow-all".to_string(),
            }),
        }
    }

    fn deny_all(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            decision_for: Arc::new(move |_request| PolicyDecision {
                status: PolicyStatus::Denied,
                reason: Some(reason.clone()),
                policy_source: "deny-all".to_string(),
            }),
        }
    }

    fn confirm_all(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            decision_for: Arc::new(move |_request| PolicyDecision {
                status: PolicyStatus::Confirm,
                reason: Some(reason.clone()),
                policy_source: "confirm-all".to_string(),
            }),
        }
    }
}

#[async_trait::async_trait]
impl PolicyEngine for MockPolicy {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        (self.decision_for)(&request)
    }
}

struct MockApproval {
    decision_for: Arc<dyn Fn(&ApprovalRequest) -> ApprovalDecision + Send + Sync>,
}

impl MockApproval {
    fn approve_all() -> Self {
        Self {
            decision_for: Arc::new(|_request| ApprovalDecision::Approve),
        }
    }

    fn deny_all() -> Self {
        Self {
            decision_for: Arc::new(|_request| ApprovalDecision::Deny),
        }
    }

    fn always_allow() -> Self {
        Self {
            decision_for: Arc::new(|_request| ApprovalDecision::AlwaysAllowForSession),
        }
    }

    fn edit_to_input(target: Value) -> Self {
        Self {
            decision_for: Arc::new(move |_request| ApprovalDecision::Edit(target.clone())),
        }
    }
}

struct RiskAwareMockTool {
    name: String,
    risk_for: Arc<dyn Fn(&Value) -> RiskLevel + Send + Sync>,
    output: String,
    executed_inputs: Arc<Mutex<Vec<Value>>>,
}

impl RiskAwareMockTool {
    fn new(
        name: impl Into<String>,
        risk_for: impl Fn(&Value) -> RiskLevel + Send + Sync + 'static,
        output: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            risk_for: Arc::new(risk_for),
            output: output.into(),
            executed_inputs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl Tool for RiskAwareMockTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "risk-aware mock tool"
    }

    fn schema(&self) -> ToolSchema {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" }
            }
        })
    }

    fn risk(&self, input: &Value) -> RiskLevel {
        (self.risk_for)(input)
    }

    fn can_run_in_parallel(&self, input: &Value) -> bool {
        matches!(self.risk(input), RiskLevel::Low)
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, gestalt_core::ToolError> {
        self.executed_inputs.lock().expect("lock").push(input);
        Ok(ToolOutput::Text {
            content: self.output.clone(),
        })
    }
}

struct AllowLowRiskConfirmHighPolicy;

#[async_trait::async_trait]
impl PolicyEngine for AllowLowRiskConfirmHighPolicy {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        if matches!(request.risk, RiskLevel::Low) {
            PolicyDecision {
                status: PolicyStatus::Allowed,
                reason: Some("low-risk allowed".to_string()),
                policy_source: "allow-low-risk".to_string(),
            }
        } else {
            PolicyDecision::confirm("confirm required".to_string(), "confirm-all".to_string())
        }
    }
}

struct DenyOnCriticalPolicy;

#[async_trait::async_trait]
impl PolicyEngine for DenyOnCriticalPolicy {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        if matches!(request.risk, RiskLevel::Critical) {
            PolicyDecision::denied(
                "critical-risk call denied".to_string(),
                "critical-denied".to_string(),
            )
        } else {
            PolicyDecision::confirm("confirm required".to_string(), "confirm-all".to_string())
        }
    }
}

#[async_trait::async_trait]
impl ApprovalProvider for MockApproval {
    async fn approve(&self, request: ApprovalRequest) -> ApprovalDecision {
        (self.decision_for)(&request)
    }
}
