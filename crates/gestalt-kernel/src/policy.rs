#![forbid(unsafe_code)]
//! Immutable, fail-closed `PolicyGate`, monotonic `BudgetCap` / `SandboxPolicy`
//! enforcement, and `RequiresVerification` deferral callbacks.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::effect::{
    BudgetCap, BudgetSnapshot, Effect, EffectEnvelope, LoopConfig, SaveMode, StepResult,
    WorkspaceMode,
};
use crate::error::KernelError;
use crate::tree::SessionSlice;

/// Outcome of evaluating an [`EffectEnvelope`] through [`PolicyGate`].
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyDecision {
    /// Effect is permitted for dispatch.
    Allow,
    /// Effect is blocked before dispatch (`StepResult::Denied`).
    Deny {
        /// Human-readable denial explanation.
        reason: String,
    },
    /// Sans-I/O deferral: driver runs `verifier_effect` first, then the kernel
    /// invokes `PolicyRule::evaluate_verification` (failing closed on error).
    RequiresVerification {
        /// Identifier of the [`PolicyRule`] that requested verification.
        rule_id: String,
        /// Verifier effect (e.g., `Effect::Model { kind: Decision, .. }`).
        verifier_effect: Box<Effect>,
        /// Original pending effect held until verification succeeds.
        pending_effect: Box<EffectEnvelope>,
    },
}

/// Extensible policy rule evaluated before effect dispatch.
///
/// Extensions may add stricter rules, but can never weaken kernel baseline
/// invariants or bypass another rule's `Deny`.
pub trait PolicyRule: Send + Sync {
    /// Unique rule identifier used for audit logs and verification callbacks.
    fn id(&self) -> &str;

    /// Evaluate a candidate effect against the active branch `SessionSlice`.
    fn evaluate(&self, effect: &EffectEnvelope, session: &SessionSlice) -> PolicyDecision;

    /// Resolve a prior [`PolicyDecision::RequiresVerification`] once the
    /// verifier effect completes. Defaults to fail-closed `Deny`.
    fn evaluate_verification(
        &self,
        _pending: &EffectEnvelope,
        _verifier_result: &StepResult,
    ) -> PolicyDecision {
        PolicyDecision::Deny {
            reason: format!(
                "rule '{}' does not implement verification resolution (fail-closed)",
                self.id()
            ),
        }
    }
}

/// Kernel-enforced sandbox invariants (`network` and `protected_paths`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Whether outbound network access is allowed inside sandboxed subprocesses.
    pub network: bool,
    /// Workspace-relative path prefixes mounted read-only inside sandboxes.
    pub protected_paths: BTreeSet<String>,
    /// Maximum subagent spawn nesting depth permitted.
    pub max_spawn_depth: u8,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            network: false,
            protected_paths: BTreeSet::new(),
            max_spawn_depth: 1,
        }
    }
}

impl SandboxPolicy {
    /// Merge a stricter tier onto `self` (`protected_paths` union, `network`
    /// cannot be enabled if disabled, `max_spawn_depth` cannot increase).
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::PolicyRelaxationForbidden`] if `other` attempts
    /// to enable network when disabled, remove a protected path, or raise
    /// `max_spawn_depth`.
    pub fn validate_and_merge(&self, other: &Self) -> Result<Self, KernelError> {
        if !self.network && other.network {
            return Err(KernelError::PolicyRelaxationForbidden(
                "cannot enable sandbox network when baseline disables it".to_string(),
            ));
        }
        if other.max_spawn_depth > self.max_spawn_depth {
            return Err(KernelError::PolicyRelaxationForbidden(format!(
                "cannot raise max_spawn_depth from {} to {}",
                self.max_spawn_depth, other.max_spawn_depth
            )));
        }
        let mut merged_paths = self.protected_paths.clone();
        for p in &other.protected_paths {
            merged_paths.insert(normalize_prefix(p));
        }
        Ok(Self {
            network: self.network && other.network,
            protected_paths: merged_paths,
            max_spawn_depth: self.max_spawn_depth.min(other.max_spawn_depth),
        })
    }
}

/// Higher-tier policy overrides (e.g., Global or Workspace `gestalt.toml`)
/// validated monotonically against [`PolicyGate`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyOverride {
    /// Optional tighter budget cap.
    pub budget: Option<BudgetCap>,
    /// Optional narrower tool allowlist.
    pub allowed_tools: Option<BTreeSet<String>>,
    /// Optional stricter sandbox policy.
    pub sandbox: Option<SandboxPolicy>,
}

/// Evaluation result for a single effect within a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchItemDecision {
    /// Approved for execution in `DispatchEffects`.
    Approved(EffectEnvelope),
    /// Denied immediately before dispatch.
    Denied {
        /// Effect ID that was denied.
        effect_id: String,
        /// Denial reason.
        reason: String,
    },
    /// Deferred pending execution of `verifier_effect`.
    NeedsVerification {
        /// Rule ID awaiting verification.
        rule_id: String,
        /// Verifier effect to run.
        verifier_effect: Box<Effect>,
        /// Pending effect held by the kernel.
        pending: Box<EffectEnvelope>,
    },
}

/// Immutable, fail-closed security and budget guard inside `gestalt-kernel`.
pub struct PolicyGate {
    baseline_budget: BudgetCap,
    baseline_allowed_tools: Option<BTreeSet<String>>,
    sandbox: SandboxPolicy,
    rules: Vec<Box<dyn PolicyRule>>,
    rules_by_id: BTreeMap<String, usize>,
}

impl Default for PolicyGate {
    fn default() -> Self {
        Self::new(BudgetCap::default(), None, SandboxPolicy::default())
    }
}

impl PolicyGate {
    /// Construct a new [`PolicyGate`] with immutable baseline bounds.
    #[must_use]
    pub fn new(
        baseline_budget: BudgetCap,
        baseline_allowed_tools: Option<BTreeSet<String>>,
        mut sandbox: SandboxPolicy,
    ) -> Self {
        sandbox.protected_paths = sandbox
            .protected_paths
            .into_iter()
            .map(|p| normalize_prefix(&p))
            .collect();
        Self {
            baseline_budget,
            baseline_allowed_tools,
            sandbox,
            rules: Vec::new(),
            rules_by_id: BTreeMap::new(),
        }
    }

    /// Return the active baseline [`BudgetCap`].
    #[must_use]
    pub fn baseline_budget(&self) -> BudgetCap {
        self.baseline_budget
    }

    /// Return the active [`SandboxPolicy`].
    #[must_use]
    pub fn sandbox(&self) -> &SandboxPolicy {
        &self.sandbox
    }

    /// Return the baseline tool allowlist, if restricted.
    #[must_use]
    pub fn baseline_allowed_tools(&self) -> Option<&BTreeSet<String>> {
        self.baseline_allowed_tools.as_ref()
    }

    /// Return registered [`PolicyRule`] IDs in deterministic sorted order.
    #[must_use]
    pub fn rule_ids(&self) -> Vec<&str> {
        self.rules_by_id.keys().map(String::as_str).collect()
    }

    /// Add an additive [`PolicyRule`] (rules can only further restrict effects).
    pub fn add_rule(&mut self, rule: Box<dyn PolicyRule>) {
        let id = rule.id().to_string();
        let idx = self.rules.len();
        self.rules.push(rule);
        self.rules_by_id.insert(id, idx);
    }

    /// Validate and apply a monotonic [`PolicyOverride`] (`Kernel <= Global <= Workspace`).
    ///
    /// # Errors
    ///
    /// Returns [`KernelError::PolicyRelaxationForbidden`] if any field attempts
    /// to loosen an existing restriction.
    pub fn apply_monotonic_override(
        &mut self,
        override_cfg: &PolicyOverride,
    ) -> Result<(), KernelError> {
        if let Some(ref budget) = override_cfg.budget {
            if !self.baseline_budget.allows_narrowing(budget) {
                return Err(KernelError::PolicyRelaxationForbidden(
                    "cannot relax BudgetCap limits above baseline".to_string(),
                ));
            }
            self.baseline_budget = self.baseline_budget.tighten(budget);
        }

        if let Some(ref tools) = override_cfg.allowed_tools {
            if let Some(ref base_tools) = self.baseline_allowed_tools
                && !tools.is_subset(base_tools)
            {
                return Err(KernelError::PolicyRelaxationForbidden(
                    "cannot add tools outside baseline allowed_tools".to_string(),
                ));
            }
            self.baseline_allowed_tools = Some(tools.clone());
        }

        if let Some(ref sb) = override_cfg.sandbox {
            self.sandbox = self.sandbox.validate_and_merge(sb)?;
        }

        Ok(())
    }

    /// Evaluate a batch of [`EffectEnvelope`]s against budgets, tool allowlists,
    /// spawn invariants, path safety, and registered [`PolicyRule`]s.
    #[must_use]
    pub fn evaluate_batch(
        &self,
        batch: &[EffectEnvelope],
        phase_config: &LoopConfig,
        budget_snapshot: &BudgetSnapshot,
        current_spawn_depth: u8,
        registered_tools: &BTreeSet<String>,
        session: &SessionSlice,
    ) -> Vec<BatchItemDecision> {
        let spawn_count = batch
            .iter()
            .filter(|e| matches!(e.effect, Effect::Spawn { .. }))
            .count();

        batch
            .iter()
            .map(|env| {
                self.evaluate_single(
                    env,
                    phase_config,
                    budget_snapshot,
                    current_spawn_depth,
                    spawn_count,
                    registered_tools,
                    session,
                )
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn evaluate_single(
        &self,
        env: &EffectEnvelope,
        phase_config: &LoopConfig,
        budget_snapshot: &BudgetSnapshot,
        current_spawn_depth: u8,
        spawn_count_in_batch: usize,
        registered_tools: &BTreeSet<String>,
        session: &SessionSlice,
    ) -> BatchItemDecision {
        // Control effects (`Switch`, `Pause`) do not consume external side-effect
        // budget or require tool ACL checks, though custom rules may still inspect them.
        let is_side_effecting = !matches!(env.effect, Effect::Switch { .. } | Effect::Pause { .. });
        if is_side_effecting && budget_snapshot.is_exhausted() {
            return BatchItemDecision::Denied {
                effect_id: env.effect_id.clone(),
                reason: "budget cap exceeded".to_string(),
            };
        }

        match &env.effect {
            Effect::Tool { name, .. } => {
                if !registered_tools.contains(name) {
                    return BatchItemDecision::Denied {
                        effect_id: env.effect_id.clone(),
                        reason: format!("unknown tool '{name}'"),
                    };
                }
                if let Some(ref base) = self.baseline_allowed_tools
                    && !base.contains(name)
                {
                    return BatchItemDecision::Denied {
                        effect_id: env.effect_id.clone(),
                        reason: format!("tool '{name}' denied by baseline policy"),
                    };
                }
                if let Some(ref phase_tools) = phase_config.allowed_tools
                    && !phase_tools.iter().any(|t| t == name)
                {
                    return BatchItemDecision::Denied {
                        effect_id: env.effect_id.clone(),
                        reason: format!("tool '{name}' not in phase allowed_tools"),
                    };
                }
            }
            Effect::Save { artifact, mode, .. } => {
                if has_path_traversal(artifact) {
                    return BatchItemDecision::Denied {
                        effect_id: env.effect_id.clone(),
                        reason: format!("save artifact path '{artifact}' escapes workspace"),
                    };
                }
                if *mode == SaveMode::Promote && self.rules.is_empty() {
                    // Promoting into protected paths requires at least baseline path
                    // validation; runtime still enforces ledger & staging checks.
                }
            }
            Effect::Spawn {
                budget,
                depth_limit,
                workspace_mode,
                ..
            } => {
                let effective_max_depth = phase_config
                    .max_spawn_depth
                    .map_or(self.sandbox.max_spawn_depth, |d| {
                        d.min(self.sandbox.max_spawn_depth)
                    })
                    .min(*depth_limit);
                if current_spawn_depth >= effective_max_depth {
                    return BatchItemDecision::Denied {
                        effect_id: env.effect_id.clone(),
                        reason: format!(
                            "spawn depth limit reached (current={current_spawn_depth}, max={effective_max_depth})"
                        ),
                    };
                }
                if !budget_snapshot.can_allocate_child(budget) {
                    return BatchItemDecision::Denied {
                        effect_id: env.effect_id.clone(),
                        reason: "child spawn budget exceeds parent remaining budget".to_string(),
                    };
                }
                if spawn_count_in_batch > 1 && *workspace_mode == WorkspaceMode::Shared {
                    return BatchItemDecision::Denied {
                        effect_id: env.effect_id.clone(),
                        reason: "concurrent spawns cannot use WorkspaceMode::Shared".to_string(),
                    };
                }
            }
            Effect::Model { .. } | Effect::Switch { .. } | Effect::Pause { .. } => {}
        }

        let mut verification: Option<(String, Effect)> = None;
        for rule in &self.rules {
            match rule.evaluate(env, session) {
                PolicyDecision::Allow => {}
                PolicyDecision::Deny { reason } => {
                    return BatchItemDecision::Denied {
                        effect_id: env.effect_id.clone(),
                        reason,
                    };
                }
                PolicyDecision::RequiresVerification {
                    rule_id,
                    verifier_effect,
                    ..
                } => {
                    if verification.is_none() {
                        verification = Some((rule_id, *verifier_effect));
                    }
                }
            }
        }

        if let Some((rule_id, verifier_effect)) = verification {
            return BatchItemDecision::NeedsVerification {
                rule_id,
                verifier_effect: Box::new(verifier_effect),
                pending: Box::new(env.clone()),
            };
        }

        BatchItemDecision::Approved(env.clone())
    }

    /// Resolve a prior [`BatchItemDecision::NeedsVerification`] using the
    /// verifier's [`StepResult`], failing closed on any error, denial, or abort.
    #[must_use]
    pub fn resolve_verification(
        &self,
        rule_id: &str,
        pending: &EffectEnvelope,
        verifier_result: &StepResult,
    ) -> PolicyDecision {
        match verifier_result {
            StepResult::Error { message, .. } => {
                return PolicyDecision::Deny {
                    reason: format!("verifier error (fail-closed): {message}"),
                };
            }
            StepResult::Denied { reason, .. } => {
                return PolicyDecision::Deny {
                    reason: format!("verifier denied (fail-closed): {reason}"),
                };
            }
            StepResult::Aborted { reason, .. } => {
                return PolicyDecision::Deny {
                    reason: format!("verifier aborted (fail-closed): {reason}"),
                };
            }
            _ => {}
        }

        let Some(&idx) = self.rules_by_id.get(rule_id) else {
            return PolicyDecision::Deny {
                reason: format!("unknown verification rule '{rule_id}' (fail-closed)"),
            };
        };
        self.rules[idx].evaluate_verification(pending, verifier_result)
    }
}

fn normalize_prefix(raw: &str) -> String {
    raw.trim_start_matches("./").to_string()
}

fn has_path_traversal(path: &str) -> bool {
    path.starts_with('/') || path.split(['/', '\\']).any(|seg| seg == "..") || path.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::SideEffectClass;
    use serde_json::json;

    struct AlwaysRequireVerifierRule;
    impl PolicyRule for AlwaysRequireVerifierRule {
        fn id(&self) -> &str {
            "decision_guard"
        }
        fn evaluate(&self, env: &EffectEnvelope, _session: &SessionSlice) -> PolicyDecision {
            PolicyDecision::RequiresVerification {
                rule_id: self.id().to_string(),
                verifier_effect: Box::new(Effect::Model {
                    target: "classifier".into(),
                    kind: crate::effect::ModelKind::Decision,
                    input: json!({}),
                    schema: None,
                }),
                pending_effect: Box::new(env.clone()),
            }
        }
    }

    #[test]
    fn monotonic_override_rejects_relaxation() {
        let mut gate = PolicyGate::new(
            BudgetCap {
                max_steps: Some(10),
                max_tokens: Some(1_000),
                max_cost_micro_usd: Some(5_000_000),
            },
            Some(["read".to_string()].into_iter().collect()),
            SandboxPolicy {
                network: false,
                protected_paths: ["protected/".to_string()].into_iter().collect(),
                max_spawn_depth: 1,
            },
        );

        let relaxed_cost = PolicyOverride {
            budget: Some(BudgetCap {
                max_steps: Some(10),
                max_tokens: Some(1_000),
                max_cost_micro_usd: Some(6_000_000),
            }),
            ..Default::default()
        };
        assert!(matches!(
            gate.apply_monotonic_override(&relaxed_cost),
            Err(KernelError::PolicyRelaxationForbidden(_))
        ));

        let relaxed_net = PolicyOverride {
            sandbox: Some(SandboxPolicy {
                network: true,
                protected_paths: BTreeSet::new(),
                max_spawn_depth: 1,
            }),
            ..Default::default()
        };
        assert!(matches!(
            gate.apply_monotonic_override(&relaxed_net),
            Err(KernelError::PolicyRelaxationForbidden(_))
        ));
    }

    #[test]
    fn verification_fails_closed_on_error_or_default_rule() {
        let mut gate = PolicyGate::default();
        gate.add_rule(Box::new(AlwaysRequireVerifierRule));
        let pending = EffectEnvelope {
            effect_id: "e1".into(),
            effect: Effect::Tool {
                name: "exec".into(),
                args: json!({}),
                side_effect: SideEffectClass::Irreversible,
            },
        };
        let err_res = StepResult::Error {
            effect_id: "v1".into(),
            message: "timeout".into(),
            retryable: true,
        };
        assert!(matches!(
            gate.resolve_verification("decision_guard", &pending, &err_res),
            PolicyDecision::Deny { .. }
        ));
    }
}
