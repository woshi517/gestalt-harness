#![forbid(unsafe_code)]
//! Typed `Effect` bus, `StepResult`, `BudgetCap`, and `LoopConfig` primitives.
//!
//! Loops never perform I/O directly; they yield [`Effect`] values wrapped in
//! [`EffectEnvelope`]s and receive [`StepResult`] values recorded on the
//! `SessionTree`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Paradigm of the model invoked by an [`Effect::Model`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelKind {
    /// Generative language model (text, reasoning, structured tool calls).
    Llm,
    /// Fast typed System One classifier/router returning a discrete choice.
    Decision,
    /// Dense vector embedding model.
    Embedding,
    /// Action simulation model predicting state transitions.
    WorldModel,
    /// GUI/screen action prediction model.
    ComputerUse,
}

/// Side-effect classification declared by tools and evaluated by `PolicyGate`.
///
/// Ordered from least impactful (`Read`) to most impactful (`Irreversible`)
/// so policy checks can compare against maximum allowed classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectClass {
    /// Pure read-only inspection with no state mutation.
    Read,
    /// Mutation that can be safely retried or overwritten without compounding.
    Idempotent,
    /// Mutation with external or destructive side effects.
    Irreversible,
}

/// Persistence mode for [`Effect::Save`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveMode {
    /// Write candidate changes to an isolated staging area with SHA-256 digests.
    Stage,
    /// Promote staged changes into live protected paths (gated by `PolicyGate`).
    Promote,
}

/// A single unified-diff hunk used in [`ArtifactPatch::ApplyDiff`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// 1-based starting line number in the original file.
    pub old_start: u32,
    /// Number of original lines matched by `target`.
    pub old_lines: u32,
    /// Number of replacement lines in `replacement`.
    pub new_lines: u32,
    /// Exact text expected to match in the original file.
    pub target: String,
    /// Replacement text substituted for `target`.
    pub replacement: String,
}

/// Mutation payload for [`Effect::Save`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "patch_kind", rename_all = "snake_case")]
pub enum ArtifactPatch {
    /// Create or completely replace an artifact file with `content`.
    WriteFile {
        /// Full UTF-8 file contents to write.
        content: String,
    },
    /// Apply targeted diff hunks to an existing artifact file.
    ApplyDiff {
        /// Ordered list of diff hunks to apply.
        hunks: Vec<DiffHunk>,
    },
    /// Delete the target artifact file.
    Delete,
}

/// Reason an [`Effect::Pause`] suspends execution to await a client response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// Requests explicit human or policy approval before continuing.
    Approval,
    /// Requests interactive user input (e.g., credentials or clarification).
    Input,
    /// Pauses at a checkpoint for inspection before the next Phase/Step.
    Checkpoint,
}

/// Completion notification behavior for detached [`Effect::Spawn`] subagents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnNotifyMode {
    /// Record `custom:spawn:completed` passively for the next Turn (default).
    #[default]
    Passive,
    /// Record `custom:spawn:completed` and enqueue a `SteeringKind::FollowUp` Turn.
    FollowUp,
}

/// Filesystem isolation mode for a child subagent launched via [`Effect::Spawn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    /// Child subagent can read workspace files but cannot mutate them.
    #[default]
    ReadOnly,
    /// Child subagent shares the workspace (forbidden for concurrent spawns).
    Shared,
    /// Child subagent runs in an isolated git worktree or workspace copy.
    Worktree,
}

/// Monotonic resource and cost ceilings enforced by `PolicyGate`.
///
/// Costs use integer `micro_usd` (`1_000_000 micro_usd == $1.00 USD`) to avoid
/// floating-point representation drift in content-addressed hashes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetCap {
    /// Maximum number of Steps permitted.
    pub max_steps: Option<u64>,
    /// Maximum total input + output token units permitted.
    pub max_tokens: Option<u64>,
    /// Maximum monetary spend in integer micro-USD (`1_000_000 = $1.00`).
    pub max_cost_micro_usd: Option<u64>,
}

impl BudgetCap {
    /// Return a tighter budget cap taking the minimum of each defined limit.
    #[must_use]
    pub fn tighten(&self, other: &Self) -> Self {
        Self {
            max_steps: min_opt(self.max_steps, other.max_steps),
            max_tokens: min_opt(self.max_tokens, other.max_tokens),
            max_cost_micro_usd: min_opt(self.max_cost_micro_usd, other.max_cost_micro_usd),
        }
    }

    /// Check whether `candidate` is at least as restrictive as `self`.
    #[must_use]
    pub fn allows_narrowing(&self, candidate: &Self) -> bool {
        within_opt(self.max_steps, candidate.max_steps)
            && within_opt(self.max_tokens, candidate.max_tokens)
            && within_opt(self.max_cost_micro_usd, candidate.max_cost_micro_usd)
    }
}

fn min_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

fn within_opt(parent: Option<u64>, child: Option<u64>) -> bool {
    match (parent, child) {
        (Some(p), Some(c)) => c <= p,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// Token and cost usage recorded for a single model/spawn result or accumulated
/// across a Turn/Session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Input token units consumed.
    pub input_units: u64,
    /// Output token units produced.
    pub output_units: u64,
    /// Cost in integer micro-USD (`1_000_000 = $1.00`).
    pub cost_micro_usd: u64,
}

impl Usage {
    /// Return total token units (`input_units + output_units`) using saturating math.
    #[must_use]
    pub fn total_tokens(&self) -> u64 {
        self.input_units.saturating_add(self.output_units)
    }

    /// Accumulate another [`Usage`] into `self` with saturating addition.
    pub fn add_assign(&mut self, other: &Self) {
        self.input_units = self.input_units.saturating_add(other.input_units);
        self.output_units = self.output_units.saturating_add(other.output_units);
        self.cost_micro_usd = self.cost_micro_usd.saturating_add(other.cost_micro_usd);
    }
}

/// Live budget consumption snapshot exposed to `AgentLoop`s via `LoopContext`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    /// Number of Steps executed so far.
    pub steps_used: u64,
    /// Total token units consumed so far.
    pub tokens_used: u64,
    /// Total cost consumed so far in micro-USD.
    pub cost_micro_usd_used: u64,
    /// Active ceiling governing this Turn/Session.
    pub cap: BudgetCap,
}

impl BudgetSnapshot {
    /// Return remaining Steps if a `max_steps` ceiling is active.
    #[must_use]
    pub fn remaining_steps(&self) -> Option<u64> {
        self.cap
            .max_steps
            .map(|m| m.saturating_sub(self.steps_used))
    }

    /// Return remaining tokens if a `max_tokens` ceiling is active.
    #[must_use]
    pub fn remaining_tokens(&self) -> Option<u64> {
        self.cap
            .max_tokens
            .map(|m| m.saturating_sub(self.tokens_used))
    }

    /// Return remaining micro-USD if a `max_cost_micro_usd` ceiling is active.
    #[must_use]
    pub fn remaining_cost_micro_usd(&self) -> Option<u64> {
        self.cap
            .max_cost_micro_usd
            .map(|m| m.saturating_sub(self.cost_micro_usd_used))
    }

    /// Return `true` if any configured ceiling has been reached or exceeded.
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.cap.max_steps.is_some_and(|m| self.steps_used >= m)
            || self.cap.max_tokens.is_some_and(|m| self.tokens_used >= m)
            || self
                .cap
                .max_cost_micro_usd
                .is_some_and(|m| self.cost_micro_usd_used >= m)
    }

    /// Check whether a child [`BudgetCap`] fits inside the remaining budget.
    #[must_use]
    pub fn can_allocate_child(&self, child: &BudgetCap) -> bool {
        within_opt(self.remaining_steps(), child.max_steps)
            && within_opt(self.remaining_tokens(), child.max_tokens)
            && within_opt(self.remaining_cost_micro_usd(), child.max_cost_micro_usd)
    }

    /// Record one executed Step and optional [`Usage`].
    pub fn record_step(&mut self, usage: Option<&Usage>) {
        self.steps_used = self.steps_used.saturating_add(1);
        if let Some(u) = usage {
            self.record_usage(u);
        }
    }

    /// Accumulate [`Usage`] without incrementing `steps_used`.
    pub fn record_usage(&mut self, usage: &Usage) {
        self.tokens_used = self.tokens_used.saturating_add(usage.total_tokens());
        self.cost_micro_usd_used = self
            .cost_micro_usd_used
            .saturating_add(usage.cost_micro_usd);
    }
}

/// Kernel-enforced Phase bounds plus an opaque user-space `params` bag.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoopConfig {
    /// Optional allowlist of tool names permitted in this Phase.
    pub allowed_tools: Option<Vec<String>>,
    /// Maximum subagent spawn depth permitted in this Phase.
    pub max_spawn_depth: Option<u8>,
    /// Optional tighter [`BudgetCap`] for this Phase.
    pub budget: Option<BudgetCap>,
    /// Opaque user-space configuration (model aliases, thresholds, routes, etc.).
    #[serde(default)]
    pub params: Value,
}

/// Typed request yielded by an `AgentLoop` to the kernel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Effect {
    /// Invoke a registered model provider for any [`ModelKind`].
    Model {
        /// Provider/model target or alias (e.g., `"provider/model"`).
        target: String,
        /// Paradigm of the model being invoked.
        kind: ModelKind,
        /// Paradigm-specific input JSON built by a `ContextBuilder`.
        input: Value,
        /// Optional JSON schema constraining structured output.
        schema: Option<Value>,
    },
    /// Invoke a registered tool inside the bound sandbox.
    Tool {
        /// Registered tool slot name.
        name: String,
        /// Tool argument JSON object.
        args: Value,
        /// Declared side-effect class for policy gating.
        side_effect: SideEffectClass,
    },
    /// Stage or promote a versioned mutation to workspace/harness artifacts.
    Save {
        /// Relative artifact path.
        artifact: String,
        /// Mutation payload.
        patch: ArtifactPatch,
        /// Whether to stage or promote the mutation.
        mode: SaveMode,
    },
    /// Launch an isolated, depth-bounded child subagent branch.
    Spawn {
        /// Registered loop or preset ID for the child subagent.
        loop_id: String,
        /// Initial input/prompt payload for the child branch.
        input: Value,
        /// Budget allocated from the parent's remaining budget.
        budget: BudgetCap,
        /// Maximum allowed spawn depth for the child.
        depth_limit: u8,
        /// Filesystem isolation mode for the child subagent.
        workspace_mode: WorkspaceMode,
        /// Optional child [`LoopConfig`] overrides.
        config: Option<LoopConfig>,
        /// When `true`, run the child subagent in the background without blocking the Step.
        #[serde(default)]
        detached: bool,
        /// Completion notification behavior when `detached == true`.
        #[serde(default)]
        notify_on_complete: SpawnNotifyMode,
    },
    /// Seal the active Phase and transition to a different loop/preset.
    Switch {
        /// Target loop or preset ID.
        next_loop: String,
        /// Typed handoff JSON passed to the next Phase's `LoopContext`.
        handoff: Value,
        /// Optional [`LoopConfig`] overrides for the next Phase.
        config: Option<LoopConfig>,
    },
    /// Suspend execution and yield control to the client/host UI.
    Pause {
        /// Semantic pause reason (`Approval`, `Input`, or `Checkpoint`).
        reason: PauseReason,
        /// Structured dialog/checkpoint payload.
        payload: Value,
    },
}

impl Effect {
    /// Return a short snake_case operation label for logging and diagnostics.
    #[must_use]
    pub fn op_name(&self) -> &'static str {
        match self {
            Self::Model { .. } => "model",
            Self::Tool { .. } => "tool",
            Self::Save { .. } => "save",
            Self::Spawn { .. } => "spawn",
            Self::Switch { .. } => "switch",
            Self::Pause { .. } => "pause",
        }
    }
}

/// An [`Effect`] paired with its deterministic kernel-assigned `effect_id`
/// so parallel batches correlate 1:1 with [`StepResult`]s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectEnvelope {
    /// Deterministic unique ID assigned by the kernel (`ctx.next_id()`).
    pub effect_id: String,
    /// Underlying typed effect payload.
    #[serde(flatten)]
    pub effect: Effect,
}

/// Kernel-recorded outcome of executing (or denying/aborting) an [`EffectEnvelope`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StepResult {
    /// Completed [`Effect::Model`] execution.
    Model {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Model paradigm that executed.
        kind: ModelKind,
        /// Normalized model output JSON.
        output: Value,
        /// Token and micro-USD usage recorded by the provider.
        usage: Usage,
    },
    /// Completed [`Effect::Tool`] execution.
    Tool {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Tool output payload (or truncated preview if spilled to blob).
        output: Value,
        /// `true` if the tool reported an execution or validation error.
        is_error: bool,
        /// Optional SHA-256 blob digest if output exceeded inline threshold.
        blob_digest: Option<String>,
    },
    /// Completed [`Effect::Save`] staging or promotion.
    Saved {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Deterministic stage identifier for `/rollback`.
        stage_id: String,
        /// Human-readable summary of applied changes.
        diff_summary: String,
        /// SHA-256 digest of the saved artifact state.
        digest: String,
    },
    /// Completed [`Effect::Spawn`] child subagent run.
    SpawnCompleted {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Head node ID of the child's branch in `SessionTree`.
        child_head_id: String,
        /// Optional isolated worktree path if `WorkspaceMode::Worktree` was used.
        worktree_path: Option<String>,
        /// Optional SHA-256 digest of the unified diff produced in the worktree.
        diff_blob_digest: Option<String>,
        /// Final output JSON returned by the child subagent.
        output: Value,
        /// Total usage consumed by the child subagent.
        usage: Usage,
    },
    /// Completed [`Effect::Switch`] Phase transition.
    Switched {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Newly opened Phase ID.
        new_phase_id: String,
    },
    /// Completed [`Effect::Pause`] resolved by user or host.
    PauseResolved {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Client resolution payload.
        resolution: Value,
    },
    /// Effect blocked by `PolicyGate` before dispatch.
    Denied {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Human-readable denial reason.
        reason: String,
    },
    /// Effect cancelled mid-step (or synthesized for an unpaired tool call).
    Aborted {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Reason for cancellation.
        reason: String,
    },
    /// Runtime or provider error during effect execution.
    Error {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Error description.
        message: String,
        /// Whether the caller may safely retry this effect.
        retryable: bool,
    },
}

impl StepResult {
    /// Return the correlated `effect_id` for any [`StepResult`] variant.
    #[must_use]
    pub fn effect_id(&self) -> &str {
        match self {
            Self::Model { effect_id, .. }
            | Self::Tool { effect_id, .. }
            | Self::Saved { effect_id, .. }
            | Self::SpawnCompleted { effect_id, .. }
            | Self::Switched { effect_id, .. }
            | Self::PauseResolved { effect_id, .. }
            | Self::Denied { effect_id, .. }
            | Self::Aborted { effect_id, .. }
            | Self::Error { effect_id, .. } => effect_id,
        }
    }

    /// Return the [`Usage`] associated with this result, if any.
    #[must_use]
    pub fn usage(&self) -> Option<&Usage> {
        match self {
            Self::Model { usage, .. } | Self::SpawnCompleted { usage, .. } => Some(usage),
            Self::Tool { .. }
            | Self::Saved { .. }
            | Self::Switched { .. }
            | Self::PauseResolved { .. }
            | Self::Denied { .. }
            | Self::Aborted { .. }
            | Self::Error { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn effect_envelope_and_step_result_round_trip() {
        let env = EffectEnvelope {
            effect_id: "eff_1".to_string(),
            effect: Effect::Model {
                target: "mock/model".to_string(),
                kind: ModelKind::Decision,
                input: json!({"state": 1}),
                schema: None,
            },
        };
        let encoded = serde_json::to_string(&env).expect("serialize envelope");
        let decoded: EffectEnvelope = serde_json::from_str(&encoded).expect("deserialize envelope");
        assert_eq!(env, decoded);

        let res = StepResult::Aborted {
            effect_id: "eff_1".to_string(),
            reason: "user cancel".to_string(),
        };
        let res_json = serde_json::to_string(&res).expect("serialize result");
        let res_dec: StepResult = serde_json::from_str(&res_json).expect("deserialize result");
        assert_eq!(res, res_dec);
        assert_eq!(res_dec.effect_id(), "eff_1");
    }

    #[test]
    fn budget_snapshot_enforces_child_allocation_bounds() {
        let mut snap = BudgetSnapshot {
            steps_used: 2,
            tokens_used: 400,
            cost_micro_usd_used: 1_000,
            cap: BudgetCap {
                max_steps: Some(10),
                max_tokens: Some(1_000),
                max_cost_micro_usd: Some(5_000),
            },
        };
        assert!(snap.can_allocate_child(&BudgetCap {
            max_steps: Some(8),
            max_tokens: Some(600),
            max_cost_micro_usd: Some(4_000),
        }));
        assert!(!snap.can_allocate_child(&BudgetCap {
            max_steps: Some(9),
            max_tokens: Some(500),
            max_cost_micro_usd: Some(1_000),
        }));
        snap.record_step(Some(&Usage {
            input_units: 300,
            output_units: 300,
            cost_micro_usd: 4_000,
        }));
        assert!(snap.is_exhausted());
    }
}
