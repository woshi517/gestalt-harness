#![forbid(unsafe_code)]
//! Deterministic offline replay engine and node-level divergence reporter.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::effect::{BudgetSnapshot, LoopConfig};
use crate::error::KernelError;
use crate::extension::{
    AgentLoop, ArtifactEntry, ArtifactStoreView, ContextBuilder, ExtensionRegistry, LoopAction,
    LoopContext, ModelCapabilities,
};
use crate::policy::{BatchItemDecision, PolicyGate};
use crate::sha256::compute_node_digest;
use crate::steering::{SteeringInbox, SteeringSignal};
use crate::tree::{NodePayload, SESSION_SCHEMA_VERSION, SessionSlice, SessionTree};

/// Replay verification mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplayMode {
    /// Validate SHA-256 chain, `PolicyGate` invariants, and re-execute all
    /// registered in-process Rust loops/builders with 0 network calls while
    /// verifying recorded `state_token`/`effect` transitions for external workers.
    #[default]
    Default,
    /// Also re-execute out-of-process extensions inside a network-disabled
    /// sandbox (requires `gestalt-runtime` IPC driver).
    ReexecuteExternal,
}

/// Classification of a detected replay divergence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DivergenceKind {
    /// Node SHA-256 digest does not match `(parent_id, kind, canonical_json(payload))`.
    DigestMismatch,
    /// Node `parent_id` references a missing node.
    ParentLinkBroken,
    /// Unsupported `session_open.schema_version`.
    SchemaVersionUnsupported,
    /// Registered `AgentLoop` digest differs from recorded `phase_open.loop_digest`.
    LoopDigestMismatch,
    /// Registered `ContextBuilder` digest differs from recorded `phase_open.builder_digest`.
    BuilderDigestMismatch,
    /// Re-executed loop yielded a different number of effects or `Done` vs `Yield`.
    EffectSequenceMismatch,
    /// Re-executed loop yielded an `Effect` with a different operation or payload.
    EffectPayloadMismatch,
    /// `PolicyGate` evaluation no longer matches recorded `Denied` vs dispatched outcome.
    PolicyDecisionMismatch,
    /// Re-executed loop finished (`Done`) when recorded trace yielded another `Effect`.
    UnexpectedDone,
    /// Recorded `Effect` has no matching recorded `StepResult`.
    MissingRecordedStepResult,
}

/// Structured divergence record pinpointing the exact node and mismatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayDivergence {
    /// Node ID in `SessionTree` where divergence occurred.
    pub node_id: String,
    /// Turn ID, if inside a Turn.
    pub turn_id: Option<String>,
    /// Phase ID, if inside a Phase.
    pub phase_id: Option<String>,
    /// Divergence classification.
    pub kind: DivergenceKind,
    /// Expected value from the recorded trace.
    pub expected: Value,
    /// Actual value produced during replay.
    pub actual: Value,
    /// Human-readable diagnostic message.
    pub message: String,
}

/// Summary report produced by [`ReplayEngine::replay`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ReplayReport {
    /// Total nodes verified for SHA-256 integrity.
    pub total_nodes_verified: usize,
    /// Total Turns replayed.
    pub turns_replayed: usize,
    /// Total Phases replayed.
    pub phases_replayed: usize,
    /// Total `Effect`s verified for sequence and payload parity.
    pub effects_verified: usize,
    /// Any divergences detected during replay (empty means 100% parity).
    pub divergences: Vec<ReplayDivergence>,
}

impl ReplayReport {
    /// Return `true` if replay achieved 100% parity with zero divergences.
    #[must_use]
    pub fn is_parity(&self) -> bool {
        self.divergences.is_empty()
    }
}

struct ReplayLoopContext<'a> {
    run_id: &'a str,
    turn_id: &'a str,
    phase_id: &'a str,
    handoff: &'a Value,
    config: &'a LoopConfig,
    session: SessionSlice,
    artifacts: &'a ArtifactStoreView,
    budget: &'a BudgetSnapshot,
    builder: Option<&'a dyn ContextBuilder>,
    inbox: &'a mut SteeringInbox,
    now_ms: u64,
    id_seq: u64,
    pending_custom_nodes: Vec<(String, Value)>,
}

impl LoopContext for ReplayLoopContext<'_> {
    fn run_id(&self) -> &str {
        self.run_id
    }
    fn turn_id(&self) -> &str {
        self.turn_id
    }
    fn phase_id(&self) -> &str {
        self.phase_id
    }
    fn handoff(&self) -> &Value {
        self.handoff
    }
    fn config(&self) -> &LoopConfig {
        self.config
    }
    fn session(&self) -> &SessionSlice {
        &self.session
    }
    fn artifacts(&self) -> &ArtifactStoreView {
        self.artifacts
    }
    fn budget_remaining(&self) -> &BudgetSnapshot {
        self.budget
    }
    fn build_context(&self, caps: &ModelCapabilities) -> Value {
        match self.builder {
            Some(b) => b.build(&self.session, self.artifacts, caps),
            None => Value::Null,
        }
    }
    fn drain_steering(&mut self) -> Vec<SteeringSignal> {
        self.inbox.drain_steering()
    }
    fn now_ms(&self) -> u64 {
        self.now_ms
    }
    fn next_id(&mut self) -> String {
        self.id_seq += 1;
        format!("replay_{}_{}", self.run_id, self.id_seq)
    }
    fn emit_custom_node(&mut self, namespace: &str, data: Value) {
        self.session.push_custom_preview(
            self.turn_id,
            self.phase_id,
            self.now_ms,
            self.pending_custom_nodes.len(),
            namespace,
            data.clone(),
        );
        self.pending_custom_nodes
            .push((namespace.to_string(), data));
    }
}

/// Deterministic offline replay engine validating cryptographic integrity,
/// `PolicyGate` invariants, and exact `Effect` parity with 0 network calls.
pub struct ReplayEngine<'a> {
    registry: &'a ExtensionRegistry,
    policy_gate: &'a PolicyGate,
}

impl<'a> ReplayEngine<'a> {
    /// Construct a new [`ReplayEngine`] bound to `registry` and `policy_gate`.
    #[must_use]
    pub fn new(registry: &'a ExtensionRegistry, policy_gate: &'a PolicyGate) -> Self {
        Self {
            registry,
            policy_gate,
        }
    }

    /// Replay a [`SessionTree`] deterministically offline.
    ///
    /// `blob_resolver` maps a SHA-256 `blob_digest` from
    /// `turn_start.artifact_snapshots` to the historical UTF-8 artifact content
    /// (typically read from `.gestalt/blobs/<sha256>`).
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] only on structural decode failures.
    #[allow(clippy::too_many_lines)]
    pub fn replay<F>(
        &self,
        tree: &SessionTree,
        mode: ReplayMode,
        blob_resolver: F,
    ) -> Result<ReplayReport, KernelError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut report = ReplayReport::default();
        let nodes = tree.all_nodes();

        // 1. Verify SHA-256 hash chain and schema version across all nodes.
        let mut seen_ids = std::collections::BTreeSet::new();
        for node in nodes {
            if let Some(ref pid) = node.parent_id
                && !seen_ids.contains(pid)
            {
                report.divergences.push(ReplayDivergence {
                    node_id: node.id.clone(),
                    turn_id: node.turn_id.clone(),
                    phase_id: node.phase_id.clone(),
                    kind: DivergenceKind::ParentLinkBroken,
                    expected: Value::String(pid.clone()),
                    actual: Value::Null,
                    message: format!("missing parent_id '{pid}'"),
                });
            }
            if node.kind == "session_open"
                && node.payload.get("schema_version").and_then(Value::as_u64)
                    != Some(u64::from(SESSION_SCHEMA_VERSION))
            {
                report.divergences.push(ReplayDivergence {
                    node_id: node.id.clone(),
                    turn_id: node.turn_id.clone(),
                    phase_id: node.phase_id.clone(),
                    kind: DivergenceKind::SchemaVersionUnsupported,
                    expected: Value::from(SESSION_SCHEMA_VERSION),
                    actual: node
                        .payload
                        .get("schema_version")
                        .cloned()
                        .unwrap_or(Value::Null),
                    message: "unsupported session_open schema_version".to_string(),
                });
            }
            let expected_digest =
                compute_node_digest(node.parent_id.as_deref(), &node.kind, &node.payload);
            if node.digest != expected_digest {
                report.divergences.push(ReplayDivergence {
                    node_id: node.id.clone(),
                    turn_id: node.turn_id.clone(),
                    phase_id: node.phase_id.clone(),
                    kind: DivergenceKind::DigestMismatch,
                    expected: Value::String(expected_digest),
                    actual: Value::String(node.digest.clone()),
                    message: "node SHA-256 digest mismatch".to_string(),
                });
            }
            seen_ids.insert(node.id.clone());
            report.total_nodes_verified += 1;
        }

        // 2. Replay active Turn/Phase sequences against recorded StepResults.
        let mut current_artifacts = ArtifactStoreView::default();
        let mut current_loop: Option<Box<dyn AgentLoop>> = None;
        let mut current_builder_id = String::new();
        let mut current_config = LoopConfig::default();
        let mut current_handoff = Value::Null;
        let mut inbox = SteeringInbox::new();
        let mut budget = BudgetSnapshot {
            cap: self.policy_gate.baseline_budget(),
            ..BudgetSnapshot::default()
        };
        let mut idx = 0;

        while idx < nodes.len() {
            let node = &nodes[idx];
            let decoded = node.decode_payload()?;

            match decoded {
                NodePayload::TurnStart {
                    artifact_snapshots, ..
                } => {
                    report.turns_replayed += 1;
                    let mut entries = BTreeMap::new();
                    for (path, digest) in artifact_snapshots {
                        if let Some(content) = blob_resolver(&digest) {
                            entries.insert(
                                path.clone(),
                                ArtifactEntry {
                                    path,
                                    content,
                                    blob_digest: digest,
                                },
                            );
                        }
                    }
                    current_artifacts = ArtifactStoreView::from_entries(entries);
                }
                NodePayload::Steering {
                    id,
                    kind,
                    content,
                    source,
                } => {
                    inbox.enqueue(SteeringSignal {
                        id,
                        kind,
                        content,
                        source,
                        timestamp_ms: node.timestamp_ms,
                    });
                }
                NodePayload::PhaseOpen {
                    preset_id,
                    base_loop_id,
                    loop_digest,
                    builder_id,
                    builder_digest,
                    config,
                    ..
                } => {
                    report.phases_replayed += 1;
                    current_config = config.clone();
                    current_builder_id = builder_id.clone();

                    if let Some(b) = self.registry.builders.get(&builder_id)
                        && b.digest() != builder_digest
                    {
                        report.divergences.push(ReplayDivergence {
                            node_id: node.id.clone(),
                            turn_id: node.turn_id.clone(),
                            phase_id: node.phase_id.clone(),
                            kind: DivergenceKind::BuilderDigestMismatch,
                            expected: Value::String(builder_digest),
                            actual: Value::String(b.digest().to_string()),
                            message: format!("builder '{builder_id}' digest mismatch"),
                        });
                    }

                    let target = preset_id.as_deref().unwrap_or(&base_loop_id);
                    if let Ok(stack) = self
                        .registry
                        .resolve_phase_stack(target, &[], Some(&config))
                    {
                        if stack.loop_digest != loop_digest {
                            report.divergences.push(ReplayDivergence {
                                node_id: node.id.clone(),
                                turn_id: node.turn_id.clone(),
                                phase_id: node.phase_id.clone(),
                                kind: DivergenceKind::LoopDigestMismatch,
                                expected: Value::String(loop_digest),
                                actual: Value::String(stack.loop_digest),
                                message: format!("loop '{base_loop_id}' digest mismatch"),
                            });
                        }
                        current_loop = stack.in_process_loop;
                    } else if mode == ReplayMode::ReexecuteExternal {
                        report.divergences.push(ReplayDivergence {
                            node_id: node.id.clone(),
                            turn_id: node.turn_id.clone(),
                            phase_id: node.phase_id.clone(),
                            kind: DivergenceKind::LoopDigestMismatch,
                            expected: Value::String(base_loop_id),
                            actual: Value::Null,
                            message: "external loop missing for --reexecute-ext".to_string(),
                        });
                    } else {
                        current_loop = None;
                    }

                    // Initial step for a Turn-opening phase (`None`), unless this Phase was
                    // opened by `Effect::Switch` (in which case the next node is
                    // `StepResult::Switched` and drives the first step of the new Phase).
                    let is_switch_target = nodes.get(idx + 1).is_some_and(|next| {
                        next.kind == "step_result"
                            && next.payload.get("status").and_then(Value::as_str)
                                == Some("switched")
                    });
                    if !is_switch_target && let Some(ref mut lp) = current_loop {
                        while idx + 1 < nodes.len() && nodes[idx + 1].kind == "steering" {
                            if let NodePayload::Steering {
                                id,
                                kind,
                                content,
                                source,
                            } = nodes[idx + 1].decode_payload()?
                            {
                                let ts = nodes[idx + 1].timestamp_ms;
                                idx += 1;
                                inbox.enqueue(SteeringSignal {
                                    id,
                                    kind,
                                    content,
                                    source,
                                    timestamp_ms: ts,
                                });
                            } else {
                                break;
                            }
                        }
                        let slice = tree.slice_to(Some(&nodes[idx].id))?;
                        let builder = self
                            .registry
                            .builders
                            .get(&current_builder_id)
                            .map(AsRef::as_ref);
                        let mut ctx = ReplayLoopContext {
                            run_id: "replay",
                            turn_id: nodes[idx].turn_id.as_deref().unwrap_or(""),
                            phase_id: nodes[idx].phase_id.as_deref().unwrap_or(""),
                            handoff: &current_handoff,
                            config: &current_config,
                            session: slice.clone(),
                            artifacts: &current_artifacts,
                            budget: &budget,
                            builder,
                            inbox: &mut inbox,
                            now_ms: nodes[idx].timestamp_ms,
                            id_seq: idx as u64,
                            pending_custom_nodes: Vec::new(),
                        };
                        let action = lp.step(&mut ctx, None);
                        self.verify_action_against_trace(
                            nodes,
                            idx + 1,
                            &action,
                            &ctx.pending_custom_nodes,
                            &current_config,
                            &budget,
                            &slice,
                            &mut report,
                        );
                    }
                }
                NodePayload::PhaseSeal { handoff, .. } => {
                    current_handoff = handoff;
                    current_loop = None;
                }
                NodePayload::StepResult(res) => {
                    if let Some(u) = res.usage() {
                        budget.record_usage(u);
                    }
                    budget.record_step(None);
                    // Feed completed step results back into active in-process loop.
                    if let Some(ref mut lp) = current_loop {
                        // Collect contiguous StepResult nodes for this batch (including any
                        // intra-batch `steering` nodes enqueued while effects were in flight).
                        let mut batch_results = vec![res];
                        while idx + 1 < nodes.len() {
                            match nodes[idx + 1].kind.as_str() {
                                "step_result" => {
                                    if let NodePayload::StepResult(next_res) =
                                        nodes[idx + 1].decode_payload()?
                                    {
                                        idx += 1;
                                        if let Some(u) = next_res.usage() {
                                            budget.record_usage(u);
                                        }
                                        batch_results.push(next_res);
                                    } else {
                                        break;
                                    }
                                }
                                "steering" => {
                                    if let NodePayload::Steering {
                                        id,
                                        kind,
                                        content,
                                        source,
                                    } = nodes[idx + 1].decode_payload()?
                                    {
                                        let ts = nodes[idx + 1].timestamp_ms;
                                        idx += 1;
                                        inbox.enqueue(SteeringSignal {
                                            id,
                                            kind,
                                            content,
                                            source,
                                            timestamp_ms: ts,
                                        });
                                    } else {
                                        break;
                                    }
                                }
                                _ => break,
                            }
                        }
                        let slice = tree.slice_to(Some(&nodes[idx].id))?;
                        let builder = self
                            .registry
                            .builders
                            .get(&current_builder_id)
                            .map(AsRef::as_ref);
                        let mut ctx = ReplayLoopContext {
                            run_id: "replay",
                            turn_id: nodes[idx].turn_id.as_deref().unwrap_or(""),
                            phase_id: nodes[idx].phase_id.as_deref().unwrap_or(""),
                            handoff: &current_handoff,
                            config: &current_config,
                            session: slice.clone(),
                            artifacts: &current_artifacts,
                            budget: &budget,
                            builder,
                            inbox: &mut inbox,
                            now_ms: nodes[idx].timestamp_ms,
                            id_seq: idx as u64,
                            pending_custom_nodes: Vec::new(),
                        };
                        let action = lp.step(&mut ctx, Some(batch_results));
                        self.verify_action_against_trace(
                            nodes,
                            idx + 1,
                            &action,
                            &ctx.pending_custom_nodes,
                            &current_config,
                            &budget,
                            &slice,
                            &mut report,
                        );
                    }
                }
                NodePayload::TurnEnd { .. } => {
                    let _ = inbox.promote_undrained_to_follow_up();
                }
                _ => {}
            }

            idx += 1;
        }

        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_action_against_trace(
        &self,
        nodes: &[crate::tree::SessionNode],
        start_idx: usize,
        action: &LoopAction,
        emitted_custom: &[(String, Value)],
        phase_config: &LoopConfig,
        budget: &BudgetSnapshot,
        slice: &SessionSlice,
        report: &mut ReplayReport,
    ) {
        let mut recorded_custom = Vec::new();
        let mut recorded_effects = Vec::new();
        let mut cursor = start_idx;
        while cursor < nodes.len() {
            let n = &nodes[cursor];
            if n.kind.starts_with("custom:") && recorded_effects.is_empty() {
                recorded_custom.push(n);
                cursor += 1;
            } else if n.kind == "effect" {
                if let Ok(NodePayload::Effect { envelope, .. }) = n.decode_payload() {
                    recorded_effects.push((
                        n.id.clone(),
                        n.turn_id.clone(),
                        n.phase_id.clone(),
                        envelope,
                    ));
                }
                cursor += 1;
            } else if n.kind == "steering" || n.kind == "pause" || n.kind == "pause_resolved" {
                cursor += 1;
            } else {
                break;
            }
        }

        if emitted_custom.len() != recorded_custom.len() {
            let node_id = recorded_custom
                .first()
                .map_or_else(|| "eof".to_string(), |n| n.id.clone());
            report.divergences.push(ReplayDivergence {
                node_id,
                turn_id: slice.head().and_then(|h| h.turn_id.clone()),
                phase_id: slice.head().and_then(|h| h.phase_id.clone()),
                kind: DivergenceKind::EffectSequenceMismatch,
                expected: Value::from(recorded_custom.len()),
                actual: Value::from(emitted_custom.len()),
                message: format!(
                    "custom node count mismatch: recorded {}, replayed {}",
                    recorded_custom.len(),
                    emitted_custom.len()
                ),
            });
        } else {
            for ((ns, data), rec_node) in emitted_custom.iter().zip(recorded_custom.iter()) {
                let expected_kind = if ns.starts_with("custom:") {
                    ns.clone()
                } else {
                    format!("custom:{ns}")
                };
                if rec_node.kind != expected_kind || &rec_node.payload != data {
                    report.divergences.push(ReplayDivergence {
                        node_id: rec_node.id.clone(),
                        turn_id: rec_node.turn_id.clone(),
                        phase_id: rec_node.phase_id.clone(),
                        kind: DivergenceKind::EffectPayloadMismatch,
                        expected: rec_node.payload.clone(),
                        actual: data.clone(),
                        message: format!(
                            "replayed custom node '{expected_kind}' diverged from '{}'",
                            rec_node.kind
                        ),
                    });
                }
            }
        }

        match action {
            LoopAction::Yield(replayed_effects) => {
                if replayed_effects.len() != recorded_effects.len() {
                    let node_id = recorded_effects
                        .first()
                        .map_or_else(|| "eof".to_string(), |(id, _, _, _)| id.clone());
                    report.divergences.push(ReplayDivergence {
                        node_id,
                        turn_id: slice.head().and_then(|h| h.turn_id.clone()),
                        phase_id: slice.head().and_then(|h| h.phase_id.clone()),
                        kind: DivergenceKind::EffectSequenceMismatch,
                        expected: Value::from(recorded_effects.len()),
                        actual: Value::from(replayed_effects.len()),
                        message: format!(
                            "effect batch length mismatch: recorded {}, replayed {}",
                            recorded_effects.len(),
                            replayed_effects.len()
                        ),
                    });
                    return;
                }

                let registered_tools = self.registry.registered_tool_names();
                let env_slice: Vec<_> = recorded_effects
                    .iter()
                    .map(|(_, _, _, e)| e.clone())
                    .collect();
                let decisions: Vec<BatchItemDecision> = self.policy_gate.evaluate_batch(
                    &env_slice,
                    phase_config,
                    budget,
                    0,
                    &registered_tools,
                    slice,
                );

                // Map recorded StepResults by effect_id to verify policy decision parity.
                let mut recorded_denied = std::collections::BTreeMap::new();
                let mut res_cursor = cursor;
                while res_cursor < nodes.len() && nodes[res_cursor].kind == "step_result" {
                    if let Ok(NodePayload::StepResult(ref sr)) = nodes[res_cursor].decode_payload()
                    {
                        recorded_denied.insert(
                            sr.effect_id().to_string(),
                            matches!(sr, crate::effect::StepResult::Denied { .. }),
                        );
                    }
                    res_cursor += 1;
                }

                for ((replayed, (node_id, turn_id, phase_id, recorded_env)), decision) in
                    replayed_effects
                        .iter()
                        .zip(recorded_effects.iter())
                        .zip(decisions.iter())
                {
                    report.effects_verified += 1;
                    let replay_is_denied = matches!(decision, BatchItemDecision::Denied { .. });
                    if let Some(&was_denied) = recorded_denied.get(&recorded_env.effect_id)
                        && replay_is_denied != was_denied
                    {
                        report.divergences.push(ReplayDivergence {
                            node_id: node_id.clone(),
                            turn_id: turn_id.clone(),
                            phase_id: phase_id.clone(),
                            kind: DivergenceKind::PolicyDecisionMismatch,
                            expected: Value::Bool(!was_denied),
                            actual: Value::Bool(!replay_is_denied),
                            message: format!(
                                "policy decision mismatch for effect '{}': recorded denied={was_denied}, replayed denied={replay_is_denied}",
                                recorded_env.effect_id
                            ),
                        });
                    }
                    if replayed != &recorded_env.effect {
                        report.divergences.push(ReplayDivergence {
                            node_id: node_id.clone(),
                            turn_id: turn_id.clone(),
                            phase_id: phase_id.clone(),
                            kind: DivergenceKind::EffectPayloadMismatch,
                            expected: serde_json::to_value(&recorded_env.effect)
                                .unwrap_or(Value::Null),
                            actual: serde_json::to_value(replayed).unwrap_or(Value::Null),
                            message: format!(
                                "replayed effect '{}' payload diverged from recorded node",
                                replayed.op_name()
                            ),
                        });
                    }
                }
            }
            LoopAction::Done(_) => {
                if !recorded_effects.is_empty() {
                    let (node_id, turn_id, phase_id, recorded_env) = &recorded_effects[0];
                    report.divergences.push(ReplayDivergence {
                        node_id: node_id.clone(),
                        turn_id: turn_id.clone(),
                        phase_id: phase_id.clone(),
                        kind: DivergenceKind::UnexpectedDone,
                        expected: serde_json::to_value(&recorded_env.effect).unwrap_or(Value::Null),
                        actual: Value::String("done".to_string()),
                        message: "replayed loop returned Done where trace recorded an Effect"
                            .to_string(),
                    });
                }
            }
        }
    }
}
