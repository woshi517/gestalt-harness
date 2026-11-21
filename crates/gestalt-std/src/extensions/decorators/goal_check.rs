#![forbid(unsafe_code)]
//! Composable [`LoopDecorator`] (`"goal_check"`) enforcing deterministic or
//! evaluator-based exit criteria before allowing an inner loop to finish.

use gestalt_kernel::{
    AgentLoop, Effect, ExtensionRegistry, LoopAction, LoopConfig, LoopContext, LoopDecorator,
    ModelKind, SideEffectClass, StepResult,
};
use serde_json::{Value, json};

use crate::extensions::loops::param_str;
use crate::extensions::{
    ChatMessage, ChatRole, ContentBlock, DecisionChoice, DecisionInput, DecisionOutput, LlmInput,
    LlmOutput, PromptTier,
};

const DEFAULT_MAX_GOAL_RETRIES: u32 = 3;
const DEFAULT_GOAL_CONFIDENCE_THRESHOLD: f64 = 0.85;

/// Composable [`LoopDecorator`] (`"goal_check"`).
#[derive(Debug, Clone, Default)]
pub struct GoalCheckDecorator;

impl LoopDecorator for GoalCheckDecorator {
    fn id(&self) -> &str {
        "goal_check"
    }

    fn digest(&self) -> &str {
        "std_goal_check_v1"
    }

    fn wrap(
        &self,
        inner: Box<dyn AgentLoop>,
        _config: &LoopConfig,
        _registry: &ExtensionRegistry,
    ) -> Box<dyn AgentLoop> {
        Box::new(GoalCheckedLoop {
            inner,
            state: GoalState::RunningInner,
            attempts: 0,
            pending_output: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoalState {
    RunningInner,
    CheckingGoal,
}

struct GoalCheckedLoop {
    inner: Box<dyn AgentLoop>,
    state: GoalState,
    attempts: u32,
    pending_output: Option<Value>,
}

impl GoalCheckedLoop {
    fn build_check_effect(&self, ctx: &dyn LoopContext, candidate: &Value) -> Option<Effect> {
        let params = &ctx.config().params;
        if let Some(cmd) = params
            .get("goal_check_command")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return Some(Effect::Tool {
                name: "bash".to_string(),
                args: json!({ "command": cmd }),
                side_effect: SideEffectClass::Read,
            });
        }

        let criteria = params
            .get("goal_criteria")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())?;
        let eval_kind = param_str(ctx.config(), "goal_evaluator_kind", "decision");
        let default_target = if eval_kind == "llm" {
            param_str(ctx.config(), "model", "default_llm")
        } else {
            param_str(ctx.config(), "decision_model", "default_decision")
        };
        let target = param_str(ctx.config(), "goal_evaluator_model", default_target).to_string();

        if eval_kind == "llm" {
            let input = LlmInput {
                system_tiers: vec![PromptTier {
                    name: "stable".into(),
                    cacheable: false,
                    content: "Evaluate whether the candidate output satisfies the goal criteria. Respond with JSON {\"passed\": true|false, \"feedback\": \"...\"}.".into(),
                }],
                messages: vec![ChatMessage {
                    role: ChatRole::User,
                    content: vec![ContentBlock::Text {
                        text: format!("Criteria: {criteria}\nCandidate output: {candidate}"),
                    }],
                }],
                ..LlmInput::default()
            };
            Some(Effect::Model {
                target,
                kind: ModelKind::Llm,
                input: serde_json::to_value(input).unwrap_or(Value::Null),
                schema: None,
            })
        } else {
            let input = DecisionInput {
                state: json!({
                    "criteria": criteria,
                    "candidate_output": candidate,
                    "attempt": self.attempts + 1,
                }),
                choices: vec![
                    DecisionChoice {
                        id: "pass".into(),
                        description: "All goal criteria are satisfied".into(),
                    },
                    DecisionChoice {
                        id: "fail".into(),
                        description: "Goal criteria are not yet satisfied".into(),
                    },
                ],
                rubric: Some(criteria.to_string()),
            };
            Some(Effect::Model {
                target,
                kind: ModelKind::Decision,
                input: serde_json::to_value(input).unwrap_or(Value::Null),
                schema: None,
            })
        }
    }

    fn evaluate_check_result(ctx: &dyn LoopContext, results: &[StepResult]) -> Result<(), String> {
        let Some(first) = results.first() else {
            return Err("goal check produced no result".to_string());
        };
        match first {
            StepResult::Tool {
                output, is_error, ..
            } => {
                let exit_code = output.get("exit_code").and_then(Value::as_i64);
                if !*is_error && exit_code.unwrap_or(0) == 0 {
                    Ok(())
                } else {
                    Err(format!("goal check command failed: {output}"))
                }
            }
            StepResult::Model {
                kind: ModelKind::Decision,
                output,
                ..
            } => {
                let threshold = ctx
                    .config()
                    .params
                    .get("goal_confidence_threshold")
                    .and_then(Value::as_f64)
                    .unwrap_or(DEFAULT_GOAL_CONFIDENCE_THRESHOLD);
                if let Ok(dec) = serde_json::from_value::<DecisionOutput>(output.clone())
                    && dec.choice == "pass"
                    && dec.confidence >= threshold
                {
                    Ok(())
                } else {
                    Err(format!("decision evaluator rejected candidate: {output}"))
                }
            }
            StepResult::Model {
                kind: ModelKind::Llm,
                output,
                ..
            } => {
                if output.get("passed").and_then(Value::as_bool) == Some(true) {
                    return Ok(());
                }
                if let Ok(parsed) = serde_json::from_value::<LlmOutput>(output.clone())
                    && let Ok(inner_json) = serde_json::from_str::<Value>(&parsed.text)
                    && inner_json.get("passed").and_then(Value::as_bool) == Some(true)
                {
                    return Ok(());
                }
                Err(format!("llm evaluator rejected candidate: {output}"))
            }
            StepResult::Denied { reason, .. } => Err(format!("goal check denied: {reason}")),
            StepResult::Aborted { reason, .. } => Err(format!("goal check aborted: {reason}")),
            StepResult::Error { message, .. } => Err(format!("goal check error: {message}")),
            other => Err(format!("unexpected goal check result: {other:?}")),
        }
    }
}

impl AgentLoop for GoalCheckedLoop {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn digest(&self) -> &str {
        self.inner.digest()
    }

    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        let max_retries = ctx
            .config()
            .params
            .get("max_goal_retries")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_MAX_GOAL_RETRIES, |n| n as u32);

        if self.state == GoalState::CheckingGoal {
            let res_list = results.unwrap_or_default();
            match Self::evaluate_check_result(ctx, &res_list) {
                Ok(()) => {
                    let candidate = self.pending_output.take().unwrap_or(Value::Null);
                    self.state = GoalState::RunningInner;
                    return LoopAction::Done(json!({
                        "goal_verified": true,
                        "attempts": self.attempts + 1,
                        "output": candidate,
                    }));
                }
                Err(feedback) => {
                    self.attempts += 1;
                    self.state = GoalState::RunningInner;
                    if self.attempts >= max_retries || ctx.budget_remaining().is_exhausted() {
                        let candidate = self.pending_output.take().unwrap_or(Value::Null);
                        return LoopAction::Done(json!({
                            "goal_verified": false,
                            "attempts": self.attempts,
                            "reason": feedback,
                            "output": candidate,
                        }));
                    }
                    ctx.emit_custom_node(
                        "std:goal_feedback",
                        json!({
                            "attempt": self.attempts,
                            "feedback": feedback,
                        }),
                    );
                    return self.inner.step(ctx, None);
                }
            }
        }

        match self.inner.step(ctx, results) {
            LoopAction::Yield(effs) => LoopAction::Yield(effs),
            LoopAction::Done(candidate) => {
                let Some(check_eff) = self.build_check_effect(ctx, &candidate) else {
                    return LoopAction::Done(candidate);
                };
                if self.attempts >= max_retries || ctx.budget_remaining().is_exhausted() {
                    return LoopAction::Done(json!({
                        "goal_verified": false,
                        "attempts": self.attempts,
                        "output": candidate,
                    }));
                }
                self.pending_output = Some(candidate);
                self.state = GoalState::CheckingGoal;
                LoopAction::Yield(vec![check_eff])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{ChatLoopFactory, StandardBuilder};
    use gestalt_kernel::{
        ArtifactStoreView, BudgetCap, DriverDirective, DriverResponse, KernelRuntime, PolicyGate,
        Preset, ReplayEngine, ReplayMode, SandboxPolicy, ToolSpec, Usage,
    };

    #[test]
    fn goal_check_retries_until_command_passes_and_replays() {
        let mut reg = ExtensionRegistry::new();
        reg.builders
            .insert("standard".into(), Box::new(StandardBuilder::default()));
        reg.loops.insert("chat".into(), Box::new(ChatLoopFactory));
        reg.decorators
            .insert("goal_check".into(), Box::new(GoalCheckDecorator));
        reg.tools.insert(
            "bash".into(),
            ToolSpec {
                name: "bash".into(),
                description: "Run bash".into(),
                parameters_schema: json!({"type": "object"}),
                default_side_effect: SideEffectClass::Irreversible,
                source_extension: None,
            },
        );
        reg.presets.insert(
            "swe_goal".into(),
            Preset {
                base_loop: "chat".into(),
                decorators: vec!["goal_check".into()],
                builder: "standard".into(),
                config: LoopConfig {
                    allowed_tools: Some(vec!["bash".into()]),
                    params: json!({
                        "goal_criteria": "cargo test passes",
                        "goal_check_command": "cargo test",
                        "max_goal_retries": 3
                    }),
                    ..LoopConfig::default()
                },
            },
        );

        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let mut rt =
            KernelRuntime::new("r_goal".into(), "/repo".into(), 10, reg, gate).expect("init");
        rt.start_turn(
            json!("fix tests"),
            "swe_goal",
            &[],
            ArtifactStoreView::default(),
            11,
        )
        .expect("start");

        // Attempt 1: inner ChatLoop finishes -> goal_check yields bash
        let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
        let eff_m1 = match d1 {
            DriverDirective::DispatchEffects { effects } => effects[0].effect_id.clone(),
            other => panic!("expected model1, got {other:?}"),
        };
        let d2 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                effect_id: eff_m1,
                kind: ModelKind::Llm,
                output: json!({ "text": "I fixed it" }),
                usage: Usage::default(),
            }]))
            .expect("d2");
        let eff_g1 = match d2 {
            DriverDirective::DispatchEffects { effects } => {
                assert!(matches!(
                    &effects[0].effect,
                    Effect::Tool { name, .. } if name == "bash"
                ));
                effects[0].effect_id.clone()
            }
            other => panic!("expected goal check tool, got {other:?}"),
        };

        // Goal check fails -> feedback injected -> inner ChatLoop retries
        let d3 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Tool {
                effect_id: eff_g1,
                output: json!({ "exit_code": 1, "stderr": "1 test failed" }),
                is_error: true,
                blob_digest: None,
            }]))
            .expect("d3");
        let eff_m2 = match d3 {
            DriverDirective::DispatchEffects { effects } => {
                if let Effect::Model { input, .. } = &effects[0].effect {
                    assert!(input.to_string().contains("1 test failed"));
                } else {
                    panic!("expected model2");
                }
                effects[0].effect_id.clone()
            }
            other => panic!("expected model2, got {other:?}"),
        };

        // Inner finishes again -> goal check passes
        let d4 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                effect_id: eff_m2,
                kind: ModelKind::Llm,
                output: json!({ "text": "Fixed the failing test" }),
                usage: Usage::default(),
            }]))
            .expect("d4");
        let eff_g2 = match d4 {
            DriverDirective::DispatchEffects { effects } => effects[0].effect_id.clone(),
            other => panic!("expected goal check 2, got {other:?}"),
        };
        let d5 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Tool {
                effect_id: eff_g2,
                output: json!({ "exit_code": 0 }),
                is_error: false,
                blob_digest: None,
            }]))
            .expect("d5");
        match d5 {
            DriverDirective::TurnComplete { output, .. } => {
                assert_eq!(output["goal_verified"], true);
                assert_eq!(output["attempts"], 2);
            }
            other => panic!("expected TurnComplete, got {other:?}"),
        }

        let report = ReplayEngine::new(rt.registry(), rt.policy_gate())
            .replay(rt.session_tree(), ReplayMode::Default, |_| None)
            .expect("replay");
        assert!(report.is_parity(), "divergences: {:?}", report.divergences);
    }
}
