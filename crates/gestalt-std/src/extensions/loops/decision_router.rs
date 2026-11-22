#![forbid(unsafe_code)]
//! Fast System One [`AgentLoop`] (`"decision_router"`) for `ModelKind::Decision`
//! routing and automatic `Effect::Switch` escalation.

use std::collections::BTreeMap;

use gestalt_kernel::{
    AgentLoop, AgentLoopFactory, Effect, ExtensionRegistry, LoopAction, LoopConfig, LoopContext,
    ModelKind, StepResult, ToolSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::extensions::DecisionOutput;
use crate::extensions::loops::{
    capture_allowed_tools, is_phase_entry, param_str, parse_model_capabilities, tool_side_effect,
};

const DEFAULT_DECISION_MODEL: &str = "default_decision";
const DEFAULT_CONFIDENCE_THRESHOLD: f64 = 0.85;
const DEFAULT_FALLBACK_PRESET: &str = "general";

/// Configured route action inside `LoopConfig::params["routes"][<choice_id>]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RouteAction {
    /// Execute a deterministic tool directly without waking a generative LLM.
    Tool {
        /// Registered tool slot name.
        name: String,
        /// Tool arguments JSON.
        #[serde(default)]
        args: Value,
    },
    /// Switch immediately to another preset/loop.
    Switch {
        /// Target preset or loop ID.
        preset: String,
    },
}

/// Factory registering [`DecisionRouter`] under `"decision_router"`.
#[derive(Debug, Clone, Default)]
pub struct DecisionRouterFactory;

impl AgentLoopFactory for DecisionRouterFactory {
    fn id(&self) -> &str {
        "decision_router"
    }

    fn digest(&self) -> &str {
        "std_decision_router_v1"
    }

    fn create(&self, config: &LoopConfig, registry: &ExtensionRegistry) -> Box<dyn AgentLoop> {
        Box::new(DecisionRouter::new(capture_allowed_tools(config, registry)))
    }
}

/// Fast System One router [`AgentLoop`] (`"decision_router"`).
///
/// Invokes `ModelKind::Decision` via `DecisionStateBuilder`. If confidence meets
/// `confidence_threshold` (default `0.85`) and a route matches, executes the
/// deterministic tool or switches to the target preset; otherwise escalates via
/// `Effect::Switch` to `fallback_preset` (default `"general"`).
#[derive(Debug, Clone)]
pub struct DecisionRouter {
    allowed_tools: BTreeMap<String, ToolSpec>,
    last_choice: Option<(String, f64)>,
}

impl DecisionRouter {
    /// Construct a [`DecisionRouter`] with Phase-captured tool specifications.
    #[must_use]
    pub fn new(allowed_tools: BTreeMap<String, ToolSpec>) -> Self {
        Self {
            allowed_tools,
            last_choice: None,
        }
    }

    fn switch_to_fallback(&self, ctx: &dyn LoopContext, reason: &str, extra: Value) -> LoopAction {
        let fallback = param_str(ctx.config(), "fallback_preset", DEFAULT_FALLBACK_PRESET);
        LoopAction::Yield(vec![Effect::Switch {
            next_loop: fallback.to_string(),
            handoff: json!({
                "escalated_from": "decision_router",
                "reason": reason,
                "details": extra,
                "ts_ms": ctx.now_ms(),
            }),
            config: None,
        }])
    }
}

impl AgentLoop for DecisionRouter {
    fn id(&self) -> &str {
        "decision_router"
    }

    fn digest(&self) -> &str {
        "std_decision_router_v1"
    }

    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        let _steered = ctx.drain_steering();

        if is_phase_entry(results.as_deref()) {
            self.last_choice = None;
            let caps = parse_model_capabilities(ctx.config());
            let input = ctx.build_context(&caps);
            let target = param_str(ctx.config(), "decision_model", DEFAULT_DECISION_MODEL);
            return LoopAction::Yield(vec![Effect::Model {
                target: target.to_string(),
                kind: ModelKind::Decision,
                input,
                schema: None,
            }]);
        }

        let res_list = results.unwrap_or_default();
        match res_list.first() {
            Some(StepResult::Model {
                kind: ModelKind::Decision,
                output,
                ..
            }) => {
                let Ok(dec) = serde_json::from_value::<DecisionOutput>(output.clone()) else {
                    return self.switch_to_fallback(
                        ctx,
                        "malformed_decision_output",
                        output.clone(),
                    );
                };

                let threshold = ctx
                    .config()
                    .params
                    .get("confidence_threshold")
                    .and_then(Value::as_f64)
                    .unwrap_or(DEFAULT_CONFIDENCE_THRESHOLD);

                if dec.confidence < threshold {
                    return self.switch_to_fallback(
                        ctx,
                        "low_confidence",
                        json!({ "choice": dec.choice, "confidence": dec.confidence, "threshold": threshold }),
                    );
                }

                let route_val = ctx
                    .config()
                    .params
                    .get("routes")
                    .and_then(|r| r.get(&dec.choice));
                let Some(route) =
                    route_val.and_then(|v| serde_json::from_value::<RouteAction>(v.clone()).ok())
                else {
                    return self.switch_to_fallback(
                        ctx,
                        "unmapped_choice",
                        json!({ "choice": dec.choice, "confidence": dec.confidence }),
                    );
                };

                self.last_choice = Some((dec.choice.clone(), dec.confidence));
                match route {
                    RouteAction::Tool { name, args } => {
                        let side_effect = tool_side_effect(&self.allowed_tools, &name);
                        LoopAction::Yield(vec![Effect::Tool {
                            name,
                            args,
                            side_effect,
                        }])
                    }
                    RouteAction::Switch { preset } => LoopAction::Yield(vec![Effect::Switch {
                        next_loop: preset,
                        handoff: json!({
                            "routed_from": "decision_router",
                            "choice": dec.choice,
                            "confidence": dec.confidence,
                        }),
                        config: None,
                    }]),
                }
            }
            Some(StepResult::Tool {
                output,
                is_error: false,
                blob_digest,
                ..
            }) => {
                let (choice, confidence) = self.last_choice.clone().unwrap_or_default();
                LoopAction::Done(json!({
                    "routed_choice": choice,
                    "confidence": confidence,
                    "tool_output": output,
                    "blob_digest": blob_digest,
                }))
            }
            Some(
                err_res @ (StepResult::Tool { is_error: true, .. }
                | StepResult::Denied { .. }
                | StepResult::Aborted { .. }
                | StepResult::Error { .. }),
            ) => self.switch_to_fallback(
                ctx,
                "fast_path_or_decision_failed",
                serde_json::to_value(err_res).unwrap_or(Value::Null),
            ),
            _ => self.switch_to_fallback(ctx, "unexpected_step_result", Value::Null),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::{ChatLoopFactory, DecisionStateBuilder, StandardBuilder};
    use gestalt_kernel::{
        ArtifactStoreView, BudgetCap, DriverDirective, DriverResponse, KernelRuntime, PolicyGate,
        Preset, SandboxPolicy, SideEffectClass, Usage,
    };

    fn setup_triage_kernel() -> KernelRuntime {
        let mut reg = ExtensionRegistry::new();
        reg.builders.insert(
            "decision_state".into(),
            std::sync::Arc::new(DecisionStateBuilder),
        );
        reg.builders.insert(
            "standard".into(),
            std::sync::Arc::new(StandardBuilder::default()),
        );
        reg.loops.insert(
            "decision_router".into(),
            std::sync::Arc::new(DecisionRouterFactory),
        );
        reg.loops
            .insert("chat".into(), std::sync::Arc::new(ChatLoopFactory));
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
            "triage".into(),
            Preset {
                base_loop: "decision_router".into(),
                decorators: vec![],
                builder: "decision_state".into(),
                builder_layers: vec![],
                config: LoopConfig {
                    allowed_tools: Some(vec!["bash".into()]),
                    params: json!({
                        "confidence_threshold": 0.85,
                        "fallback_preset": "general",
                        "routes": {
                            "run_tests": { "action": "tool", "name": "bash", "args": { "command": "cargo test" } }
                        }
                    }),
                    ..LoopConfig::default()
                },
            },
        );
        reg.presets.insert(
            "general".into(),
            Preset {
                base_loop: "chat".into(),
                decorators: vec![],
                builder: "standard".into(),
                builder_layers: vec![],
                config: LoopConfig::default(),
            },
        );

        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        KernelRuntime::new("r_triage".into(), "/repo".into(), 10, reg, gate).expect("init")
    }

    #[test]
    fn decision_router_fast_path_executes_tool_without_llm() {
        let mut rt = setup_triage_kernel();
        rt.start_turn(
            json!("run tests"),
            "triage",
            &[],
            ArtifactStoreView::default(),
            11,
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
                output: json!({ "choice": "run_tests", "confidence": 0.94, "latency_ms": 42 }),
                usage: Usage::default(),
            }]))
            .expect("d2");
        let eff_tool = match d2 {
            DriverDirective::DispatchEffects { effects } => {
                assert!(matches!(
                    &effects[0].effect,
                    Effect::Tool { name, .. } if name == "bash"
                ));
                effects[0].effect_id.clone()
            }
            other => panic!("expected tool dispatch, got {other:?}"),
        };
        let d3 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Tool {
                effect_id: eff_tool,
                output: json!({ "exit_code": 0 }),
                is_error: false,
                blob_digest: None,
            }]))
            .expect("d3");
        match d3 {
            DriverDirective::TurnComplete { output, .. } => {
                assert_eq!(output["routed_choice"], "run_tests");
            }
            other => panic!("expected TurnComplete, got {other:?}"),
        }
    }

    #[test]
    fn decision_router_escalates_to_swe_on_low_confidence() {
        let mut rt = setup_triage_kernel();
        rt.start_turn(
            json!("refactor kernel"),
            "triage",
            &[],
            ArtifactStoreView::default(),
            11,
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
                output: json!({ "choice": "run_tests", "confidence": 0.40, "latency_ms": 38 }),
                usage: Usage::default(),
            }]))
            .expect("d2");
        match d2 {
            DriverDirective::DispatchEffects { effects } => {
                assert!(matches!(
                    &effects[0].effect,
                    Effect::Model {
                        kind: ModelKind::Llm,
                        ..
                    }
                ));
            }
            other => panic!("expected escalation to ChatLoop LLM, got {other:?}"),
        }
    }
}
