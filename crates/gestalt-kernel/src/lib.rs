#![forbid(unsafe_code)]
#![deny(missing_docs)]
//! Invariant sans-I/O microkernel for the Gestalt agent harness.
//!
//! `gestalt-kernel` owns only bare-bones mechanism, scheduling, and trust:
//! [`SessionTree`] + [`SteeringInbox`], typed [`Effect`] bus, fail-closed
//! [`PolicyGate`], [`SandboxPolicy`] / `Save` boundary, [`EventStream`] +
//! [`ReplayEngine`], and unified [`Extension`] contracts.

pub mod effect;
pub mod error;
pub mod event;
pub mod extension;
pub mod policy;
pub mod replay;
pub mod sha256;
pub mod steering;
pub mod tree;

use std::collections::{BTreeMap, VecDeque};

use serde_json::Value;

pub use effect::{
    ArtifactPatch, BudgetCap, BudgetSnapshot, DiffHunk, Effect, EffectEnvelope, LoopConfig,
    ModelKind, PauseReason, SaveMode, SideEffectClass, StepResult, Usage, WorkspaceMode,
};
pub use error::{ExtensionError, KernelError};
pub use event::{DriverDirective, DriverResponse, EventStream, RunEvent};
pub use extension::{
    AgentLoop, AgentLoopFactory, ArtifactEntry, ArtifactStoreView, CommandSpec, ContextBuilder,
    Extension, ExtensionRegistry, ExternalBinding, ExternalComponentSpec, ExternalExtensionSpec,
    LoopAction, LoopContext, LoopDecorator, ModelCapabilities, Preset, ResolvedPhaseStack,
    ToolSpec, TriggerSpec, UIActionSpec, UIDialogSpec, UIPanelSpec, UIRendererSpec, UISurface,
};
pub use policy::{
    BatchItemDecision, PolicyDecision, PolicyGate, PolicyOverride, PolicyRule, SandboxPolicy,
};
pub use replay::{DivergenceKind, ReplayDivergence, ReplayEngine, ReplayMode, ReplayReport};
pub use sha256::{Sha256, canonical_json, compute_node_digest, sha256_hex};
pub use steering::{
    SteeringInbox, SteeringKind, SteeringSignal, SteeringSource, synthesize_aborted_results,
};
pub use tree::{
    BranchAction, NodePayload, SESSION_SCHEMA_VERSION, SessionNode, SessionSlice, SessionTree,
    TurnStatus,
};

struct ActivePhase {
    phase_id: String,
    handoff: Value,
    config: LoopConfig,
    builder_id: String,
    external_builder: Option<ExternalBinding>,
    cached_external_context: Option<Value>,
    in_process_loop: Option<Box<dyn AgentLoop>>,
    external_loop: Option<ExternalBinding>,
    external_state_token: Option<Value>,
}

struct PendingBatch {
    /// Total effects yielded in this step, preserving exact 1:1 batch order.
    total_len: usize,
    /// Pre-denied results keyed by original batch index.
    pre_denied: BTreeMap<usize, StepResult>,
    /// Effects dispatched or awaiting verification, mapped to original batch index.
    dispatched_indices: BTreeMap<String, usize>,
    /// Effects currently in flight in `gestalt-runtime`.
    in_flight: Vec<EffectEnvelope>,
    /// Active verification hold `(rule_id, pending_effect, original_index)`.
    pending_verification: Option<(String, EffectEnvelope, usize)>,
    /// Remaining queued verifications `(rule_id, verifier_env, pending_env, original_index)`.
    verification_queue: VecDeque<(String, EffectEnvelope, EffectEnvelope, usize)>,
}

struct ActiveTurn {
    turn_id: String,
    usage: Usage,
    phase: ActivePhase,
    pending_batch: Option<PendingBatch>,
    awaiting_pause: Option<(String, String)>,
}

/// Read/write view implementing [`LoopContext`] for an active Phase without
/// exposing `KernelRuntime` internals to user-space loops.
struct PhaseLoopContext<'a> {
    run_id: &'a str,
    turn_id: &'a str,
    phase_id: &'a str,
    handoff: &'a Value,
    config: &'a LoopConfig,
    session: SessionSlice,
    artifacts: &'a ArtifactStoreView,
    budget: &'a BudgetSnapshot,
    builder: Option<&'a dyn ContextBuilder>,
    precomputed_context: Option<&'a Value>,
    inbox: &'a mut SteeringInbox,
    now_ms: u64,
    id_counter: &'a mut u64,
    pending_custom_nodes: Vec<(String, Value)>,
}

impl LoopContext for PhaseLoopContext<'_> {
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
        if let Some(pre) = self.precomputed_context {
            return pre.clone();
        }
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
        *self.id_counter += 1;
        format!("id_{}_{}", self.run_id, *self.id_counter)
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

/// Sans-I/O microkernel state machine driving Turns, Steps, Phases,
/// `PolicyGate` evaluations, steering, and `SessionTree` persistence.
pub struct KernelRuntime {
    run_id: String,
    tree: SessionTree,
    registry: ExtensionRegistry,
    policy_gate: PolicyGate,
    inbox: SteeringInbox,
    events: EventStream,
    artifacts: ArtifactStoreView,
    budget: BudgetSnapshot,
    spawn_depth: u8,
    now_ms: u64,
    id_seq: u64,
    pending_jsonl: Vec<String>,
    active_turn: Option<ActiveTurn>,
}

impl KernelRuntime {
    /// Initialize a fresh [`KernelRuntime`] and append the root `session_open` node.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] if `session_open` serialization fails.
    pub fn new(
        run_id: String,
        workspace_root: String,
        created_at_ms: u64,
        registry: ExtensionRegistry,
        policy_gate: PolicyGate,
    ) -> Result<Self, KernelError> {
        let budget = BudgetSnapshot {
            cap: policy_gate.baseline_budget(),
            ..BudgetSnapshot::default()
        };
        let mut rt = Self {
            run_id,
            tree: SessionTree::new(),
            registry,
            policy_gate,
            inbox: SteeringInbox::new(),
            events: EventStream::new(),
            artifacts: ArtifactStoreView::default(),
            budget,
            spawn_depth: 0,
            now_ms: created_at_ms,
            id_seq: 0,
            pending_jsonl: Vec::new(),
            active_turn: None,
        };
        let payload = NodePayload::SessionOpen {
            schema_version: SESSION_SCHEMA_VERSION,
            workspace_root,
            created_at_ms,
        };
        rt.append_node(None, None, payload)?;
        Ok(rt)
    }

    /// Reset this [`KernelRuntime`] to a fresh `SessionTree` with a new
    /// `session_open` root node while preserving `registry` and `policy_gate`.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] if `session_open` serialization fails.
    pub fn reset_session(
        &mut self,
        run_id: String,
        workspace_root: String,
        created_at_ms: u64,
    ) -> Result<(), KernelError> {
        self.run_id = run_id;
        self.tree = SessionTree::new();
        self.inbox = SteeringInbox::new();
        self.events = EventStream::new();
        self.artifacts = ArtifactStoreView::default();
        self.budget = BudgetSnapshot {
            cap: self.policy_gate.baseline_budget(),
            ..BudgetSnapshot::default()
        };
        self.spawn_depth = 0;
        self.now_ms = created_at_ms;
        self.id_seq = 0;
        self.pending_jsonl.clear();
        self.active_turn = None;
        self.append_node(
            None,
            None,
            NodePayload::SessionOpen {
                schema_version: SESSION_SCHEMA_VERSION,
                workspace_root,
                created_at_ms,
            },
        )?;
        Ok(())
    }

    /// Resume a [`KernelRuntime`] from an already-validated [`SessionTree`].
    #[must_use]
    pub fn from_tree(
        run_id: String,
        tree: SessionTree,
        registry: ExtensionRegistry,
        policy_gate: PolicyGate,
        now_ms: u64,
    ) -> Self {
        let mut rt = Self {
            run_id,
            tree: SessionTree::new(),
            registry,
            policy_gate,
            inbox: SteeringInbox::new(),
            events: EventStream::new(),
            artifacts: ArtifactStoreView::default(),
            budget: BudgetSnapshot::default(),
            spawn_depth: 0,
            now_ms,
            id_seq: 0,
            pending_jsonl: Vec::new(),
            active_turn: None,
        };
        rt.replace_tree(tree, now_ms);
        rt
    }

    /// Replace the active [`SessionTree`] (e.g., when switching sessions in a
    /// surface) while preserving `registry` and `policy_gate` and recomputing
    /// `budget` usage from the active branch.
    pub fn replace_tree(&mut self, tree: SessionTree, now_ms: u64) {
        self.id_seq = tree.all_nodes().len() as u64;
        self.inbox = SteeringInbox::new();
        self.events = EventStream::new();
        self.artifacts = ArtifactStoreView::default();
        self.spawn_depth = 0;
        self.now_ms = now_ms;
        self.pending_jsonl.clear();
        self.active_turn = None;

        let mut budget = BudgetSnapshot {
            cap: self.policy_gate.baseline_budget(),
            ..BudgetSnapshot::default()
        };
        if let Ok(slice) = tree.slice_to(None) {
            for node in slice.nodes() {
                if let Ok(NodePayload::StepResult(res)) = node.decode_payload() {
                    budget.record_step(res.usage());
                }
            }
        }
        self.budget = budget;
        self.tree = tree;
    }

    /// Update the deterministic kernel clock supplied by `gestalt-runtime`.
    pub fn set_now_ms(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    /// Set the current spawn nesting depth (used when instantiating child runtimes).
    pub fn set_spawn_depth(&mut self, depth: u8) {
        self.spawn_depth = depth;
    }

    /// Borrow the underlying [`SessionTree`].
    #[must_use]
    pub fn session_tree(&self) -> &SessionTree {
        &self.tree
    }

    /// Borrow the [`ExtensionRegistry`].
    #[must_use]
    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }

    /// Mutably borrow the [`ExtensionRegistry`] (e.g., for `/reload <ext>` between Turns).
    pub fn registry_mut(&mut self) -> &mut ExtensionRegistry {
        &mut self.registry
    }

    /// Borrow the [`PolicyGate`].
    #[must_use]
    pub fn policy_gate(&self) -> &PolicyGate {
        &self.policy_gate
    }

    /// Borrow the live [`BudgetSnapshot`].
    #[must_use]
    pub fn budget_snapshot(&self) -> &BudgetSnapshot {
        &self.budget
    }

    /// Drain buffered [`RunEvent`]s in FIFO order.
    pub fn drain_events(&mut self) -> Vec<RunEvent> {
        self.events.drain()
    }

    /// Drain newly appended compact JSONL lines for atomic disk persistence.
    pub fn drain_pending_jsonl_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_jsonl)
    }

    /// Enqueue a [`SteeringSignal`], persisting it immediately on `SessionTree`.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] if node serialization fails.
    pub fn enqueue_steering(
        &mut self,
        kind: SteeringKind,
        content: Value,
        source: SteeringSource,
        timestamp_ms: u64,
    ) -> Result<SteeringSignal, KernelError> {
        self.now_ms = timestamp_ms;
        let id = self.next_id();
        let signal = SteeringSignal {
            id: id.clone(),
            kind,
            content: content.clone(),
            source,
            timestamp_ms,
        };
        let (turn_id, phase_id) = self.active_ids();
        self.append_node(
            turn_id,
            phase_id,
            NodePayload::Steering {
                id,
                kind,
                content,
                source,
            },
        )?;
        self.inbox.enqueue(signal.clone());
        Ok(signal)
    }

    /// Pop the next queued `FollowUp` signal (including any un-drained `Steer`
    /// auto-promoted at Turn end) to initiate Turn `N + 1`.
    pub fn pop_next_follow_up(&mut self) -> Option<SteeringSignal> {
        self.inbox.pop_next_follow_up()
    }

    /// Return the number of queued `FollowUp` signals waiting for the next Turn.
    #[must_use]
    pub fn pending_follow_up_count(&self) -> usize {
        self.inbox.pending_follow_up_count()
    }

    /// Fork the session from `from_node_id` (or active head) into `branch_id`.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] if the target node is missing.
    pub fn fork_session(
        &mut self,
        branch_id: String,
        from_node_id: Option<&str>,
        timestamp_ms: u64,
    ) -> Result<String, KernelError> {
        self.now_ms = timestamp_ms;
        let audit_id = self.next_node_id();
        let line = {
            let node = self
                .tree
                .fork(branch_id, from_node_id, audit_id.clone(), timestamp_ms)?;
            node.to_jsonl_line()?
        };
        self.pending_jsonl.push(line);
        self.events.emit(RunEvent::NodeAppended {
            node_id: audit_id.clone(),
            kind: "branch_head".to_string(),
        });
        Ok(audit_id)
    }

    /// Rewind the active conversation head to `target_node_id`.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] if `target_node_id` is missing.
    pub fn rewind_session(
        &mut self,
        branch_id: String,
        target_node_id: &str,
        timestamp_ms: u64,
    ) -> Result<(), KernelError> {
        self.now_ms = timestamp_ms;
        let audit_id = self.next_node_id();
        let line = {
            let node =
                self.tree
                    .rewind(branch_id, target_node_id, audit_id.clone(), timestamp_ms)?;
            node.to_jsonl_line()?
        };
        self.pending_jsonl.push(line);
        self.events.emit(RunEvent::NodeAppended {
            node_id: audit_id,
            kind: "branch_head".to_string(),
        });
        Ok(())
    }

    /// Append a user-space namespaced node (e.g., `"std:compaction"` -> `"custom:std:compaction"`)
    /// to the active `SessionTree` branch.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] if serialization fails.
    pub fn append_custom_node(
        &mut self,
        namespace: String,
        data: Value,
        timestamp_ms: u64,
    ) -> Result<String, KernelError> {
        self.now_ms = timestamp_ms;
        let (turn_id, phase_id) = self.active_ids();
        self.append_node(turn_id, phase_id, NodePayload::Custom { namespace, data })
    }

    /// Start a new user-facing Turn using `preset_or_loop`.
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::InvalidState`] if a Turn is already active, or
    /// [`KernelError::UnresolvedExtensionSlot`] if the preset/loop is missing.
    pub fn start_turn(
        &mut self,
        prompt: Value,
        preset_or_loop: &str,
        extra_decorators: &[String],
        artifacts: ArtifactStoreView,
        timestamp_ms: u64,
    ) -> Result<String, KernelError> {
        if self.active_turn.is_some() {
            return Err(KernelError::InvalidState(
                "cannot start turn while another turn is active".to_string(),
            ));
        }
        self.now_ms = timestamp_ms;
        let turn_id = self.next_id();
        let phase_id = self.next_id();
        let stack = self
            .registry
            .resolve_phase_stack(preset_or_loop, extra_decorators, None)?;
        self.artifacts = artifacts;

        self.append_node(
            Some(turn_id.clone()),
            None,
            NodePayload::TurnStart {
                turn_id: turn_id.clone(),
                prompt,
                artifact_snapshots: self.artifacts.snapshots(),
            },
        )?;
        self.open_phase_node(&turn_id, &phase_id, Value::Null, &stack)?;

        self.active_turn = Some(ActiveTurn {
            turn_id: turn_id.clone(),
            usage: Usage::default(),
            phase: ActivePhase {
                phase_id,
                handoff: Value::Null,
                config: stack.config,
                builder_id: stack.builder_id,
                external_builder: stack.external_builder,
                cached_external_context: None,
                in_process_loop: stack.in_process_loop,
                external_loop: stack.external_loop,
                external_state_token: None,
            },
            pending_batch: None,
            awaiting_pause: None,
        });

        Ok(turn_id)
    }

    /// Switch the active Phase mid-session (`/mode <preset>`).
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] if no Turn is active or the target preset/loop is missing.
    pub fn switch_mode(
        &mut self,
        preset_or_loop: &str,
        handoff: Value,
        config_override: Option<LoopConfig>,
        timestamp_ms: u64,
    ) -> Result<String, KernelError> {
        self.now_ms = timestamp_ms;
        let Some(turn) = self.active_turn.as_ref() else {
            return Err(KernelError::InvalidState(
                "cannot switch mode without an active turn".to_string(),
            ));
        };
        let turn_id = turn.turn_id.clone();
        let old_phase_id = turn.phase.phase_id.clone();

        let stack =
            self.registry
                .resolve_phase_stack(preset_or_loop, &[], config_override.as_ref())?;

        self.append_node(
            Some(turn_id.clone()),
            Some(old_phase_id.clone()),
            NodePayload::PhaseSeal {
                phase_id: old_phase_id,
                handoff: handoff.clone(),
            },
        )?;

        let new_phase_id = self.next_id();
        self.open_phase_node(&turn_id, &new_phase_id, handoff.clone(), &stack)?;

        if let Some(turn_mut) = self.active_turn.as_mut() {
            turn_mut.phase = ActivePhase {
                phase_id: new_phase_id.clone(),
                handoff,
                config: stack.config,
                builder_id: stack.builder_id,
                external_builder: stack.external_builder,
                cached_external_context: None,
                in_process_loop: stack.in_process_loop,
                external_loop: stack.external_loop,
                external_state_token: None,
            };
        }
        Ok(new_phase_id)
    }

    /// Advance the Sans-I/O state machine by one [`DriverDirective`].
    ///
    /// # Errors
    ///
    /// Returns [`KernelError`] if no Turn is active or if `DriverResponse` does
    /// not match the current state.
    pub fn next_action(
        &mut self,
        response: DriverResponse,
    ) -> Result<DriverDirective, KernelError> {
        if self.active_turn.is_none() {
            return Err(KernelError::InvalidState(
                "next_action called with no active turn".to_string(),
            ));
        }

        // Check preemptive Abort before processing normal results.
        if let Some(abort_sig) = self.inbox.take_abort() {
            return self.handle_turn_abort(&abort_sig.content.to_string());
        }

        // Check preemptive UrgentSteer while a batch is in flight.
        if self.inbox.has_urgent_interrupt() {
            let has_in_flight = self
                .active_turn
                .as_ref()
                .and_then(|t| t.pending_batch.as_ref())
                .is_some_and(|b| !b.in_flight.is_empty());
            if has_in_flight {
                self.inbox.acknowledge_urgent_interrupt();
                let mut turn = self
                    .active_turn
                    .take()
                    .ok_or_else(|| KernelError::InvalidState("missing active turn".to_string()))?;
                let batch = turn.pending_batch.take().ok_or_else(|| {
                    KernelError::InvalidState("missing pending batch".to_string())
                })?;
                let in_flight_ids: Vec<String> = batch
                    .in_flight
                    .iter()
                    .map(|e| e.effect_id.clone())
                    .collect();
                let aborted = synthesize_aborted_results(&in_flight_ids, "urgent steer interrupt");
                for res in &aborted {
                    self.append_node(
                        Some(turn.turn_id.clone()),
                        Some(turn.phase.phase_id.clone()),
                        NodePayload::StepResult(res.clone()),
                    )?;
                }
                let merged = merge_batch_results(
                    batch.total_len,
                    batch.pre_denied,
                    &batch.dispatched_indices,
                    aborted,
                );
                self.active_turn = Some(turn);
                return self.step_active_loop(Some(merged));
            }
        }

        match response {
            DriverResponse::Poll => self.step_active_loop(None),
            DriverResponse::EffectResults(results) => self.handle_effect_results(results),
            DriverResponse::VerificationResult {
                rule_id,
                pending_effect_id,
                verifier_result,
            } => self.handle_verification_result(&rule_id, &pending_effect_id, verifier_result),
            DriverResponse::ExternalLoopOutput {
                action,
                state_token,
            } => {
                if let Some(turn) = self.active_turn.as_mut() {
                    turn.phase.external_state_token = state_token;
                }
                self.handle_loop_action(action)
            }
            DriverResponse::ExternalBuilderOutput { model_input } => {
                if let Some(turn) = self.active_turn.as_mut() {
                    turn.phase.cached_external_context = Some(model_input);
                }
                self.step_active_loop(None)
            }
            DriverResponse::PauseResolution {
                pause_id,
                effect_id,
                resolution,
            } => self.handle_pause_resolution(pause_id, effect_id, resolution),
        }
    }

    fn handle_turn_abort(&mut self, reason: &str) -> Result<DriverDirective, KernelError> {
        let mut turn = self
            .active_turn
            .take()
            .ok_or_else(|| KernelError::InvalidState("missing active turn on abort".to_string()))?;

        if let Some(batch) = turn.pending_batch.take() {
            let ids: Vec<String> = batch
                .in_flight
                .iter()
                .map(|e| e.effect_id.clone())
                .collect();
            let aborted = synthesize_aborted_results(&ids, reason);
            for res in aborted {
                self.append_node(
                    Some(turn.turn_id.clone()),
                    Some(turn.phase.phase_id.clone()),
                    NodePayload::StepResult(res),
                )?;
            }
        }

        self.append_node(
            Some(turn.turn_id.clone()),
            Some(turn.phase.phase_id.clone()),
            NodePayload::PhaseSeal {
                phase_id: turn.phase.phase_id.clone(),
                handoff: Value::Null,
            },
        )?;
        self.append_node(
            Some(turn.turn_id.clone()),
            None,
            NodePayload::TurnEnd {
                turn_id: turn.turn_id.clone(),
                status: TurnStatus::Aborted,
                output: Value::Null,
                usage: turn.usage,
            },
        )?;
        self.events.emit(RunEvent::TurnAborted {
            turn_id: turn.turn_id.clone(),
            reason: reason.to_string(),
        });
        Ok(DriverDirective::TurnAborted {
            turn_id: turn.turn_id,
            reason: reason.to_string(),
        })
    }

    fn handle_effect_results(
        &mut self,
        results: Vec<StepResult>,
    ) -> Result<DriverDirective, KernelError> {
        let mut turn = self
            .active_turn
            .take()
            .ok_or_else(|| KernelError::InvalidState("missing active turn".to_string()))?;
        let batch = turn
            .pending_batch
            .take()
            .ok_or_else(|| KernelError::InvalidState("unexpected EffectResults".to_string()))?;

        for res in &results {
            if let Some(u) = res.usage() {
                turn.usage.add_assign(u);
                self.budget.record_usage(u);
            }
            self.append_node(
                Some(turn.turn_id.clone()),
                Some(turn.phase.phase_id.clone()),
                NodePayload::StepResult(res.clone()),
            )?;
        }
        self.budget.record_step(None);

        let merged = merge_batch_results(
            batch.total_len,
            batch.pre_denied,
            &batch.dispatched_indices,
            results,
        );
        self.active_turn = Some(turn);
        self.step_active_loop(Some(merged))
    }

    fn handle_verification_result(
        &mut self,
        rule_id: &str,
        pending_effect_id: &str,
        verifier_result: StepResult,
    ) -> Result<DriverDirective, KernelError> {
        let mut turn = self
            .active_turn
            .take()
            .ok_or_else(|| KernelError::InvalidState("missing active turn".to_string()))?;
        let mut batch = turn
            .pending_batch
            .take()
            .ok_or_else(|| KernelError::InvalidState("missing pending batch".to_string()))?;

        self.append_node(
            Some(turn.turn_id.clone()),
            Some(turn.phase.phase_id.clone()),
            NodePayload::StepResult(verifier_result.clone()),
        )?;

        let Some((held_rule, pending_env, orig_idx)) = batch.pending_verification.take() else {
            return Err(KernelError::InvalidState(
                "no verification pending".to_string(),
            ));
        };
        if held_rule != rule_id || pending_env.effect_id != pending_effect_id {
            return Err(KernelError::InvalidState(
                "verification response mismatch".to_string(),
            ));
        }

        match self
            .policy_gate
            .resolve_verification(rule_id, &pending_env, &verifier_result)
        {
            PolicyDecision::Allow => {
                batch
                    .dispatched_indices
                    .insert(pending_env.effect_id.clone(), orig_idx);
                batch.in_flight.push(pending_env);
            }
            PolicyDecision::Deny { reason }
            | PolicyDecision::RequiresVerification {
                rule_id: reason, ..
            } => {
                let denied = StepResult::Denied {
                    effect_id: pending_env.effect_id.clone(),
                    reason: reason.clone(),
                };
                self.append_node(
                    Some(turn.turn_id.clone()),
                    Some(turn.phase.phase_id.clone()),
                    NodePayload::StepResult(denied.clone()),
                )?;
                self.events.emit(RunEvent::PolicyDenied {
                    effect_id: pending_env.effect_id,
                    reason,
                });
                batch.pre_denied.insert(orig_idx, denied);
            }
        }

        if let Some((next_rule, next_verifier, next_pending, next_idx)) =
            batch.verification_queue.pop_front()
        {
            batch.pending_verification = Some((next_rule.clone(), next_pending.clone(), next_idx));
            turn.pending_batch = Some(batch);
            self.active_turn = Some(turn);
            return Ok(DriverDirective::VerifyEffect {
                rule_id: next_rule,
                verifier: Box::new(next_verifier),
                pending: Box::new(next_pending),
            });
        }

        if batch.in_flight.is_empty() {
            let merged = merge_batch_results(
                batch.total_len,
                batch.pre_denied,
                &batch.dispatched_indices,
                Vec::new(),
            );
            self.active_turn = Some(turn);
            self.step_active_loop(Some(merged))
        } else {
            let to_dispatch = batch.in_flight.clone();
            turn.pending_batch = Some(batch);
            self.active_turn = Some(turn);
            Ok(DriverDirective::DispatchEffects {
                effects: to_dispatch,
            })
        }
    }

    fn handle_pause_resolution(
        &mut self,
        pause_id: String,
        effect_id: String,
        resolution: Value,
    ) -> Result<DriverDirective, KernelError> {
        let mut turn = self
            .active_turn
            .take()
            .ok_or_else(|| KernelError::InvalidState("missing active turn".to_string()))?;
        let Some((expected_pause, expected_eff)) = turn.awaiting_pause.take() else {
            return Err(KernelError::InvalidState(
                "no pause awaiting resolution".to_string(),
            ));
        };
        if expected_pause != pause_id || expected_eff != effect_id {
            return Err(KernelError::InvalidState("pause_id mismatch".to_string()));
        }

        self.append_node(
            Some(turn.turn_id.clone()),
            Some(turn.phase.phase_id.clone()),
            NodePayload::PauseResolved {
                pause_id,
                effect_id: effect_id.clone(),
                resolution: resolution.clone(),
            },
        )?;
        let res = StepResult::PauseResolved {
            effect_id,
            resolution,
        };
        self.append_node(
            Some(turn.turn_id.clone()),
            Some(turn.phase.phase_id.clone()),
            NodePayload::StepResult(res.clone()),
        )?;
        self.active_turn = Some(turn);
        self.step_active_loop(Some(vec![res]))
    }

    fn step_active_loop(
        &mut self,
        results: Option<Vec<StepResult>>,
    ) -> Result<DriverDirective, KernelError> {
        let mut turn = self
            .active_turn
            .take()
            .ok_or_else(|| KernelError::InvalidState("missing active turn".to_string()))?;

        if let Some(ext) = turn.phase.external_loop.clone() {
            let directive = DriverDirective::InvokeExternalLoop {
                ext_name: ext.ext_name,
                loop_id: ext.id,
                phase_id: turn.phase.phase_id.clone(),
                turn_id: turn.turn_id.clone(),
                handoff: turn.phase.handoff.clone(),
                config: turn.phase.config.clone(),
                state_token: turn.phase.external_state_token.clone(),
                results,
            };
            self.active_turn = Some(turn);
            return Ok(directive);
        }

        let slice = self.tree.slice_to(None)?;
        if results.is_none()
            && turn.phase.cached_external_context.is_none()
            && let Some(ref ext_b) = turn.phase.external_builder
        {
            let directive = DriverDirective::InvokeExternalBuilder {
                ext_name: ext_b.ext_name.clone(),
                builder_id: ext_b.id.clone(),
                session_slice: slice.nodes().to_vec(),
                artifacts: self.artifacts.entries().clone(),
                caps: ModelCapabilities::default(),
            };
            self.active_turn = Some(turn);
            return Ok(directive);
        }

        let Some(mut loop_impl) = turn.phase.in_process_loop.take() else {
            return Err(KernelError::InvalidState(
                "active phase has neither in-process nor external loop".to_string(),
            ));
        };

        let builder = self
            .registry
            .builders
            .get(&turn.phase.builder_id)
            .map(AsRef::as_ref);
        let precomputed = turn.phase.cached_external_context.take();
        let mut id_counter = self.id_seq;
        let (action, emitted_custom) = {
            let mut ctx = PhaseLoopContext {
                run_id: &self.run_id,
                turn_id: &turn.turn_id,
                phase_id: &turn.phase.phase_id,
                handoff: &turn.phase.handoff,
                config: &turn.phase.config,
                session: slice,
                artifacts: &self.artifacts,
                budget: &self.budget,
                builder,
                precomputed_context: precomputed.as_ref(),
                inbox: &mut self.inbox,
                now_ms: self.now_ms,
                id_counter: &mut id_counter,
                pending_custom_nodes: Vec::new(),
            };
            let action = loop_impl.step(&mut ctx, results);
            (action, ctx.pending_custom_nodes)
        };
        self.id_seq = id_counter;
        let turn_id_for_custom = turn.turn_id.clone();
        let phase_id_for_custom = turn.phase.phase_id.clone();
        turn.phase.in_process_loop = Some(loop_impl);
        self.active_turn = Some(turn);
        for (namespace, data) in emitted_custom {
            self.append_node(
                Some(turn_id_for_custom.clone()),
                Some(phase_id_for_custom.clone()),
                NodePayload::Custom { namespace, data },
            )?;
        }
        self.handle_loop_action(action)
    }

    fn handle_loop_action(&mut self, action: LoopAction) -> Result<DriverDirective, KernelError> {
        let mut turn = self
            .active_turn
            .take()
            .ok_or_else(|| KernelError::InvalidState("missing active turn".to_string()))?;

        match action {
            LoopAction::Done(output) => {
                self.append_node(
                    Some(turn.turn_id.clone()),
                    Some(turn.phase.phase_id.clone()),
                    NodePayload::PhaseSeal {
                        phase_id: turn.phase.phase_id.clone(),
                        handoff: Value::Null,
                    },
                )?;
                let _promoted = self.inbox.promote_undrained_to_follow_up();
                self.append_node(
                    Some(turn.turn_id.clone()),
                    None,
                    NodePayload::TurnEnd {
                        turn_id: turn.turn_id.clone(),
                        status: TurnStatus::Completed,
                        output: output.clone(),
                        usage: turn.usage,
                    },
                )?;
                self.events.emit(RunEvent::TurnCompleted {
                    turn_id: turn.turn_id.clone(),
                    output: output.clone(),
                    usage: turn.usage,
                });
                Ok(DriverDirective::TurnComplete {
                    turn_id: turn.turn_id,
                    output,
                    usage: turn.usage,
                })
            }
            LoopAction::Yield(effects) => {
                if effects.is_empty() {
                    self.active_turn = Some(turn);
                    return self.step_active_loop(Some(Vec::new()));
                }

                let mut envelopes = Vec::with_capacity(effects.len());
                for eff in effects {
                    let env = EffectEnvelope {
                        effect_id: self.next_id(),
                        effect: eff,
                    };
                    self.append_node(
                        Some(turn.turn_id.clone()),
                        Some(turn.phase.phase_id.clone()),
                        NodePayload::Effect {
                            envelope: env.clone(),
                            state_token: turn.phase.external_state_token.clone(),
                        },
                    )?;
                    envelopes.push(env);
                }

                // Handle kernel-owned `Switch` or `Pause` when yielded as single control effects.
                if envelopes.len() == 1 {
                    match &envelopes[0].effect {
                        Effect::Switch {
                            next_loop,
                            handoff,
                            config,
                        } => {
                            let eff_id = envelopes[0].effect_id.clone();
                            let next_loop = next_loop.clone();
                            let handoff = handoff.clone();
                            let config = config.clone();
                            self.active_turn = Some(turn);
                            let new_phase_id =
                                self.switch_mode(&next_loop, handoff, config, self.now_ms)?;
                            let res = StepResult::Switched {
                                effect_id: eff_id,
                                new_phase_id: new_phase_id.clone(),
                            };
                            let turn_id = self.active_turn.as_ref().map(|t| t.turn_id.clone());
                            self.append_node(
                                turn_id,
                                Some(new_phase_id),
                                NodePayload::StepResult(res.clone()),
                            )?;
                            return self.step_active_loop(Some(vec![res]));
                        }
                        Effect::Pause { reason, payload } => {
                            let pause_id = self.next_id();
                            let eff_id = envelopes[0].effect_id.clone();
                            let reason = *reason;
                            let payload = payload.clone();
                            self.append_node(
                                Some(turn.turn_id.clone()),
                                Some(turn.phase.phase_id.clone()),
                                NodePayload::Pause {
                                    pause_id: pause_id.clone(),
                                    effect_id: eff_id.clone(),
                                    reason,
                                    payload: payload.clone(),
                                },
                            )?;
                            self.events.emit(RunEvent::Paused {
                                pause_id: pause_id.clone(),
                                effect_id: eff_id.clone(),
                                reason,
                                payload: payload.clone(),
                            });
                            turn.awaiting_pause = Some((pause_id.clone(), eff_id.clone()));
                            self.active_turn = Some(turn);
                            return Ok(DriverDirective::WaitForPause {
                                pause_id,
                                effect_id: eff_id,
                                reason,
                                payload,
                            });
                        }
                        _ => {}
                    }
                }

                let slice = self.tree.slice_to(None)?;
                let registered_tools = self.registry.registered_tool_names();
                let decisions = self.policy_gate.evaluate_batch(
                    &envelopes,
                    &turn.phase.config,
                    &self.budget,
                    self.spawn_depth,
                    &registered_tools,
                    &slice,
                );

                let mut batch = PendingBatch {
                    total_len: envelopes.len(),
                    pre_denied: BTreeMap::new(),
                    dispatched_indices: BTreeMap::new(),
                    in_flight: Vec::new(),
                    pending_verification: None,
                    verification_queue: VecDeque::new(),
                };

                for (idx, decision) in decisions.into_iter().enumerate() {
                    match decision {
                        BatchItemDecision::Approved(env) => {
                            batch.dispatched_indices.insert(env.effect_id.clone(), idx);
                            batch.in_flight.push(env);
                        }
                        BatchItemDecision::Denied { effect_id, reason } => {
                            let denied = StepResult::Denied {
                                effect_id: effect_id.clone(),
                                reason: reason.clone(),
                            };
                            self.append_node(
                                Some(turn.turn_id.clone()),
                                Some(turn.phase.phase_id.clone()),
                                NodePayload::StepResult(denied.clone()),
                            )?;
                            self.events
                                .emit(RunEvent::PolicyDenied { effect_id, reason });
                            batch.pre_denied.insert(idx, denied);
                        }
                        BatchItemDecision::NeedsVerification {
                            rule_id,
                            verifier_effect,
                            pending,
                        } => {
                            let verifier_env = EffectEnvelope {
                                effect_id: self.next_id(),
                                effect: *verifier_effect,
                            };
                            self.append_node(
                                Some(turn.turn_id.clone()),
                                Some(turn.phase.phase_id.clone()),
                                NodePayload::Effect {
                                    envelope: verifier_env.clone(),
                                    state_token: None,
                                },
                            )?;
                            batch.verification_queue.push_back((
                                rule_id,
                                verifier_env,
                                *pending,
                                idx,
                            ));
                        }
                    }
                }

                if let Some((rule_id, verifier_env, pending_env, idx)) =
                    batch.verification_queue.pop_front()
                {
                    batch.pending_verification = Some((rule_id.clone(), pending_env.clone(), idx));
                    turn.pending_batch = Some(batch);
                    self.active_turn = Some(turn);
                    return Ok(DriverDirective::VerifyEffect {
                        rule_id,
                        verifier: Box::new(verifier_env),
                        pending: Box::new(pending_env),
                    });
                }

                if batch.in_flight.is_empty() {
                    let merged = merge_batch_results(
                        batch.total_len,
                        batch.pre_denied,
                        &batch.dispatched_indices,
                        Vec::new(),
                    );
                    self.active_turn = Some(turn);
                    return self.step_active_loop(Some(merged));
                }

                let to_dispatch = batch.in_flight.clone();
                turn.pending_batch = Some(batch);
                self.active_turn = Some(turn);
                Ok(DriverDirective::DispatchEffects {
                    effects: to_dispatch,
                })
            }
        }
    }

    fn open_phase_node(
        &mut self,
        turn_id: &str,
        phase_id: &str,
        _handoff: Value,
        stack: &ResolvedPhaseStack,
    ) -> Result<(), KernelError> {
        self.append_node(
            Some(turn_id.to_string()),
            Some(phase_id.to_string()),
            NodePayload::PhaseOpen {
                phase_id: phase_id.to_string(),
                preset_id: stack.preset_id.clone(),
                base_loop_id: stack.base_loop_id.clone(),
                loop_digest: stack.loop_digest.clone(),
                decorator_digests: stack.decorator_digests.clone(),
                builder_id: stack.builder_id.clone(),
                builder_digest: stack.builder_digest.clone(),
                config: stack.config.clone(),
            },
        )?;
        self.events.emit(RunEvent::PhaseSwitched {
            phase_id: phase_id.to_string(),
            preset_id: stack.preset_id.clone(),
            base_loop_id: stack.base_loop_id.clone(),
        });
        Ok(())
    }

    fn append_node(
        &mut self,
        turn_id: Option<String>,
        phase_id: Option<String>,
        payload: NodePayload,
    ) -> Result<String, KernelError> {
        let node_id = self.next_node_id();
        let (kind, line) = {
            let node =
                self.tree
                    .append(node_id.clone(), turn_id, phase_id, self.now_ms, &payload)?;
            (node.kind.clone(), node.to_jsonl_line()?)
        };
        self.pending_jsonl.push(line);
        self.events.emit(RunEvent::NodeAppended {
            node_id: node_id.clone(),
            kind,
        });
        Ok(node_id)
    }

    fn active_ids(&self) -> (Option<String>, Option<String>) {
        match &self.active_turn {
            Some(t) => (Some(t.turn_id.clone()), Some(t.phase.phase_id.clone())),
            None => (None, None),
        }
    }

    fn next_id(&mut self) -> String {
        self.id_seq += 1;
        format!("id_{}_{}", self.run_id, self.id_seq)
    }

    fn next_node_id(&mut self) -> String {
        self.id_seq += 1;
        format!("n_{}_{}", self.run_id, self.id_seq)
    }
}

fn merge_batch_results(
    total_len: usize,
    pre_denied: BTreeMap<usize, StepResult>,
    dispatched_indices: &BTreeMap<String, usize>,
    results: Vec<StepResult>,
) -> Vec<StepResult> {
    let mut ordered: Vec<Option<StepResult>> = vec![None; total_len];
    for (idx, res) in pre_denied {
        if idx < ordered.len() {
            ordered[idx] = Some(res);
        }
    }
    for res in results {
        if let Some(&idx) = dispatched_indices.get(res.effect_id())
            && idx < ordered.len()
        {
            ordered[idx] = Some(res);
        }
    }
    ordered.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn follow_up_queue_accessors_expose_promoted_and_queued_signals() {
        let mut reg = ExtensionRegistry::new();
        struct DoneLoop;
        impl AgentLoop for DoneLoop {
            fn id(&self) -> &str {
                "done"
            }
            fn digest(&self) -> &str {
                "d1"
            }
            fn step(
                &mut self,
                _ctx: &mut dyn LoopContext,
                _results: Option<Vec<StepResult>>,
            ) -> LoopAction {
                LoopAction::Done(json!("ok"))
            }
        }
        struct DoneFactory;
        impl AgentLoopFactory for DoneFactory {
            fn id(&self) -> &str {
                "done"
            }
            fn digest(&self) -> &str {
                "d1"
            }
            fn create(
                &self,
                _config: &LoopConfig,
                _registry: &ExtensionRegistry,
            ) -> Box<dyn AgentLoop> {
                Box::new(DoneLoop)
            }
        }
        struct NullBuilder;
        impl ContextBuilder for NullBuilder {
            fn id(&self) -> &str {
                "default"
            }
            fn digest(&self) -> &str {
                "b1"
            }
            fn build(
                &self,
                _session: &SessionSlice,
                _artifacts: &ArtifactStoreView,
                _caps: &ModelCapabilities,
            ) -> Value {
                Value::Null
            }
        }
        reg.loops.insert("done".into(), Box::new(DoneFactory));
        reg.builders.insert("default".into(), Box::new(NullBuilder));
        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let mut rt = KernelRuntime::new("r1".into(), "/ws".into(), 100, reg, gate).expect("init");
        rt.start_turn(json!("hi"), "done", &[], ArtifactStoreView::default(), 101)
            .expect("start");
        // Enqueue a Steer without draining it before Done
        rt.enqueue_steering(
            SteeringKind::Steer,
            json!("follow me"),
            SteeringSource::User,
            102,
        )
        .expect("steer");
        let dir = rt.next_action(DriverResponse::Poll).expect("poll");
        assert!(matches!(dir, DriverDirective::TurnComplete { .. }));
        assert_eq!(rt.pending_follow_up_count(), 1);
        let sig = rt.pop_next_follow_up().expect("promoted follow up");
        assert_eq!(sig.kind, SteeringKind::FollowUp);
        assert_eq!(sig.content, json!("follow me"));
        assert_eq!(rt.pending_follow_up_count(), 0);
    }
}
