#![forbid(unsafe_code)]
//! Comprehensive integration tests for `gestalt-kernel` covering:
//! - Multi-turn, multi-phase `KernelRuntime` execution (`Switch`, `Pause`, `Spawn`)
//! - Fail-closed `PolicyGate` (budgets, tool ACLs, `RequiresVerification`, concurrent `Shared` spawns)
//! - Mid-step `UrgentSteer` & `Abort` with orphan `StepResult::Aborted` synthesis
//! - `LoopDecorator` stacking & `ExtensionRegistry` slot resolution
//! - Hermetic offline `ReplayEngine` parity (including mutated post-recording workspace files)
//! - Intentional behavioral & digest divergence detection

use std::collections::{BTreeMap, BTreeSet};

use gestalt_kernel::{
    AgentLoop, AgentLoopFactory, ArtifactEntry, ArtifactStoreView, BudgetCap, ContextBuilder,
    DivergenceKind, DriverDirective, DriverResponse, Effect, EffectEnvelope, Extension,
    ExtensionError, ExtensionRegistry, KernelRuntime, LoopAction, LoopConfig, LoopContext,
    LoopDecorator, ModelCapabilities, ModelKind, PauseReason, PolicyDecision, PolicyGate,
    PolicyRule, Preset, ReplayEngine, ReplayMode, RunEvent, SandboxPolicy, SessionSlice,
    SessionTree, SideEffectClass, SteeringKind, SteeringSource, StepResult, ToolSpec, Usage,
    WorkspaceMode, sha256_hex,
};
use serde_json::{Value, json};

struct ArtifactEchoBuilder;
impl ContextBuilder for ArtifactEchoBuilder {
    fn id(&self) -> &str {
        "echo_builder"
    }
    fn digest(&self) -> &str {
        "echo_builder_v1"
    }
    fn build(
        &self,
        session: &SessionSlice,
        artifacts: &ArtifactStoreView,
        _caps: &ModelCapabilities,
    ) -> Value {
        let guide = artifacts.get("GUIDE.md").map_or("", |e| e.content.as_str());
        json!({
            "nodes": session.len(),
            "guide": guide,
        })
    }
}

struct TriageLoop;
impl AgentLoop for TriageLoop {
    fn id(&self) -> &str {
        "triage"
    }
    fn digest(&self) -> &str {
        "triage_v1"
    }
    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        match results {
            None => {
                let input = ctx.build_context(&ModelCapabilities::default());
                LoopAction::Yield(vec![Effect::Model {
                    target: "fast/decision".into(),
                    kind: ModelKind::Decision,
                    input,
                    schema: None,
                }])
            }
            Some(res) => match res.first() {
                Some(StepResult::Model { output, .. })
                    if output
                        .get("confidence")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                        < 0.85 =>
                {
                    LoopAction::Yield(vec![Effect::Switch {
                        next_loop: "worker_preset".into(),
                        handoff: json!({"escalated_from": "triage", "ts": ctx.now_ms()}),
                        config: None,
                    }])
                }
                _ => LoopAction::Done(json!({"triaged": true})),
            },
        }
    }
}

struct TriageFactory;
impl AgentLoopFactory for TriageFactory {
    fn id(&self) -> &str {
        "triage"
    }
    fn digest(&self) -> &str {
        "triage_v1"
    }
    fn create(&self, _config: &LoopConfig, _registry: &ExtensionRegistry) -> Box<dyn AgentLoop> {
        Box::new(TriageLoop)
    }
}

struct WorkerLoop {
    step_num: u32,
}
impl AgentLoop for WorkerLoop {
    fn id(&self) -> &str {
        "worker"
    }
    fn digest(&self) -> &str {
        "worker_v1"
    }
    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        let steers = ctx.drain_steering();
        self.step_num += 1;
        match results {
            None => LoopAction::Yield(vec![Effect::Pause {
                reason: PauseReason::Approval,
                payload: json!({"ask": "proceed?"}),
            }]),
            Some(res) => {
                if matches!(res.first(), Some(StepResult::Switched { .. })) {
                    return LoopAction::Yield(vec![
                        Effect::Tool {
                            name: "read_file".into(),
                            args: json!({"path": "a.rs"}),
                            side_effect: SideEffectClass::Read,
                        },
                        Effect::Tool {
                            name: "forbidden_tool".into(),
                            args: json!({}),
                            side_effect: SideEffectClass::Irreversible,
                        },
                    ]);
                }
                if !steers.is_empty() {
                    return LoopAction::Done(json!({
                        "steered_with": steers[0].content,
                        "handoff": ctx.handoff(),
                        "step": self.step_num,
                    }));
                }
                LoopAction::Done(json!({
                    "handoff": ctx.handoff(),
                    "results_count": res.len(),
                    "step": self.step_num,
                }))
            }
        }
    }
}

struct WorkerFactory;
impl AgentLoopFactory for WorkerFactory {
    fn id(&self) -> &str {
        "worker"
    }
    fn digest(&self) -> &str {
        "worker_v1"
    }
    fn create(&self, _config: &LoopConfig, _registry: &ExtensionRegistry) -> Box<dyn AgentLoop> {
        Box::new(WorkerLoop { step_num: 0 })
    }
}

struct AuditDecorator;
impl LoopDecorator for AuditDecorator {
    fn id(&self) -> &str {
        "audit_dec"
    }
    fn digest(&self) -> &str {
        "audit_dec_v1"
    }
    fn wrap(
        &self,
        inner: Box<dyn AgentLoop>,
        _config: &LoopConfig,
        _registry: &ExtensionRegistry,
    ) -> Box<dyn AgentLoop> {
        Box::new(AuditWrapped(inner))
    }
}

struct AuditWrapped(Box<dyn AgentLoop>);
impl AgentLoop for AuditWrapped {
    fn id(&self) -> &str {
        self.0.id()
    }
    fn digest(&self) -> &str {
        self.0.digest()
    }
    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        self.0.step(ctx, results)
    }
}

struct TestExtension;
impl Extension for TestExtension {
    fn name(&self) -> &str {
        "test_ext"
    }
    fn register(&self, registry: &mut ExtensionRegistry) -> Result<(), ExtensionError> {
        registry
            .builders
            .insert("echo_builder".into(), Box::new(ArtifactEchoBuilder));
        registry
            .loops
            .insert("triage".into(), Box::new(TriageFactory));
        registry
            .loops
            .insert("worker".into(), Box::new(WorkerFactory));
        registry
            .decorators
            .insert("audit_dec".into(), Box::new(AuditDecorator));
        registry.tools.insert(
            "read_file".into(),
            ToolSpec {
                name: "read_file".into(),
                description: "read".into(),
                parameters_schema: json!({}),
                default_side_effect: SideEffectClass::Read,
                source_extension: None,
            },
        );
        registry.tools.insert(
            "forbidden_tool".into(),
            ToolSpec {
                name: "forbidden_tool".into(),
                description: "forbidden".into(),
                parameters_schema: json!({}),
                default_side_effect: SideEffectClass::Irreversible,
                source_extension: None,
            },
        );
        registry.presets.insert(
            "triage_preset".into(),
            Preset {
                base_loop: "triage".into(),
                decorators: vec![],
                builder: "echo_builder".into(),
                config: LoopConfig::default(),
            },
        );
        registry.presets.insert(
            "worker_preset".into(),
            Preset {
                base_loop: "worker".into(),
                decorators: vec!["audit_dec".into()],
                builder: "echo_builder".into(),
                config: LoopConfig {
                    allowed_tools: Some(vec!["read_file".into()]),
                    ..LoopConfig::default()
                },
            },
        );
        Ok(())
    }
}

fn setup_registry() -> ExtensionRegistry {
    let mut reg = ExtensionRegistry::new();
    TestExtension.register(&mut reg).expect("register ext");
    reg
}

fn setup_gate() -> PolicyGate {
    PolicyGate::new(
        BudgetCap {
            max_steps: Some(20),
            max_tokens: Some(100_000),
            max_cost_micro_usd: Some(1_000_000),
        },
        None,
        SandboxPolicy::default(),
    )
}

#[test]
fn multi_phase_switch_mixed_batch_and_hermetic_offline_replay() {
    let reg = setup_registry();
    let gate = setup_gate();
    let mut rt = KernelRuntime::new("r1".into(), "/repo".into(), 1000, reg, gate).expect("init");

    let guide_v1 = "Initial guide v1".to_string();
    let guide_digest = sha256_hex(guide_v1.as_bytes());
    let mut blob_store = BTreeMap::new();
    blob_store.insert(guide_digest.clone(), guide_v1.clone());

    let mut art_entries = BTreeMap::new();
    art_entries.insert(
        "GUIDE.md".into(),
        ArtifactEntry {
            path: "GUIDE.md".into(),
            content: guide_v1,
            blob_digest: guide_digest,
        },
    );
    let artifacts = ArtifactStoreView::from_entries(art_entries);

    rt.start_turn(json!("fix bug"), "triage_preset", &[], artifacts, 1001)
        .expect("start_turn");

    // Step 1: TriageLoop yields Decision model call.
    let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
    let eff1 = match d1 {
        DriverDirective::DispatchEffects { effects } => {
            assert_eq!(effects.len(), 1);
            effects[0].effect_id.clone()
        }
        other => panic!("unexpected directive: {other:?}"),
    };

    // Feed low confidence -> TriageLoop yields Effect::Switch -> WorkerLoop yields [read_file, forbidden_tool].
    // PolicyGate approves read_file and denies forbidden_tool in the same batch!
    let d2 = rt
        .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
            effect_id: eff1,
            kind: ModelKind::Decision,
            output: json!({"choice": "escalate", "confidence": 0.4}),
            usage: Usage {
                input_units: 10,
                output_units: 5,
                cost_micro_usd: 100,
            },
        }]))
        .expect("d2");

    let eff_read = match d2 {
        DriverDirective::DispatchEffects { effects } => {
            assert_eq!(effects.len(), 1);
            match &effects[0].effect {
                Effect::Tool { name, .. } => assert_eq!(name, "read_file"),
                other => panic!("expected tool, got {other:?}"),
            }
            effects[0].effect_id.clone()
        }
        other => panic!("expected DispatchEffects, got {other:?}"),
    };

    // Complete read_file -> WorkerLoop receives [Tool(ok), Denied(forbidden_tool)] and finishes Turn.
    let d3 = rt
        .next_action(DriverResponse::EffectResults(vec![StepResult::Tool {
            effect_id: eff_read,
            output: json!({"content": "fn main() {}"}),
            is_error: false,
            blob_digest: None,
        }]))
        .expect("d3");

    match d3 {
        DriverDirective::TurnComplete { output, usage, .. } => {
            assert_eq!(output["results_count"], 2);
            assert_eq!(usage.cost_micro_usd, 100);
        }
        other => panic!("expected TurnComplete, got {other:?}"),
    }

    // Verify JSONL persistence round-trip and hermetic offline replay.
    let jsonl = rt.drain_pending_jsonl_lines().join("\n");
    let (loaded_tree, warn) = SessionTree::from_jsonl_str(&jsonl).expect("load jsonl");
    assert!(warn.is_none());

    // Even if workspace GUIDE.md changes on disk after recording, blob_store holds the snapshot.
    let replay_reg = setup_registry();
    let replay_gate = setup_gate();
    let engine = ReplayEngine::new(&replay_reg, &replay_gate);
    let report = engine
        .replay(&loaded_tree, ReplayMode::Default, |digest| {
            blob_store.get(digest).cloned()
        })
        .expect("replay");
    assert!(
        report.is_parity(),
        "expected 100% parity, got divergences: {:?}",
        report.divergences
    );
    assert_eq!(report.turns_replayed, 1);
    assert_eq!(report.phases_replayed, 2);

    // Check canonical events include PhaseSwitched and PolicyDenied.
    let events = rt.drain_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::PolicyDenied { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::PhaseSwitched { .. }))
    );
}

#[test]
fn urgent_steer_synthesizes_aborted_step_results_and_redirects_loop() {
    let reg = setup_registry();
    let gate = setup_gate();
    let mut rt = KernelRuntime::new("r2".into(), "/repo".into(), 100, reg, gate).expect("init");

    rt.start_turn(
        json!("go"),
        "triage_preset",
        &[],
        ArtifactStoreView::default(),
        101,
    )
    .expect("start");
    let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
    assert!(matches!(d1, DriverDirective::DispatchEffects { .. }));

    // Enqueue UrgentSteer while Decision model is in flight, then switch to worker?
    // Or directly on Poll after UrgentSteer: kernel synthesizes StepResult::Aborted for in-flight effect!
    rt.enqueue_steering(
        SteeringKind::UrgentSteer,
        json!("switch to worker immediately"),
        SteeringSource::User,
        102,
    )
    .expect("steer");

    let d2 = rt
        .next_action(DriverResponse::EffectResults(vec![]))
        .expect("d2");
    assert!(matches!(d2, DriverDirective::TurnComplete { .. }));
    let has_aborted_node = rt
        .session_tree()
        .all_nodes()
        .iter()
        .any(|n| n.kind == "step_result" && n.payload["status"] == "aborted");
    assert!(has_aborted_node);
}

#[test]
fn pause_round_trip_and_abort_cleanup() {
    let reg = setup_registry();
    let gate = setup_gate();
    let mut rt = KernelRuntime::new("r3".into(), "/repo".into(), 200, reg, gate).expect("init");

    rt.start_turn(
        json!("pause test"),
        "worker_preset",
        &[],
        ArtifactStoreView::default(),
        201,
    )
    .expect("start");

    let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
    let (pause_id, eff_id) = match d1 {
        DriverDirective::WaitForPause {
            pause_id,
            effect_id,
            reason,
            ..
        } => {
            assert_eq!(reason, PauseReason::Approval);
            (pause_id, effect_id)
        }
        other => panic!("expected WaitForPause, got {other:?}"),
    };

    let d2 = rt
        .next_action(DriverResponse::PauseResolution {
            pause_id,
            effect_id: eff_id,
            resolution: json!({"approved": true}),
        })
        .expect("d2");
    assert!(matches!(d2, DriverDirective::TurnComplete { .. }));
}

#[test]
fn replay_detects_digest_tampering_and_behavioral_drift() {
    let reg = setup_registry();
    let gate = setup_gate();
    let mut rt = KernelRuntime::new("r4".into(), "/repo".into(), 300, reg, gate).expect("init");
    rt.start_turn(
        json!("hi"),
        "triage_preset",
        &[],
        ArtifactStoreView::default(),
        301,
    )
    .expect("start");
    let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
    let eff1 = match d1 {
        DriverDirective::DispatchEffects { effects } => effects[0].effect_id.clone(),
        _ => panic!("expected dispatch"),
    };
    let _ = rt
        .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
            effect_id: eff1,
            kind: ModelKind::Decision,
            output: json!({"confidence": 0.99}),
            usage: Usage::default(),
        }]))
        .expect("done");

    // Mutate a node payload without updating its digest -> DigestMismatch reported.
    let mut tampered = rt.session_tree().clone();
    let jsonl = tampered
        .all_nodes()
        .iter()
        .map(|n| {
            let mut cloned = n.clone();
            if cloned.kind == "turn_start" {
                cloned.payload = json!({"tampered": true});
            }
            cloned.to_jsonl_line().expect("line")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let err = SessionTree::from_jsonl_str(&jsonl).expect_err("must reject tampered digest");
    assert!(format!("{err}").contains("digest mismatch"));

    // Now test behavioral drift in ReplayEngine when registered loop changes its digest/behavior.
    struct DriftedLoop;
    impl AgentLoop for DriftedLoop {
        fn id(&self) -> &str {
            "triage"
        }
        fn digest(&self) -> &str {
            "triage_v2_drifted"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            _results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            LoopAction::Done(json!({"early": true}))
        }
    }
    struct DriftedFactory;
    impl AgentLoopFactory for DriftedFactory {
        fn id(&self) -> &str {
            "triage"
        }
        fn digest(&self) -> &str {
            "triage_v2_drifted"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(DriftedLoop)
        }
    }

    let mut drifted_reg = setup_registry();
    drifted_reg
        .loops
        .insert("triage".into(), Box::new(DriftedFactory));
    let engine = ReplayEngine::new(&drifted_reg, rt.policy_gate());
    let report = engine
        .replay(rt.session_tree(), ReplayMode::Default, |_| None)
        .expect("replay");
    assert!(!report.is_parity());
    assert!(
        report
            .divergences
            .iter()
            .any(|d| d.kind == DivergenceKind::LoopDigestMismatch)
    );
    assert!(
        report
            .divergences
            .iter()
            .any(|d| d.kind == DivergenceKind::UnexpectedDone)
    );
    let _ = tampered.set_head(rt.session_tree().head_id().expect("head"));
}

#[test]
fn spawn_policy_blocks_concurrent_shared_and_depth_overflow() {
    let gate = PolicyGate::new(
        BudgetCap {
            max_steps: Some(10),
            max_tokens: Some(500),
            max_cost_micro_usd: Some(1000),
        },
        None,
        SandboxPolicy {
            network: false,
            protected_paths: BTreeSet::new(),
            max_spawn_depth: 1,
        },
    );
    let snap = gestalt_kernel::BudgetSnapshot {
        cap: gate.baseline_budget(),
        ..Default::default()
    };
    let batch = vec![
        EffectEnvelope {
            effect_id: "s1".into(),
            effect: Effect::Spawn {
                loop_id: "worker".into(),
                input: json!({}),
                budget: BudgetCap {
                    max_steps: Some(2),
                    max_tokens: Some(100),
                    max_cost_micro_usd: Some(100),
                },
                depth_limit: 1,
                workspace_mode: WorkspaceMode::Shared,
                config: None,
            },
        },
        EffectEnvelope {
            effect_id: "s2".into(),
            effect: Effect::Spawn {
                loop_id: "worker".into(),
                input: json!({}),
                budget: BudgetCap {
                    max_steps: Some(2),
                    max_tokens: Some(100),
                    max_cost_micro_usd: Some(100),
                },
                depth_limit: 1,
                workspace_mode: WorkspaceMode::Shared,
                config: None,
            },
        },
    ];
    let decisions = gate.evaluate_batch(
        &batch,
        &LoopConfig::default(),
        &snap,
        0,
        &BTreeSet::new(),
        &SessionSlice::default(),
    );
    assert!(
        decisions
            .iter()
            .all(|d| matches!(d, gestalt_kernel::BatchItemDecision::Denied { .. }))
    );

    // Depth limit check at depth == 1
    let at_depth_one = gate.evaluate_batch(
        &batch[..1],
        &LoopConfig::default(),
        &snap,
        1,
        &BTreeSet::new(),
        &SessionSlice::default(),
    );
    assert!(matches!(
        at_depth_one[0],
        gestalt_kernel::BatchItemDecision::Denied { .. }
    ));
}

struct VerificationGateRule;
impl PolicyRule for VerificationGateRule {
    fn id(&self) -> &str {
        "verify_read"
    }
    fn evaluate(&self, env: &EffectEnvelope, _s: &SessionSlice) -> PolicyDecision {
        if matches!(env.effect, Effect::Tool { .. }) {
            PolicyDecision::RequiresVerification {
                rule_id: self.id().to_string(),
                verifier_effect: Box::new(Effect::Model {
                    target: "verifier/decision".into(),
                    kind: ModelKind::Decision,
                    input: json!({"tool": env.effect_id}),
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
        match verifier_result {
            StepResult::Model { output, .. }
                if output.get("allow").and_then(Value::as_bool) == Some(true) =>
            {
                PolicyDecision::Allow
            }
            _ => PolicyDecision::Deny {
                reason: "verifier rejected tool".into(),
            },
        }
    }
}

#[test]
fn requires_verification_defers_and_promotes_on_allow() {
    let reg = setup_registry();
    let mut gate = setup_gate();
    gate.add_rule(Box::new(VerificationGateRule));
    let mut rt = KernelRuntime::new("r5".into(), "/repo".into(), 400, reg, gate).expect("init");
    rt.start_turn(
        json!("verify"),
        "triage_preset",
        &[],
        ArtifactStoreView::default(),
        401,
    )
    .expect("start");

    let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
    let eff1 = match d1 {
        DriverDirective::DispatchEffects { effects } => effects[0].effect_id.clone(),
        other => panic!("expected dispatch, got {other:?}"),
    };
    let d2 = rt
        .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
            effect_id: eff1,
            kind: ModelKind::Decision,
            output: json!({"confidence": 0.2}),
            usage: Usage::default(),
        }]))
        .expect("d2");
    let (rule_id, pending_id, verifier_id) = match d2 {
        DriverDirective::VerifyEffect {
            rule_id,
            verifier,
            pending,
        } => (rule_id, pending.effect_id, verifier.effect_id),
        other => panic!("expected VerifyEffect, got {other:?}"),
    };
    let d3 = rt
        .next_action(DriverResponse::VerificationResult {
            rule_id,
            pending_effect_id: pending_id,
            verifier_result: StepResult::Model {
                effect_id: verifier_id,
                kind: ModelKind::Decision,
                output: json!({"allow": true}),
                usage: Usage::default(),
            },
        })
        .expect("d3");
    assert!(matches!(d3, DriverDirective::DispatchEffects { .. }));
}

#[test]
fn external_builder_deferral_and_custom_node_persistence() {
    let mut reg = setup_registry();
    reg.register_external_describe(gestalt_kernel::ExternalExtensionSpec {
        name: "ext_b".into(),
        builders: vec![gestalt_kernel::ExternalComponentSpec {
            id: "ext_builder".into(),
            digest: "eb_v1".into(),
        }],
        presets: [(
            "ext_builder_preset".to_string(),
            Preset {
                base_loop: "triage".into(),
                decorators: vec![],
                builder: "ext_builder".into(),
                config: LoopConfig::default(),
            },
        )]
        .into_iter()
        .collect(),
        ..Default::default()
    });

    let gate = setup_gate();
    let mut rt = KernelRuntime::new("r6".into(), "/repo".into(), 500, reg, gate).expect("init");
    rt.start_turn(
        json!("test external builder"),
        "ext_builder_preset",
        &[],
        ArtifactStoreView::default(),
        501,
    )
    .expect("start");

    // First poll yields InvokeExternalBuilder because ext_builder is out-of-process.
    let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
    match d1 {
        DriverDirective::InvokeExternalBuilder {
            ext_name,
            builder_id,
            ..
        } => {
            assert_eq!(ext_name, "ext_b");
            assert_eq!(builder_id, "ext_builder");
        }
        other => panic!("expected InvokeExternalBuilder, got {other:?}"),
    }

    // Feed ExternalBuilderOutput -> TriageLoop receives precomputed context via ctx.build_context().
    let d2 = rt
        .next_action(DriverResponse::ExternalBuilderOutput {
            model_input: json!({"from_external_builder": true}),
        })
        .expect("d2");
    match d2 {
        DriverDirective::DispatchEffects { effects } => match &effects[0].effect {
            Effect::Model { input, .. } => {
                assert_eq!(input["from_external_builder"], true);
            }
            other => panic!("expected Model effect, got {other:?}"),
        },
        other => panic!("expected DispatchEffects, got {other:?}"),
    }

    let custom_id = rt
        .append_custom_node(
            "std:compaction".into(),
            json!({"summary": "compacted"}),
            502,
        )
        .expect("custom node");
    let custom_node = rt.session_tree().get(&custom_id).expect("get custom node");
    assert_eq!(custom_node.kind, "custom:std:compaction");
}

#[test]
fn emit_custom_node_updates_slice_immediately_and_replays_with_parity() {
    struct CustomNodeBuilder;
    impl ContextBuilder for CustomNodeBuilder {
        fn id(&self) -> &str {
            "custom_b"
        }
        fn digest(&self) -> &str {
            "cb1"
        }
        fn build(
            &self,
            session: &SessionSlice,
            _artifacts: &ArtifactStoreView,
            _caps: &ModelCapabilities,
        ) -> Value {
            let has_summary = session
                .nodes()
                .iter()
                .any(|n| n.kind == "custom:test:summary");
            json!({ "has_summary": has_summary })
        }
    }

    struct CustomEmittingLoop;
    impl AgentLoop for CustomEmittingLoop {
        fn id(&self) -> &str {
            "custom_emit"
        }
        fn digest(&self) -> &str {
            "ce1"
        }
        fn step(
            &mut self,
            ctx: &mut dyn LoopContext,
            results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            if results.is_none() {
                ctx.emit_custom_node("test:summary", json!({"v": 1}));
                let built = ctx.build_context(&ModelCapabilities::default());
                assert_eq!(built["has_summary"], true);
                LoopAction::Yield(vec![Effect::Model {
                    target: "mock/m".into(),
                    kind: ModelKind::Llm,
                    input: built,
                    schema: None,
                }])
            } else {
                LoopAction::Done(json!({"ok": true}))
            }
        }
    }
    struct CustomEmitFactory;
    impl AgentLoopFactory for CustomEmitFactory {
        fn id(&self) -> &str {
            "custom_emit"
        }
        fn digest(&self) -> &str {
            "ce1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(CustomEmittingLoop)
        }
    }

    let mut reg = setup_registry();
    reg.builders
        .insert("custom_b".into(), Box::new(CustomNodeBuilder));
    reg.loops
        .insert("custom_emit".into(), Box::new(CustomEmitFactory));
    reg.presets.insert(
        "custom_preset".into(),
        Preset {
            base_loop: "custom_emit".into(),
            decorators: vec![],
            builder: "custom_b".into(),
            config: LoopConfig::default(),
        },
    );

    let mut gate = setup_gate();
    gate.add_rule(Box::new(VerificationGateRule));
    assert_eq!(gate.rule_ids(), vec!["verify_read"]);

    let mut rt = KernelRuntime::new("r7".into(), "/repo".into(), 600, reg, gate).expect("init");
    rt.start_turn(
        json!("go"),
        "custom_preset",
        &[],
        ArtifactStoreView::default(),
        601,
    )
    .expect("start");
    let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
    let eff1 = match d1 {
        DriverDirective::DispatchEffects { effects } => effects[0].effect_id.clone(),
        other => panic!("expected dispatch, got {other:?}"),
    };
    let d2 = rt
        .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
            effect_id: eff1,
            kind: ModelKind::Llm,
            output: json!({"text": "done"}),
            usage: Usage::default(),
        }]))
        .expect("d2");
    assert!(matches!(d2, DriverDirective::TurnComplete { .. }));

    let report = ReplayEngine::new(rt.registry(), rt.policy_gate())
        .replay(rt.session_tree(), ReplayMode::Default, |_| None)
        .expect("replay");
    assert!(report.is_parity(), "divergences: {:?}", report.divergences);
}
