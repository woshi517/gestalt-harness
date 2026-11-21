#![forbid(unsafe_code)]
//! Comprehensive integration and security conformance suite for `gestalt-runtime` (Milestone 1.2).

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::pin::Pin;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gestalt_kernel::{
    AgentLoop, AgentLoopFactory, ArtifactPatch, ArtifactStoreView, BudgetCap, ContextBuilder,
    Effect, EffectEnvelope, ExtensionRegistry, LoopAction, LoopConfig, LoopContext,
    ModelCapabilities, ModelKind, PolicyDecision, PolicyGate, PolicyRule, Preset, ReplayEngine,
    ReplayMode, RunEvent, SaveMode, SessionSlice, SessionTree, SideEffectClass, SteeringKind,
    SteeringSource, StepResult, ToolSpec, Usage, WorkspaceMode,
};
use gestalt_runtime::sandbox::UnsandboxedFallbackDriver;
use gestalt_runtime::{
    CancellationToken, ConfigLoadOptions, CredentialStore, EventSink, ExternalPolicyBridgeRule,
    ExternalReplayOptions, HeadlessPauseHandler, LiveSteeringMessage, ModelProvider, ModelRequest,
    ModelResponse, ProviderContext, RuntimeError, RuntimeHost, RuntimeRegistry, SandboxDriver,
    SessionLockGuard, StreamDeltaFn, ToolExecutionContext, ToolHandler, ToolOutcome, TurnOutcome,
    WorkspaceTrustStore, capture_worktree_diff_blob, create_worktree, default_sandbox_driver,
    load_and_merge_config, prune_clean_worktrees, replay_session_tree,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

#[derive(Default)]
struct CollectingSink {
    events: Mutex<Vec<RunEvent>>,
}

impl EventSink for CollectingSink {
    fn on_event(&self, event: &RunEvent) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push(event.clone());
        }
    }
}

struct DummyBuilder;
impl ContextBuilder for DummyBuilder {
    fn id(&self) -> &str {
        "dummy_builder"
    }
    fn digest(&self) -> &str {
        "db_v1"
    }
    fn build(
        &self,
        _session: &SessionSlice,
        _artifacts: &ArtifactStoreView,
        _caps: &ModelCapabilities,
    ) -> Value {
        json!({"prompt": "hi"})
    }
}

struct ParallelToolAndSaveLoop {
    step_idx: usize,
}
impl AgentLoop for ParallelToolAndSaveLoop {
    fn id(&self) -> &str {
        "parallel_save_loop"
    }
    fn digest(&self) -> &str {
        "psl_v1"
    }
    fn step(&mut self, _ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        self.step_idx += 1;
        match self.step_idx {
            1 => LoopAction::Yield(vec![
                Effect::Tool {
                    name: "big_echo".to_string(),
                    args: json!({"size": 40000}),
                    side_effect: SideEffectClass::Read,
                },
                Effect::Tool {
                    name: "big_echo".to_string(),
                    args: json!({"size": "invalid_int"}),
                    side_effect: SideEffectClass::Read,
                },
            ]),
            2 => {
                let res = results.unwrap_or_default();
                assert_eq!(res.len(), 2);
                assert!(matches!(
                    &res[0],
                    StepResult::Tool {
                        is_error: false,
                        blob_digest: Some(_),
                        ..
                    }
                ));
                assert!(matches!(&res[1], StepResult::Tool { is_error: true, .. }));
                LoopAction::Yield(vec![Effect::Save {
                    artifact: ".gestalt/protected/note.md".to_string(),
                    patch: ArtifactPatch::WriteFile {
                        content: "staged content\n".to_string(),
                    },
                    mode: SaveMode::Stage,
                }])
            }
            3 => LoopAction::Yield(vec![Effect::Save {
                artifact: ".gestalt/protected/note.md".to_string(),
                patch: ArtifactPatch::WriteFile {
                    content: "promoted content\n".to_string(),
                },
                mode: SaveMode::Promote,
            }]),
            _ => LoopAction::Done(json!({"completed": true})),
        }
    }
}

struct ParallelToolAndSaveFactory;
impl AgentLoopFactory for ParallelToolAndSaveFactory {
    fn id(&self) -> &str {
        "parallel_save_loop"
    }
    fn digest(&self) -> &str {
        "psl_v1"
    }
    fn create(&self, _config: &LoopConfig, _registry: &ExtensionRegistry) -> Box<dyn AgentLoop> {
        Box::new(ParallelToolAndSaveLoop { step_idx: 0 })
    }
}

struct BigEchoTool;
impl ToolHandler for BigEchoTool {
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        _ctx: &'a ToolExecutionContext,
        on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            let size = args.get("size").and_then(Value::as_u64).unwrap_or(10) as usize;
            // Split host secret across two consecutive streaming deltas to test cross-delta redaction
            on_delta("tool_output", json!("emitting secret_xyz_"));
            on_delta("tool_output", json!("999999 done"));
            let mut body = "a".repeat(size);
            body.push_str(" secret_xyz_999999");
            Ok(ToolOutcome {
                output: Value::String(body),
                is_error: false,
            })
        })
    }
}

struct SlowSleepTool;
impl ToolHandler for SlowSleepTool {
    fn execute<'a>(
        &'a self,
        _args: &'a Value,
        ctx: &'a ToolExecutionContext,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            tokio::select! {
                () = ctx.cancel.cancelled() => {
                    Ok(ToolOutcome {
                        output: Value::String("aborted".to_string()),
                        is_error: true,
                    })
                }
                () = tokio::time::sleep(Duration::from_secs(5)) => {
                    Ok(ToolOutcome {
                        output: Value::String("finished".to_string()),
                        is_error: false,
                    })
                }
            }
        })
    }
}

struct SteerableSlowLoop(bool);
impl AgentLoop for SteerableSlowLoop {
    fn id(&self) -> &str {
        "slow_loop"
    }
    fn digest(&self) -> &str {
        "slow_v1"
    }
    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        let steered = ctx.drain_steering();
        if !steered.is_empty() {
            return LoopAction::Done(json!({ "redirected": steered[0].content }));
        }
        if !self.0 || results.is_none() {
            self.0 = true;
            LoopAction::Yield(vec![Effect::Tool {
                name: "slow_tool".to_string(),
                args: json!({}),
                side_effect: SideEffectClass::Read,
            }])
        } else {
            LoopAction::Done(json!({ "finished": true }))
        }
    }
}

struct SlowLoopFactory;
impl AgentLoopFactory for SlowLoopFactory {
    fn id(&self) -> &str {
        "slow_loop"
    }
    fn digest(&self) -> &str {
        "slow_v1"
    }
    fn create(&self, _config: &LoopConfig, _registry: &ExtensionRegistry) -> Box<dyn AgentLoop> {
        Box::new(SteerableSlowLoop(false))
    }
}

struct VerifierDecisionRule;
impl PolicyRule for VerifierDecisionRule {
    fn id(&self) -> &str {
        "decision_verifier"
    }
    fn evaluate(&self, env: &EffectEnvelope, _session: &SessionSlice) -> PolicyDecision {
        if matches!(env.effect, Effect::Tool { .. }) {
            PolicyDecision::RequiresVerification {
                rule_id: self.id().to_string(),
                verifier_effect: Box::new(Effect::Model {
                    target: "mock_dec/v1".to_string(),
                    kind: ModelKind::Decision,
                    input: json!({"check": "tool"}),
                    schema: None,
                }),
                pending_effect: Box::new(env.clone()),
            }
        } else {
            PolicyDecision::Allow
        }
    }
    fn evaluate_verification(
        &self,
        _pending: &EffectEnvelope,
        verifier_result: &StepResult,
    ) -> PolicyDecision {
        if let StepResult::Model { output, .. } = verifier_result
            && output.get("allow").and_then(Value::as_bool) == Some(true)
        {
            PolicyDecision::Allow
        } else {
            PolicyDecision::Deny {
                reason: "verifier denied tool".to_string(),
            }
        }
    }
}

struct MockDecisionProvider {
    fail_with_timeout: bool,
}
impl ModelProvider for MockDecisionProvider {
    fn id(&self) -> &str {
        "mock_dec"
    }
    fn capabilities(&self, _model: &str) -> ModelCapabilities {
        ModelCapabilities::default()
    }
    fn invoke<'a>(
        &'a self,
        req: &'a ModelRequest,
        _ctx: &'a ProviderContext<'a>,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>> {
        let fail = self.fail_with_timeout;
        Box::pin(async move {
            if fail {
                Err(RuntimeError::ProviderError {
                    kind: gestalt_runtime::ProviderErrorKind::Retryable,
                    message: "decision model timed out".to_string(),
                })
            } else {
                Ok(ModelResponse {
                    kind: req.kind,
                    output: json!({ "allow": true }),
                    usage: Usage::default(),
                })
            }
        })
    }
}

#[tokio::test]
async fn runtime_host_parallel_spill_save_rollback_redaction_and_replay() {
    let dir = std::env::temp_dir().join(format!("gestalt_rt_int_1_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".gestalt/protected")).expect("mkdir");
    let canon = fs::canonicalize(&dir).expect("canon");

    let mut reg = ExtensionRegistry::new();
    reg.builders
        .insert("dummy_builder".to_string(), Box::new(DummyBuilder));
    reg.loops.insert(
        "parallel_save_loop".to_string(),
        Box::new(ParallelToolAndSaveFactory),
    );
    reg.tools.insert(
        "big_echo".to_string(),
        ToolSpec {
            name: "big_echo".to_string(),
            description: "echo large text".to_string(),
            parameters_schema: json!({
                "type": "object",
                "required": ["size"],
                "properties": { "size": { "type": "integer" } }
            }),
            default_side_effect: SideEffectClass::Read,
            source_extension: None,
        },
    );
    reg.presets.insert(
        "test_preset".to_string(),
        Preset {
            base_loop: "parallel_save_loop".to_string(),
            decorators: Vec::new(),
            builder: "dummy_builder".to_string(),
            config: LoopConfig::default(),
        },
    );

    let mut runtime = RuntimeRegistry::new(default_sandbox_driver(true));
    runtime
        .tools
        .insert("big_echo".to_string(), Arc::new(BigEchoTool));

    let mut creds = CredentialStore::default();
    creds.insert_runtime_secret("mock", "secret_xyz_999999");

    let sink = Arc::new(CollectingSink::default());
    let session_path = canon.join(".gestalt/sessions/s1.jsonl");
    let mut host = RuntimeHost::new(
        "run1".to_string(),
        &canon,
        session_path.clone(),
        reg,
        PolicyGate::default(),
        runtime,
        creds,
        Some(sink.clone()),
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host init");

    // Advisory .lock contention check while host is alive
    let lock_err = SessionLockGuard::acquire(&session_path).expect_err("must be locked");
    assert!(matches!(lock_err, RuntimeError::SessionLocked { .. }));

    let outcome = host
        .run_turn(json!("run"), "test_preset", &[], CancellationToken::new())
        .await
        .expect("run_turn");
    assert!(matches!(outcome, TurnOutcome::Completed { .. }));

    // Secret must never appear in persisted SessionTree JSONL or EventStream (even split across deltas)
    let jsonl = fs::read_to_string(&session_path).expect("read jsonl");
    assert!(!jsonl.contains("secret_xyz_999999"));
    assert!(jsonl.contains("[REDACTED]"));
    {
        let events = sink.events.lock().expect("events");
        let events_json = serde_json::to_string(&*events).expect("ser events");
        assert!(!events_json.contains("secret_xyz_999999"));
    }

    // Verify promoted artifact & rollback
    let note_path = canon.join(".gestalt/protected/note.md");
    assert_eq!(
        fs::read_to_string(&note_path).expect("note"),
        "promoted content\n"
    );
    let ledger = host.artifacts().read_ledger().expect("ledger");
    assert_eq!(ledger.len(), 1);
    host.rollback_stage(&ledger[0].stage_id).expect("rollback");
    assert!(!note_path.exists());

    // Offline replay parity check
    let (loaded_tree, warn) = SessionTree::from_jsonl_str(&jsonl).expect("parse session");
    assert!(warn.is_none());
    let report = ReplayEngine::new(host.kernel().registry(), host.kernel().policy_gate())
        .replay(&loaded_tree, ReplayMode::Default, |d| {
            host.blob_store().get_utf8(d).ok()
        })
        .expect("replay");
    assert!(report.is_parity(), "divergences: {:?}", report.divergences);

    host.shutdown().await;

    // Verify truncated-tail crash recovery repairs file on open_session
    let mut corrupted = fs::read_to_string(&session_path).expect("read");
    corrupted.push_str("{\"id\":\"n_partial\",\"parent_id\":");
    fs::write(&session_path, corrupted).expect("write truncated tail");

    let repaired_host = RuntimeHost::open_session(
        "run1_resume".to_string(),
        &canon,
        session_path.clone(),
        ExtensionRegistry::new(),
        PolicyGate::default(),
        RuntimeRegistry::new(default_sandbox_driver(true)),
        CredentialStore::default(),
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("open_session repairs tail");
    let after_repair = fs::read_to_string(&session_path).expect("read repaired");
    let (_, warn_after) = SessionTree::from_jsonl_str(&after_repair).expect("clean jsonl");
    assert!(warn_after.is_none());
    repaired_host.shutdown().await;

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn runtime_host_preemptive_abort_urgent_steer_and_verification_fail_closed() {
    let dir = std::env::temp_dir().join(format!("gestalt_rt_int_2_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    let canon = fs::canonicalize(&dir).expect("canon");

    // 1. Preemptive UrgentSteer cancels slow tool <= 250ms and pivots loop to Done
    let mut reg = ExtensionRegistry::new();
    reg.builders
        .insert("dummy_builder".to_string(), Box::new(DummyBuilder));
    reg.loops
        .insert("slow_loop".to_string(), Box::new(SlowLoopFactory));
    reg.tools.insert(
        "slow_tool".to_string(),
        ToolSpec {
            name: "slow_tool".to_string(),
            description: "sleeps".to_string(),
            parameters_schema: json!({"type": "object"}),
            default_side_effect: SideEffectClass::Read,
            source_extension: None,
        },
    );

    let mut runtime = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
    runtime
        .tools
        .insert("slow_tool".to_string(), Arc::new(SlowSleepTool));

    let session_path = canon.join(".gestalt/sessions/steer.jsonl");
    let mut host = RuntimeHost::new(
        "run_steer".to_string(),
        &canon,
        session_path.clone(),
        reg,
        PolicyGate::default(),
        runtime,
        CredentialStore::default(),
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");

    let (tx, rx) = mpsc::unbounded_channel();
    let started = Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = tx.send(LiveSteeringMessage {
            kind: SteeringKind::UrgentSteer,
            content: json!("pivot now"),
            source: SteeringSource::User,
        });
    });
    let outcome = host
        .run_turn_with_steering(
            json!("go"),
            "slow_loop",
            &[],
            CancellationToken::new(),
            Some(rx),
        )
        .await
        .expect("urgent steer turn");
    assert!(started.elapsed() <= Duration::from_millis(250));
    match outcome {
        TurnOutcome::Completed { output, .. } => {
            assert_eq!(output, json!({ "redirected": "pivot now" }));
        }
        other => panic!("expected redirected completion, got {other:?}"),
    }
    let jsonl = fs::read_to_string(&session_path).expect("jsonl");
    assert!(jsonl.contains("\"status\":\"aborted\""));
    host.shutdown().await;

    // 2. RequiresVerification fails closed when decision provider errors
    let mut reg2 = ExtensionRegistry::new();
    reg2.builders
        .insert("dummy_builder".to_string(), Box::new(DummyBuilder));
    reg2.tools.insert(
        "slow_tool".to_string(),
        ToolSpec {
            name: "slow_tool".to_string(),
            description: "sleeps".to_string(),
            parameters_schema: json!({"type": "object"}),
            default_side_effect: SideEffectClass::Read,
            source_extension: None,
        },
    );

    struct OneShotToolLoop(bool);
    impl AgentLoop for OneShotToolLoop {
        fn id(&self) -> &str {
            "oneshot"
        }
        fn digest(&self) -> &str {
            "os1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            if !self.0 {
                self.0 = true;
                LoopAction::Yield(vec![Effect::Tool {
                    name: "slow_tool".to_string(),
                    args: json!({}),
                    side_effect: SideEffectClass::Read,
                }])
            } else {
                let res = results.unwrap_or_default();
                assert!(matches!(&res[0], StepResult::Denied { .. }));
                LoopAction::Done(json!({"denied": true}))
            }
        }
    }
    struct OneShotFactory;
    impl AgentLoopFactory for OneShotFactory {
        fn id(&self) -> &str {
            "oneshot"
        }
        fn digest(&self) -> &str {
            "os1"
        }
        fn create(&self, _cfg: &LoopConfig, _reg: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(OneShotToolLoop(false))
        }
    }

    reg2.loops
        .insert("oneshot".to_string(), Box::new(OneShotFactory));
    let mut gate2 = PolicyGate::default();
    gate2.add_rule(Box::new(VerifierDecisionRule));

    let mut rt2 = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
    rt2.providers
        .register_provider(Arc::new(MockDecisionProvider {
            fail_with_timeout: true,
        }));

    let mut host2 = RuntimeHost::new(
        "run_verify".to_string(),
        &canon,
        canon.join(".gestalt/sessions/verify.jsonl"),
        reg2,
        gate2,
        rt2,
        CredentialStore::default(),
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host2");
    let out2 = host2
        .run_turn(json!("verify"), "oneshot", &[], CancellationToken::new())
        .await
        .expect("verify turn");
    assert_eq!(
        out2,
        TurnOutcome::Completed {
            turn_id: "id_run_verify_2".to_string(),
            output: json!({"denied": true}),
            usage: Usage::default()
        }
    );
    host2.shutdown().await;
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn real_spawn_execution_and_worktree_lifecycle() {
    let dir = std::env::temp_dir().join(format!("gestalt_wt_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    let canon = fs::canonicalize(&dir).expect("canon");

    // Non-git directory fails cleanly
    let err = create_worktree(&canon, "sp1")
        .await
        .expect_err("non-git must fail");
    assert!(matches!(err, RuntimeError::WorktreeError(_)));

    // Initialize git repo
    assert!(
        Command::new("git")
            .args(["init"])
            .current_dir(&canon)
            .status()
            .expect("git init")
            .success()
    );
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&canon)
        .status()
        .expect("git email");
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(&canon)
        .status()
        .expect("git name");
    fs::write(canon.join("README.md"), "init\n").expect("write");
    Command::new("git")
        .args(["add", "."])
        .current_dir(&canon)
        .status()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(&canon)
        .status()
        .expect("git commit");

    // 1. Direct worktree helper check
    let wt_path = create_worktree(&canon, "sp_wt_1")
        .await
        .expect("create worktree");
    assert!(wt_path.exists());
    fs::write(wt_path.join("README.md"), "modified in worktree\n").expect("modify wt");
    assert_eq!(
        fs::read_to_string(canon.join("README.md")).expect("orig"),
        "init\n"
    );

    let blobs = gestalt_runtime::BlobStore::new(&canon);
    let digest = capture_worktree_diff_blob(&wt_path, &blobs)
        .await
        .expect("diff")
        .expect("diff digest");
    let diff_text = blobs.get_utf8(&digest).expect("read diff");
    assert!(diff_text.contains("modified in worktree"));

    fs::write(wt_path.join("README.md"), "init\n").expect("revert");
    assert_eq!(prune_clean_worktrees(&canon).await, 1);

    // 2. Real Effect::Spawn inside RuntimeHost executes child loop in isolated Worktree
    struct ChildWorkerLoop;
    impl AgentLoop for ChildWorkerLoop {
        fn id(&self) -> &str {
            "child_worker"
        }
        fn digest(&self) -> &str {
            "cw1"
        }
        fn step(&mut self, _ctx: &mut dyn LoopContext, _r: Option<Vec<StepResult>>) -> LoopAction {
            LoopAction::Done(json!({ "child_answer": 42 }))
        }
    }
    struct ChildWorkerFactory;
    impl AgentLoopFactory for ChildWorkerFactory {
        fn id(&self) -> &str {
            "child_worker"
        }
        fn digest(&self) -> &str {
            "cw1"
        }
        fn create(&self, _cfg: &LoopConfig, _reg: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(ChildWorkerLoop)
        }
    }
    struct ParentSpawnerLoop(bool);
    impl AgentLoop for ParentSpawnerLoop {
        fn id(&self) -> &str {
            "parent_spawner"
        }
        fn digest(&self) -> &str {
            "ps1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            if !self.0 {
                self.0 = true;
                LoopAction::Yield(vec![Effect::Spawn {
                    loop_id: "child_worker".to_string(),
                    input: json!("compute 42"),
                    budget: BudgetCap::default(),
                    depth_limit: 1,
                    workspace_mode: WorkspaceMode::Worktree,
                    config: None,
                }])
            } else {
                let res = results.unwrap_or_default();
                match &res[0] {
                    StepResult::SpawnCompleted {
                        output,
                        worktree_path,
                        ..
                    } => {
                        assert!(worktree_path.is_some());
                        LoopAction::Done(output.clone())
                    }
                    other => panic!("unexpected spawn result: {other:?}"),
                }
            }
        }
    }
    struct ParentSpawnerFactory;
    impl AgentLoopFactory for ParentSpawnerFactory {
        fn id(&self) -> &str {
            "parent_spawner"
        }
        fn digest(&self) -> &str {
            "ps1"
        }
        fn create(&self, _cfg: &LoopConfig, _reg: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(ParentSpawnerLoop(false))
        }
    }

    let mut reg = ExtensionRegistry::new();
    reg.builders
        .insert("dummy_builder".to_string(), Box::new(DummyBuilder));
    reg.loops
        .insert("child_worker".to_string(), Box::new(ChildWorkerFactory));
    reg.loops
        .insert("parent_spawner".to_string(), Box::new(ParentSpawnerFactory));

    let mut host = RuntimeHost::new(
        "run_spawn".to_string(),
        &canon,
        canon.join(".gestalt/sessions/spawn.jsonl"),
        reg,
        PolicyGate::default(),
        RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver)),
        CredentialStore::default(),
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("spawn host");
    let out = host
        .run_turn(
            json!("spawn child"),
            "parent_spawner",
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("spawn turn");
    match out {
        TurnOutcome::Completed { output, .. } => {
            assert_eq!(output, json!({ "child_answer": 42 }));
        }
        other => panic!("unexpected parent outcome: {other:?}"),
    }
    host.shutdown().await;
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[tokio::test]
async fn reexecute_ext_offline_replay_and_headless_pause_denied() {
    let dir = std::env::temp_dir().join(format!("gestalt_reexec_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".gestalt")).expect("mkdir");
    let canon = fs::canonicalize(&dir).expect("canon");

    let script = canon.join("ext_loop.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"extension/describe"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"name":"ext_replay","loops":[{"id":"rloop","digest":"rd1"}],"builders":[{"id":"rbuild","digest":"rb1"}]}}\n' "$id"
      ;;
    *'"method":"loop/step"'*)
      case "$line" in
        *'"results":null'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"action":{"action":"yield","payload":[{"op":"pause","reason":"approval","payload":{"action":"confirm"}}]},"state_token":{"s":1}}}\n' "$id"
          ;;
        *)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"action":{"action":"done","payload":{"ok":true}},"state_token":{"s":2}}}\n' "$id"
          ;;
      esac
      ;;
    *'"method":"extension/shutdown"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      exit 0
      ;;
  esac
done
"#,
    )
    .expect("write script");

    let ws_toml = canon.join(".gestalt/gestalt.toml");
    fs::write(
        &ws_toml,
        format!(
            "[defaults]\npreset = \"rloop\"\n\n[extensions.ext_replay]\ncommand = [\"sh\", \"{}\"]\nprotocol = \"rpc\"\ntimeout_ms = 2000\n",
            script.display()
        ),
    )
    .expect("write toml");

    let trust_store = WorkspaceTrustStore::default();
    let mut gate = PolicyGate::default();
    let mut reg = ExtensionRegistry::new();
    let resolved = load_and_merge_config(
        ConfigLoadOptions {
            workspace_root: &canon,
            global_toml_path: None,
            workspace_toml_path: Some(&ws_toml),
            cli_extensions: BTreeMap::new(),
            trust_store: &trust_store,
            auto_trust_workspace: true,
        },
        &mut gate,
        &mut reg,
    )
    .expect("load config");

    let creds = CredentialStore::default();
    let mut runtime = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
    runtime
        .connect_configured_extensions(&canon, &resolved, &creds.redactor(), &mut gate, &mut reg)
        .await
        .expect("connect extensions before boot");

    let mut host = RuntimeHost::new(
        "run_reexec".to_string(),
        &canon,
        canon.join(".gestalt/sessions/reexec.jsonl"),
        reg,
        gate,
        runtime,
        creds,
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");

    let pause_err = host
        .run_turn(json!("go"), "rloop", &[], CancellationToken::new())
        .await
        .expect_err("headless pause denied");
    assert!(matches!(pause_err, RuntimeError::HeadlessPauseDenied(_)));

    let mut tree = SessionTree::new();
    let _ = tree.append(
        "n0".into(),
        None,
        None,
        1,
        &gestalt_kernel::NodePayload::SessionOpen {
            schema_version: 1,
            workspace_root: canon.to_string_lossy().to_string(),
            created_at_ms: 1,
        },
    );
    let _ = tree.append(
        "n1".into(),
        Some("t1".into()),
        Some("p1".into()),
        2,
        &gestalt_kernel::NodePayload::PhaseOpen {
            phase_id: "p1".into(),
            preset_id: None,
            base_loop_id: "rloop".into(),
            loop_digest: "rd1".into(),
            decorator_digests: vec![],
            builder_id: "rbuild".into(),
            builder_digest: "rb1".into(),
            config: LoopConfig::default(),
        },
    );
    let _ = tree.append(
        "n2".into(),
        Some("t1".into()),
        Some("p1".into()),
        3,
        &gestalt_kernel::NodePayload::Effect {
            envelope: EffectEnvelope {
                effect_id: "e1".into(),
                effect: Effect::Pause {
                    reason: gestalt_kernel::PauseReason::Approval,
                    payload: json!({"action": "confirm"}),
                },
            },
            state_token: None,
        },
    );

    let sb_drv: Arc<dyn SandboxDriver> = Arc::new(UnsandboxedFallbackDriver);
    let report = replay_session_tree(
        &tree,
        host.kernel().registry(),
        host.kernel().policy_gate(),
        host.blob_store(),
        ReplayMode::ReexecuteExternal,
        Some(ExternalReplayOptions {
            ext_configs: &resolved.extensions,
            workspace_root: &canon,
            sandbox_driver: &sb_drv,
        }),
    )
    .await
    .expect("reexecute replay");
    assert!(report.is_parity(), "divergences: {:?}", report.divergences);

    let bridge = ExternalPolicyBridgeRule::new("ext_pol");
    let dec = bridge.evaluate_verification(
        &EffectEnvelope {
            effect_id: "e9".into(),
            effect: Effect::Pause {
                reason: gestalt_kernel::PauseReason::Approval,
                payload: Value::Null,
            },
        },
        &StepResult::Denied {
            effect_id: "v1".into(),
            reason: "timeout".into(),
        },
    );
    assert!(matches!(dec, PolicyDecision::Deny { .. }));

    host.shutdown().await;
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn tool_execution_context_emits_redacted_tool_ui_events() {
    struct UiEmittingTool;
    impl ToolHandler for UiEmittingTool {
        fn execute<'a>(
            &'a self,
            _args: &'a Value,
            ctx: &'a ToolExecutionContext,
            _on_delta: StreamDeltaFn<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
            Box::pin(async move {
                ctx.emit_tool_ui(
                    "diff_preview",
                    json!({ "patch": "secret_ui_token_12345 updated" }),
                );
                Ok(ToolOutcome {
                    output: json!({"ok": true}),
                    is_error: false,
                })
            })
        }
    }

    struct UiLoop(bool);
    impl AgentLoop for UiLoop {
        fn id(&self) -> &str {
            "ui_loop"
        }
        fn digest(&self) -> &str {
            "ui_v1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            _results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            if !self.0 {
                self.0 = true;
                LoopAction::Yield(vec![Effect::Tool {
                    name: "ui_tool".into(),
                    args: json!({}),
                    side_effect: SideEffectClass::Read,
                }])
            } else {
                LoopAction::Done(json!({"done": true}))
            }
        }
    }
    struct UiLoopFactory;
    impl AgentLoopFactory for UiLoopFactory {
        fn id(&self) -> &str {
            "ui_loop"
        }
        fn digest(&self) -> &str {
            "ui_v1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(UiLoop(false))
        }
    }

    let dir = std::env::temp_dir().join(format!("gestalt_tool_ui_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    let canon = fs::canonicalize(&dir).expect("canon");

    let mut reg = ExtensionRegistry::new();
    reg.builders
        .insert("dummy_builder".into(), Box::new(DummyBuilder));
    reg.loops.insert("ui_loop".into(), Box::new(UiLoopFactory));
    reg.tools.insert(
        "ui_tool".into(),
        ToolSpec {
            name: "ui_tool".into(),
            description: "emits tool ui".into(),
            parameters_schema: json!({"type": "object"}),
            default_side_effect: SideEffectClass::Read,
            source_extension: None,
        },
    );

    let mut rt = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
    rt.tools.insert("ui_tool".into(), Arc::new(UiEmittingTool));

    let mut creds = CredentialStore::default();
    creds.insert_runtime_secret("ui", "secret_ui_token_12345");

    let sink = Arc::new(CollectingSink::default());
    let mut host = RuntimeHost::new(
        "run_ui".into(),
        &canon,
        canon.join(".gestalt/sessions/ui.jsonl"),
        reg,
        PolicyGate::default(),
        rt,
        creds,
        Some(sink.clone()),
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");

    let out = host
        .run_turn(json!("go"), "ui_loop", &[], CancellationToken::new())
        .await
        .expect("turn");
    assert!(matches!(out, TurnOutcome::Completed { .. }));

    {
        let events = sink.events.lock().expect("lock");
        let tool_ui = events
            .iter()
            .find_map(|e| match e {
                RunEvent::ToolUi { renderer, data, .. } => Some((renderer, data)),
                _ => None,
            })
            .expect("expected RunEvent::ToolUi");
        assert_eq!(tool_ui.0, "diff_preview");
        let data_str = tool_ui.1.to_string();
        assert!(!data_str.contains("secret_ui_token_12345"));
        assert!(data_str.contains("[REDACTED]"));
    }

    host.shutdown().await;
    let _ = fs::remove_dir_all(&dir);
}
