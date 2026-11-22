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
    ReplayMode, RunEvent, SandboxPolicy, SaveMode, SessionSlice, SessionTree, SideEffectClass,
    SteeringKind, SteeringSource, StepResult, ToolSpec, Usage, WorkspaceMode,
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
        .insert("dummy_builder".to_string(), Arc::new(DummyBuilder));
    reg.loops.insert(
        "parallel_save_loop".to_string(),
        Arc::new(ParallelToolAndSaveFactory),
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
            builder_layers: Vec::new(),
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
        .insert("dummy_builder".to_string(), Arc::new(DummyBuilder));
    reg.loops
        .insert("slow_loop".to_string(), Arc::new(SlowLoopFactory));
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
        .insert("dummy_builder".to_string(), Arc::new(DummyBuilder));
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
        .insert("oneshot".to_string(), Arc::new(OneShotFactory));
    let mut gate2 = PolicyGate::default();
    gate2.add_rule(Arc::new(VerifierDecisionRule));

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
                    detached: false,
                    notify_on_complete: Default::default(),
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
        .insert("dummy_builder".to_string(), Arc::new(DummyBuilder));
    reg.loops
        .insert("child_worker".to_string(), Arc::new(ChildWorkerFactory));
    reg.loops
        .insert("parent_spawner".to_string(), Arc::new(ParentSpawnerFactory));

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
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"extension/describe"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"name":"ext_replay","loops":[{"id":"rloop","digest":"rd1"}],"builders":[{"id":"rbuild","digest":"rb1"}]}}\n' "$id"
      ;;
    *'"method":"builder/build"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
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
            builder_layer_digests: vec![],
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
        .insert("dummy_builder".into(), Arc::new(DummyBuilder));
    reg.loops.insert("ui_loop".into(), Arc::new(UiLoopFactory));
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

#[cfg(unix)]
#[tokio::test]
async fn docker_and_microvm_sandbox_drivers_enforce_fail_closed_and_abort() {
    use gestalt_runtime::{
        DockerSandboxConfig, DockerSandboxDriver, MicroVmBackend, MicroVmSandboxConfig,
        MicroVmSandboxDriver, SandboxCommand, SandboxKind, select_sandbox_driver,
    };

    let dir = std::env::temp_dir().join(format!("gestalt_m21_sb_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".gestalt/memories")).expect("mkdir");
    let canon = fs::canonicalize(&dir).expect("canon");

    // 1. Missing binaries/assets fail closed
    let missing_docker = DockerSandboxDriver::with_binary("/nonexistent/docker_bin");
    let missing_vm = MicroVmSandboxDriver::new();
    let sample_cmd = SandboxCommand {
        program: "echo".to_string(),
        args: vec!["ok".to_string()],
        cwd: canon.clone(),
        env: BTreeMap::new(),
        stdin_bytes: None,
        timeout_ms: Some(1_000),
    };
    assert!(matches!(
        missing_docker.build_os_command(&sample_cmd, &canon, &SandboxPolicy::default()),
        Err(RuntimeError::SandboxUnavailable { .. })
    ));
    assert!(matches!(
        missing_vm.build_os_command(&sample_cmd, &canon, &SandboxPolicy::default()),
        Err(RuntimeError::SandboxUnavailable { .. })
    ));

    // 2. Mock docker CLI verifies protected_paths ro mounts, --network none, and <=250ms abort
    let mock_docker = canon.join("mock_docker.sh");
    fs::write(
        &mock_docker,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "Docker version 27.0.0"
  exit 0
fi
if [ "$1" = "rm" ]; then
  exit 0
fi
for arg in "$@"; do
  if [ "$arg" = "sleep_test" ]; then
    sleep 10
    exit 0
  fi
done
printf '%s\n' "$*"
exit 0
"#,
    )
    .expect("write mock docker");
    Command::new("chmod")
        .args(["+x", mock_docker.to_str().expect("utf8")])
        .status()
        .expect("chmod");

    let docker_drv = select_sandbox_driver(
        SandboxKind::Docker,
        Some(DockerSandboxConfig {
            binary: mock_docker.to_string_lossy().to_string(),
            image: "alpine:3.20".to_string(),
            memory_limit: Some("256m".to_string()),
            cpu_limit: Some("1.0".to_string()),
            extra_args: Vec::new(),
        }),
        None,
        false,
    );
    assert_eq!(docker_drv.name(), "docker");
    let out = docker_drv
        .run(
            &sample_cmd,
            &canon,
            &SandboxPolicy {
                network: false,
                protected_paths: [".gestalt/memories/".to_string()].into_iter().collect(),
                max_spawn_depth: 1,
            },
            CancellationToken::new(),
        )
        .await
        .expect("mock docker run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--network none"));
    assert!(stdout.contains(".gestalt/memories:ro"));
    assert!(stdout.contains("--cap-drop ALL"));

    // Fast abort <= 250ms
    let sleep_cmd = SandboxCommand {
        program: "sleep_test".to_string(),
        args: Vec::new(),
        cwd: canon.clone(),
        env: BTreeMap::new(),
        stdin_bytes: None,
        timeout_ms: None,
    };
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let t0 = Instant::now();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        cancel_clone.cancel();
    });
    let aborted = docker_drv
        .run(&sleep_cmd, &canon, &SandboxPolicy::default(), cancel)
        .await
        .expect("docker abort");
    assert!(aborted.aborted);
    assert!(t0.elapsed() <= Duration::from_secs(2));

    // 3. Mock Firecracker VMM runner verifies config generation and execution
    let kernel = canon.join("vmlinux");
    let rootfs = canon.join("rootfs.ext4");
    fs::write(&kernel, "k").expect("k");
    fs::write(&rootfs, "r").expect("r");
    let mock_fc = canon.join("mock_fc.sh");
    fs::write(
        &mock_fc,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  echo "Firecracker v1.9.0"
  exit 0
fi
cat "$3"
exit 0
"#,
    )
    .expect("write mock fc");
    Command::new("chmod")
        .args(["+x", mock_fc.to_str().expect("utf8")])
        .status()
        .expect("chmod fc");

    let fc_drv = select_sandbox_driver(
        SandboxKind::MicroVm,
        None,
        Some(MicroVmSandboxConfig {
            backend: MicroVmBackend::Firecracker,
            binary: mock_fc.to_string_lossy().to_string(),
            kernel_image: Some(kernel),
            rootfs_image: Some(rootfs),
            vcpu_count: 1,
            mem_mib: 128,
        }),
        false,
    );
    assert_eq!(fc_drv.name(), "microvm");
    let fc_out = fc_drv
        .run(
            &sample_cmd,
            &canon,
            &SandboxPolicy::default(),
            CancellationToken::new(),
        )
        .await
        .expect("mock fc run");
    let fc_json: Value = serde_json::from_slice(&fc_out.stdout).expect("fc json");
    assert_eq!(fc_json["network-interfaces"], json!([]));
    assert_eq!(fc_json["drives"][0]["is_read_only"], true);

    // 4. Monotonic sandbox driver downgrade in workspace config is rejected
    let global_toml = canon.join("global_sb.toml");
    let ws_toml = canon.join("ws_sb.toml");
    fs::write(&global_toml, "[sandbox]\ndriver = \"docker\"\n").expect("write global");
    fs::write(&ws_toml, "[sandbox]\ndriver = \"os\"\n").expect("write ws");
    let mut gate = PolicyGate::default();
    let mut reg = ExtensionRegistry::new();
    let trust = WorkspaceTrustStore::default();
    let err = load_and_merge_config(
        ConfigLoadOptions {
            workspace_root: &canon,
            global_toml_path: Some(&global_toml),
            workspace_toml_path: Some(&ws_toml),
            cli_extensions: BTreeMap::new(),
            trust_store: &trust,
            auto_trust_workspace: true,
        },
        &mut gate,
        &mut reg,
    )
    .expect_err("must reject sandbox driver downgrade");
    assert!(matches!(
        err,
        RuntimeError::Kernel(gestalt_kernel::KernelError::PolicyRelaxationForbidden(_))
    ));

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn spawned_child_inherits_builders_presets_switch_and_parent_policy_rules() {
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, ContextBuilder, EffectEnvelope, LoopAction, LoopConfig,
        LoopContext, ModelCapabilities, PolicyDecision, PolicyRule, Preset, SessionSlice,
        SideEffectClass, ToolSpec,
    };
    use gestalt_runtime::{ToolExecutionContext, ToolHandler, ToolOutcome};

    let dir = std::env::temp_dir().join(format!(
        "gestalt_spawn_inherit_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create dir");
    let canon = fs::canonicalize(&dir).expect("canon");

    struct RichBuilder;
    impl ContextBuilder for RichBuilder {
        fn id(&self) -> &str {
            "rich_builder"
        }
        fn digest(&self) -> &str {
            "rb1"
        }
        fn build(
            &self,
            _session: &SessionSlice,
            _artifacts: &gestalt_kernel::ArtifactStoreView,
            _caps: &ModelCapabilities,
        ) -> Value {
            json!({ "injected": "from_parent_builder" })
        }
    }

    struct DenyBashRule;
    impl PolicyRule for DenyBashRule {
        fn id(&self) -> &str {
            "deny_bash"
        }
        fn evaluate(&self, env: &EffectEnvelope, _session: &SessionSlice) -> PolicyDecision {
            if let Effect::Tool { name, .. } = &env.effect
                && name == "bash"
            {
                return PolicyDecision::Deny {
                    reason: "bash blocked by parent rule".to_string(),
                };
            }
            PolicyDecision::Allow
        }
    }

    struct ChildSwitchLoop {
        step_idx: usize,
        saw_context: Option<Value>,
        tool_denied: bool,
    }
    impl AgentLoop for ChildSwitchLoop {
        fn id(&self) -> &str {
            "child_switch"
        }
        fn digest(&self) -> &str {
            "csw1"
        }
        fn step(
            &mut self,
            ctx: &mut dyn LoopContext,
            results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            match self.step_idx {
                0 => {
                    self.step_idx = 1;
                    self.saw_context = Some(ctx.build_context(&ModelCapabilities::default()));
                    LoopAction::Yield(vec![Effect::Tool {
                        name: "bash".to_string(),
                        args: json!({}),
                        side_effect: SideEffectClass::Idempotent,
                    }])
                }
                1 => {
                    self.step_idx = 2;
                    self.tool_denied = matches!(
                        results.and_then(|v| v.into_iter().next()),
                        Some(StepResult::Denied { .. })
                    );
                    LoopAction::Yield(vec![Effect::Switch {
                        next_loop: "child_final".to_string(),
                        handoff: json!({
                            "saw_context": self.saw_context,
                            "tool_denied": self.tool_denied,
                        }),
                        config: None,
                    }])
                }
                _ => LoopAction::Done(Value::Null),
            }
        }
    }
    struct ChildSwitchFactory;
    impl AgentLoopFactory for ChildSwitchFactory {
        fn id(&self) -> &str {
            "child_switch"
        }
        fn digest(&self) -> &str {
            "csw1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(ChildSwitchLoop {
                step_idx: 0,
                saw_context: None,
                tool_denied: false,
            })
        }
    }

    struct ChildFinalLoop;
    impl AgentLoop for ChildFinalLoop {
        fn id(&self) -> &str {
            "child_final"
        }
        fn digest(&self) -> &str {
            "cf1"
        }
        fn step(&mut self, ctx: &mut dyn LoopContext, _r: Option<Vec<StepResult>>) -> LoopAction {
            LoopAction::Done(ctx.handoff().clone())
        }
    }
    struct ChildFinalFactory;
    impl AgentLoopFactory for ChildFinalFactory {
        fn id(&self) -> &str {
            "child_final"
        }
        fn digest(&self) -> &str {
            "cf1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(ChildFinalLoop)
        }
    }

    struct ParentSpawnerLoop {
        spawned: bool,
    }
    impl AgentLoop for ParentSpawnerLoop {
        fn id(&self) -> &str {
            "parent_spawner_inherit"
        }
        fn digest(&self) -> &str {
            "psi1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            if !self.spawned {
                self.spawned = true;
                return LoopAction::Yield(vec![Effect::Spawn {
                    loop_id: "child_preset".to_string(),
                    input: json!("go"),
                    budget: BudgetCap {
                        max_steps: Some(10),
                        max_tokens: Some(10_000),
                        max_cost_micro_usd: None,
                    },
                    depth_limit: 1,
                    workspace_mode: WorkspaceMode::ReadOnly,
                    config: None,
                    detached: false,
                    notify_on_complete: Default::default(),
                }]);
            }
            match results.and_then(|v| v.into_iter().next()) {
                Some(StepResult::SpawnCompleted { output, .. }) => LoopAction::Done(output),
                other => LoopAction::Done(json!({ "unexpected": format!("{other:?}") })),
            }
        }
    }
    struct ParentSpawnerFactory;
    impl AgentLoopFactory for ParentSpawnerFactory {
        fn id(&self) -> &str {
            "parent_spawner_inherit"
        }
        fn digest(&self) -> &str {
            "psi1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(ParentSpawnerLoop { spawned: false })
        }
    }

    struct DummyBash;
    impl ToolHandler for DummyBash {
        fn execute<'a>(
            &'a self,
            _args: &'a Value,
            _ctx: &'a ToolExecutionContext,
            _on_delta: StreamDeltaFn<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>> {
            Box::pin(async move {
                Ok(ToolOutcome {
                    output: json!("ran"),
                    is_error: false,
                })
            })
        }
    }

    let mut reg = ExtensionRegistry::new();
    reg.builders
        .insert("rich_builder".into(), Arc::new(RichBuilder));
    reg.loops
        .insert("child_switch".into(), Arc::new(ChildSwitchFactory));
    reg.loops
        .insert("child_final".into(), Arc::new(ChildFinalFactory));
    reg.loops.insert(
        "parent_spawner_inherit".into(),
        Arc::new(ParentSpawnerFactory),
    );
    reg.presets.insert(
        "child_preset".into(),
        Preset {
            base_loop: "child_switch".into(),
            decorators: vec![],
            builder: "rich_builder".into(),
            builder_layers: vec![],
            config: LoopConfig::default(),
        },
    );
    reg.tools.insert(
        "bash".into(),
        ToolSpec {
            name: "bash".into(),
            description: "bash".into(),
            parameters_schema: json!({"type": "object"}),
            default_side_effect: SideEffectClass::Idempotent,
            source_extension: None,
        },
    );

    let mut rt = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
    rt.tools.insert("bash".into(), Arc::new(DummyBash));

    let mut gate = PolicyGate::new(
        BudgetCap {
            max_steps: Some(20),
            max_tokens: Some(100_000),
            max_cost_micro_usd: None,
        },
        None,
        SandboxPolicy::default(),
    );
    gate.add_rule(Arc::new(DenyBashRule));

    let mut host = RuntimeHost::new(
        "run_spawn_inherit".into(),
        &canon,
        canon.join("session.jsonl"),
        reg,
        gate,
        rt,
        CredentialStore::default(),
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");

    let outcome = host
        .run_turn(
            json!("start"),
            "parent_spawner_inherit",
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("turn");
    match outcome {
        TurnOutcome::Completed { output, .. } => {
            assert_eq!(
                output["saw_context"],
                json!({ "injected": "from_parent_builder" })
            );
            assert_eq!(output["tool_denied"], json!(true));
        }
        other => panic!("expected completed turn, got {other:?}"),
    }

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn configurable_artifacts_writable_roots_and_mid_turn_promote_refresh() {
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, ContextBuilder, LoopAction, LoopConfig, LoopContext,
        ModelCapabilities, Preset, SessionSlice,
    };

    let dir = std::env::temp_dir().join(format!(
        "gestalt_u5_artifacts_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let global_skill_dir = std::env::temp_dir().join(format!(
        "gestalt_u5_global_skills_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&global_skill_dir);
    fs::create_dir_all(&dir).expect("mkdir");
    fs::create_dir_all(&global_skill_dir).expect("mkdir global");
    let canon = fs::canonicalize(&dir).expect("canon");
    let canon_global = fs::canonicalize(&global_skill_dir).expect("canon global");

    fs::write(canon.join("SOUL.md"), "initial soul\n").expect("write soul");
    let global_toml = canon.join("global.toml");
    let ws_toml = canon.join("ws.toml");
    fs::write(
        &global_toml,
        format!(
            "[artifacts]\ntracked_paths = [\"SOUL.md\"]\nwritable_roots = [\"{}\"]\n",
            canon_global.to_string_lossy()
        ),
    )
    .expect("write global");
    fs::write(&ws_toml, "[artifacts]\ntracked_paths = [\"EXTRA.md\"]\n").expect("write ws");

    let mut gate = PolicyGate::default();
    let mut reg = ExtensionRegistry::new();
    let trust = WorkspaceTrustStore::default();
    let resolved = load_and_merge_config(
        ConfigLoadOptions {
            workspace_root: &canon,
            global_toml_path: Some(&global_toml),
            workspace_toml_path: Some(&ws_toml),
            cli_extensions: BTreeMap::new(),
            trust_store: &trust,
            auto_trust_workspace: true,
        },
        &mut gate,
        &mut reg,
    )
    .expect("merge");
    assert_eq!(resolved.tracked_artifact_paths, vec!["SOUL.md", "EXTRA.md"]);

    // Workspace trying to add an unapproved writable_root must fail closed
    let bad_ws_toml = canon.join("bad_ws.toml");
    fs::write(&bad_ws_toml, "[artifacts]\nwritable_roots = [\"/etc\"]\n").expect("write bad ws");
    let mut gate_bad = PolicyGate::default();
    let mut reg_bad = ExtensionRegistry::new();
    let err = load_and_merge_config(
        ConfigLoadOptions {
            workspace_root: &canon,
            global_toml_path: Some(&global_toml),
            workspace_toml_path: Some(&bad_ws_toml),
            cli_extensions: BTreeMap::new(),
            trust_store: &trust,
            auto_trust_workspace: true,
        },
        &mut gate_bad,
        &mut reg_bad,
    )
    .expect_err("workspace writable_roots must be rejected");
    assert!(matches!(
        err,
        RuntimeError::Kernel(gestalt_kernel::KernelError::PolicyRelaxationForbidden(_))
    ));

    // Mid-Turn Save(Promote) refreshes ArtifactStoreView for Step 2
    struct SoulEchoBuilder;
    impl ContextBuilder for SoulEchoBuilder {
        fn id(&self) -> &str {
            "soul_echo"
        }
        fn digest(&self) -> &str {
            "se1"
        }
        fn build(
            &self,
            _session: &SessionSlice,
            artifacts: &ArtifactStoreView,
            _caps: &ModelCapabilities,
        ) -> Value {
            json!({
                "soul": artifacts.get("SOUL.md").map(|e| e.content.clone()),
            })
        }
    }
    struct PromoteAndReadLoop {
        step: usize,
        global_target: String,
    }
    impl AgentLoop for PromoteAndReadLoop {
        fn id(&self) -> &str {
            "promote_read"
        }
        fn digest(&self) -> &str {
            "pr1"
        }
        fn step(&mut self, ctx: &mut dyn LoopContext, _r: Option<Vec<StepResult>>) -> LoopAction {
            if self.step == 0 {
                self.step = 1;
                return LoopAction::Yield(vec![
                    Effect::Save {
                        artifact: "SOUL.md".to_string(),
                        patch: ArtifactPatch::WriteFile {
                            content: "evolved soul\n".to_string(),
                        },
                        mode: SaveMode::Promote,
                    },
                    Effect::Save {
                        artifact: self.global_target.clone(),
                        patch: ArtifactPatch::WriteFile {
                            content: "# Global Skill\n".to_string(),
                        },
                        mode: SaveMode::Promote,
                    },
                ]);
            }
            let ctx_val = ctx.build_context(&ModelCapabilities::default());
            LoopAction::Done(ctx_val)
        }
    }
    struct PromoteAndReadFactory(String);
    impl AgentLoopFactory for PromoteAndReadFactory {
        fn id(&self) -> &str {
            "promote_read"
        }
        fn digest(&self) -> &str {
            "pr1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(PromoteAndReadLoop {
                step: 0,
                global_target: self.0.clone(),
            })
        }
    }

    let global_skill_file = canon_global.join("my_skill.md");
    reg.builders
        .insert("soul_echo".into(), Arc::new(SoulEchoBuilder));
    reg.loops.insert(
        "promote_read".into(),
        Arc::new(PromoteAndReadFactory(
            global_skill_file.to_string_lossy().to_string(),
        )),
    );
    reg.presets.insert(
        "pr_preset".into(),
        Preset {
            base_loop: "promote_read".into(),
            decorators: vec![],
            builder: "soul_echo".into(),
            builder_layers: vec![],
            config: LoopConfig::default(),
        },
    );

    let rt = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
    let mut host = RuntimeHost::new(
        "run_u5".into(),
        &canon,
        canon.join("session.jsonl"),
        reg,
        gate,
        rt,
        CredentialStore::default(),
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");
    host.set_tracked_artifact_paths(resolved.tracked_artifact_paths);
    host.set_writable_artifact_roots(vec![canon_global.clone()]);

    match host
        .run_turn(json!("go"), "pr_preset", &[], CancellationToken::new())
        .await
        .expect("turn")
    {
        TurnOutcome::Completed { output, .. } => {
            assert_eq!(output["soul"], json!("evolved soul\n"));
        }
        other => panic!("expected completed turn, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(&global_skill_file).expect("global skill"),
        "# Global Skill\n"
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&global_skill_dir);
}

#[tokio::test]
async fn detached_spawn_passive_and_opt_in_followup() {
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, LoopAction, LoopConfig, LoopContext, SpawnNotifyMode,
    };

    let dir = std::env::temp_dir().join(format!(
        "gestalt_u7_detached_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    let canon = fs::canonicalize(&dir).expect("canon");

    struct QuickChild;
    impl AgentLoop for QuickChild {
        fn id(&self) -> &str {
            "quick_child"
        }
        fn digest(&self) -> &str {
            "qc1"
        }
        fn step(&mut self, _ctx: &mut dyn LoopContext, _r: Option<Vec<StepResult>>) -> LoopAction {
            LoopAction::Done(json!({ "child": "ok" }))
        }
    }
    struct QuickChildFactory;
    impl AgentLoopFactory for QuickChildFactory {
        fn id(&self) -> &str {
            "quick_child"
        }
        fn digest(&self) -> &str {
            "qc1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(QuickChild)
        }
    }

    struct DetachedParent {
        notify: SpawnNotifyMode,
        yielded: bool,
    }
    impl AgentLoop for DetachedParent {
        fn id(&self) -> &str {
            "det_parent"
        }
        fn digest(&self) -> &str {
            "dp1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            if !self.yielded {
                self.yielded = true;
                return LoopAction::Yield(vec![Effect::Spawn {
                    loop_id: "quick_child".to_string(),
                    input: json!("bg"),
                    budget: BudgetCap {
                        max_steps: Some(4),
                        max_tokens: Some(1_000),
                        max_cost_micro_usd: None,
                    },
                    depth_limit: 1,
                    workspace_mode: WorkspaceMode::ReadOnly,
                    config: None,
                    detached: true,
                    notify_on_complete: self.notify,
                }]);
            }
            match results.and_then(|v| v.into_iter().next()) {
                Some(StepResult::SpawnCompleted {
                    child_head_id,
                    output,
                    ..
                }) => LoopAction::Done(json!({
                    "child_head_id": child_head_id,
                    "output": output,
                })),
                other => panic!("expected SpawnCompleted, got {other:?}"),
            }
        }
    }
    struct DetachedParentFactory(SpawnNotifyMode);
    impl AgentLoopFactory for DetachedParentFactory {
        fn id(&self) -> &str {
            "det_parent"
        }
        fn digest(&self) -> &str {
            "dp1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(DetachedParent {
                notify: self.0,
                yielded: false,
            })
        }
    }

    // 1. Passive default records custom:spawn:completed without queuing FollowUp
    let mut reg = ExtensionRegistry::new();
    reg.builders
        .insert("dummy_builder".into(), Arc::new(DummyBuilder));
    reg.loops
        .insert("quick_child".into(), Arc::new(QuickChildFactory));
    reg.loops.insert(
        "det_passive".into(),
        Arc::new(DetachedParentFactory(SpawnNotifyMode::Passive)),
    );
    reg.loops.insert(
        "det_followup".into(),
        Arc::new(DetachedParentFactory(SpawnNotifyMode::FollowUp)),
    );

    let mut host = RuntimeHost::new(
        "run_det".into(),
        &canon,
        canon.join("session.jsonl"),
        reg,
        PolicyGate::default(),
        RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver)),
        CredentialStore::default(),
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");

    match host
        .run_turn(json!("start"), "det_passive", &[], CancellationToken::new())
        .await
        .expect("turn")
    {
        TurnOutcome::Completed { output, .. } => {
            assert_eq!(output["child_head_id"], json!("detached"));
            assert_eq!(output["output"]["status"], json!("detached"));
        }
        other => panic!("expected completed turn, got {other:?}"),
    }
    for _ in 0..50 {
        if host.poll_detached_spawns().expect("poll") > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        host.kernel().pending_follow_up_count(),
        0,
        "passive detached spawn must not enqueue FollowUp"
    );
    assert!(
        host.kernel()
            .session_tree()
            .all_nodes()
            .iter()
            .any(|n| n.kind == "custom:spawn:completed")
    );

    // 2. Opt-in FollowUp enqueues a SteeringKind::FollowUp
    let _ = host
        .run_turn(
            json!("start2"),
            "det_followup",
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("turn2");
    for _ in 0..50 {
        if host.poll_detached_spawns().expect("poll") > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(host.kernel().pending_follow_up_count(), 1);
    assert_eq!(host.activity().completed_turn_count(), 2);
    assert!(!host.activity().turn_in_progress());

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn ext_ipc_parity_heterogeneous_decorators_triggers_and_external_provider() {
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, LoopAction, LoopConfig, LoopContext, LoopDecorator,
    };

    let dir = std::env::temp_dir().join(format!(
        "gestalt_ext_parity_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".gestalt")).expect("mkdir");
    let canon = fs::canonicalize(&dir).expect("canon");

    let script = canon.join("parity_worker.py");
    fs::write(
        &script,
        r#"#!/usr/bin/env python3
import json, sys

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    req_id = msg.get("id")
    method = msg.get("method")
    params = msg.get("params", {})

    if method == "extension/describe":
        res = {
            "name": "parity_ext",
            "loops": [{"id": "ext_echo_loop", "digest": "el1"}],
            "builders": [{"id": "ext_b", "digest": "eb1"}],
            "builder_layers": [{"id": "ext_layer", "digest": "ely1"}],
            "decorators": [{"id": "ext_dec", "digest": "ed1"}],
            "triggers": [{"id": "t_ext", "kind": "ext_tick", "description": "External tick"}],
            "providers": [{
                "id": "ext_prov",
                "kinds": ["decision"],
                "capabilities": {
                    "max_context_tokens": 32000,
                    "max_output_tokens": 512,
                    "supports_prompt_caching": False,
                    "supports_reasoning": False,
                    "supports_vision": False
                }
            }]
        }
    elif method == "builder/build":
        res = {
            "built_step_nodes": len(params.get("session_slice", [])),
            "caps_max_ctx": params.get("caps", {}).get("max_context_tokens", 0)
        }
    elif method == "builder/layer":
        inp = params.get("input", {})
        inp["layered"] = True
        res = inp
    elif method == "loop/step":
        if params.get("results") is None:
            res = {
                "action": {
                    "action": "yield",
                    "payload": [{
                        "op": "model",
                        "target": "ext_prov/v1",
                        "kind": "decision",
                        "input": params.get("context", {}),
                        "schema": None
                    }]
                },
                "state_token": {"step": 1},
                "custom_nodes": [["ext:trace", {"saw_slice": len(params.get("session_slice", []))}]]
            }
        else:
            res = {
                "action": {
                    "action": "done",
                    "payload": {
                        "context": params.get("context"),
                        "results_len": len(params.get("results", []))
                    }
                }
            }
    elif method == "decorator/wrap_step":
        act = params.get("inner_action", {})
        if act.get("action") == "done" and isinstance(act.get("payload"), dict):
            act["payload"]["wrapped_by_ext_dec"] = True
        res = {"action": act}
    elif method == "model/call":
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "method": "stream/delta",
            "params": {"channel": "text", "payload": "chunk1"}
        }) + "\n")
        sys.stdout.flush()
        res = {
            "kind": "decision",
            "output": {"choice": "ok", "confidence": 0.99},
            "usage": {"input_units": 5, "output_units": 2, "cost_micro_usd": 1}
        }
    elif method == "trigger/start":
        sys.stdout.write(json.dumps({
            "jsonrpc": "2.0",
            "method": "trigger/fire",
            "params": {
                "trigger_id": params.get("trigger_id", "t_ext"),
                "kind": "ext_tick",
                "prompt": "fired from ext",
                "timestamp_ms": 12345
            }
        }) + "\n")
        sys.stdout.flush()
        res = {"started": True}
    elif method == "trigger/stop":
        res = {"stopped": True}
    elif method == "extension/shutdown":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": {}}) + "\n")
        sys.stdout.flush()
        break
    else:
        res = {}

    if req_id is not None:
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req_id, "result": res}) + "\n")
        sys.stdout.flush()
"#,
    )
    .expect("write worker");

    let ws_toml = canon.join(".gestalt/gestalt.toml");
    fs::write(
        &ws_toml,
        format!(
            "[extensions.parity_ext]\nprotocol = \"rpc\"\ncommand = [\"python3\", \"{}\"]\ntimeout_ms = 5000\n\n\
             [presets.ext_with_in_dec]\nbase_loop = \"ext_echo_loop\"\ndecorators = [\"in_tag\"]\nbuilder = \"ext_b\"\nbuilder_layers = [\"ext_layer\"]\nparams = {{ \"model\" = \"ext_prov/v1\" }}\n\n\
             [presets.in_with_ext_dec]\nbase_loop = \"in_done\"\ndecorators = [\"ext_dec\"]\nbuilder = \"ext_b\"\nbuilder_layers = [\"ext_layer\"]\n",
            script.display()
        ),
    )
    .expect("write toml");

    struct InTagDecorator;
    impl LoopDecorator for InTagDecorator {
        fn id(&self) -> &str {
            "in_tag"
        }
        fn digest(&self) -> &str {
            "it1"
        }
        fn wrap(
            &self,
            inner: Box<dyn AgentLoop>,
            _c: &LoopConfig,
            _r: &ExtensionRegistry,
        ) -> Box<dyn AgentLoop> {
            struct Wrapped(Box<dyn AgentLoop>);
            impl AgentLoop for Wrapped {
                fn id(&self) -> &str {
                    self.0.id()
                }
                fn digest(&self) -> &str {
                    self.0.digest()
                }
                fn step(
                    &mut self,
                    ctx: &mut dyn LoopContext,
                    results: Option<Vec<StepResult>>,
                ) -> LoopAction {
                    match self.0.step(ctx, results) {
                        LoopAction::Done(mut val) => {
                            if let Some(obj) = val.as_object_mut() {
                                obj.insert("wrapped_by_in_dec".to_string(), json!(true));
                            }
                            LoopAction::Done(val)
                        }
                        other => other,
                    }
                }
            }
            Box::new(Wrapped(inner))
        }
    }

    struct InDoneFactory;
    impl AgentLoopFactory for InDoneFactory {
        fn id(&self) -> &str {
            "in_done"
        }
        fn digest(&self) -> &str {
            "id1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            struct L;
            impl AgentLoop for L {
                fn id(&self) -> &str {
                    "in_done"
                }
                fn digest(&self) -> &str {
                    "id1"
                }
                fn step(
                    &mut self,
                    _ctx: &mut dyn LoopContext,
                    _r: Option<Vec<StepResult>>,
                ) -> LoopAction {
                    LoopAction::Done(json!({"from_in_loop": true}))
                }
            }
            Box::new(L)
        }
    }

    let mut reg = ExtensionRegistry::new();
    reg.decorators
        .insert("in_tag".into(), Arc::new(InTagDecorator));
    reg.loops.insert("in_done".into(), Arc::new(InDoneFactory));

    let mut gate = PolicyGate::default();
    let trust = WorkspaceTrustStore::default();
    let resolved = load_and_merge_config(
        ConfigLoadOptions {
            workspace_root: &canon,
            global_toml_path: None,
            workspace_toml_path: Some(&ws_toml),
            cli_extensions: BTreeMap::new(),
            trust_store: &trust,
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
        .expect("connect");

    let mut host = RuntimeHost::new(
        "run_parity".into(),
        &canon,
        canon.join(".gestalt/sessions/parity.jsonl"),
        reg,
        gate,
        runtime,
        creds,
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");

    // 1. External loop + in-process decorator + external per-step builder + external layer + external ModelProvider
    match host
        .run_turn(
            json!("go"),
            "ext_with_in_dec",
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("ext_with_in_dec")
    {
        TurnOutcome::Completed { output, .. } => {
            assert_eq!(output["wrapped_by_in_dec"], json!(true));
            assert_eq!(output["results_len"], json!(1));
            assert_eq!(output["context"]["layered"], json!(true));
            assert_eq!(output["context"]["caps_max_ctx"], json!(32000));
            assert!(output["context"]["built_step_nodes"].as_u64().unwrap_or(0) >= 4);
        }
        other => panic!("expected completed, got {other:?}"),
    }
    assert!(
        host.kernel()
            .session_tree()
            .all_nodes()
            .iter()
            .any(|n| n.kind == "custom:ext:trace")
    );

    // 2. In-process loop + external decorator
    match host
        .run_turn(
            json!("go2"),
            "in_with_ext_dec",
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("in_with_ext_dec")
    {
        TurnOutcome::Completed { output, .. } => {
            assert_eq!(output["from_in_loop"], json!(true));
            assert_eq!(output["wrapped_by_ext_dec"], json!(true));
        }
        other => panic!("expected completed, got {other:?}"),
    }

    // 3. External trigger/start + unsolicited trigger/fire notification -> TriggerManager receiver
    let mut rx = host.take_trigger_receiver().await.expect("rx");
    host.start_trigger(gestalt_runtime::TriggerInstanceConfig {
        id: "t_ext".into(),
        kind: "ext_tick".into(),
        enabled: true,
        prompt: json!("ignored"),
        preset: None,
        extra_decorators: vec![],
        config: json!({}),
    })
    .await
    .expect("start_trigger");
    let ev = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("timeout")
        .expect("trigger event");
    assert_eq!(ev.trigger_id, "t_ext");
    assert_eq!(ev.prompt, json!("fired from ext"));

    host.shutdown().await;
    let _ = fs::remove_dir_all(&dir);
}
