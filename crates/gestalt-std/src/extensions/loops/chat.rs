#![forbid(unsafe_code)]
//! Standard multi-turn generative [`AgentLoop`] (`"chat"`).

use std::collections::BTreeMap;

use gestalt_kernel::{
    AgentLoop, AgentLoopFactory, Effect, ExtensionRegistry, LoopAction, LoopConfig, LoopContext,
    ModelKind, StepResult, ToolSpec,
};
use serde_json::{Value, json};

use crate::extensions::loops::{
    capture_allowed_tools, is_phase_entry, param_str, parse_model_capabilities, tool_side_effect,
};
use crate::extensions::{LlmInput, LlmOutput, LlmToolDefinition};

const DEFAULT_CHAT_MODEL: &str = "default_llm";
const DEFAULT_MAX_RETRIES: u32 = 2;

/// Factory registering [`ChatLoop`] under `"chat"`.
#[derive(Debug, Clone, Default)]
pub struct ChatLoopFactory;

impl AgentLoopFactory for ChatLoopFactory {
    fn id(&self) -> &str {
        "chat"
    }

    fn digest(&self) -> &str {
        "std_chat_loop_v1"
    }

    fn create(&self, config: &LoopConfig, registry: &ExtensionRegistry) -> Box<dyn AgentLoop> {
        Box::new(ChatLoop::new(capture_allowed_tools(config, registry)))
    }
}

/// Standard multi-turn generative chat + tool-calling [`AgentLoop`] (`"chat"`).
///
/// Dogfoods `LoopContext` with zero private kernel backdoors:
/// - Builds context via `ctx.build_context(&caps)`
/// - Yields `Effect::Model { kind: ModelKind::Llm, .. }`
/// - Yields parallel `Effect::Tool` batches
/// - Drains mid-run `SteeringSignal`s between Steps via `ctx.drain_steering()`
/// - Relies on `PolicyGate` + `validate_tool_args` to turn unknown/malformed tool
///   calls into recoverable `StepResult::Denied` / `StepResult::Tool { is_error: true }`
///   for next-Step LLM self-correction.
#[derive(Debug, Clone)]
pub struct ChatLoop {
    allowed_tools: BTreeMap<String, ToolSpec>,
    retries_used: u32,
}

impl ChatLoop {
    /// Construct a [`ChatLoop`] with Phase-captured tool specifications.
    #[must_use]
    pub fn new(allowed_tools: BTreeMap<String, ToolSpec>) -> Self {
        Self {
            allowed_tools,
            retries_used: 0,
        }
    }

    fn build_model_effect(&self, ctx: &dyn LoopContext) -> Effect {
        let caps = parse_model_capabilities(ctx.config());
        let mut input = ctx.build_context(&caps);
        if let Ok(mut parsed) = serde_json::from_value::<LlmInput>(input.clone())
            && parsed.tools.is_empty()
            && !self.allowed_tools.is_empty()
        {
            parsed.tools = self
                .allowed_tools
                .values()
                .map(|spec| LlmToolDefinition {
                    name: spec.name.clone(),
                    description: spec.description.clone(),
                    parameters_schema: spec.parameters_schema.clone(),
                })
                .collect();
            if let Ok(updated) = serde_json::to_value(parsed) {
                input = updated;
            }
        }
        let target = param_str(ctx.config(), "model", DEFAULT_CHAT_MODEL).to_string();
        let schema = ctx.config().params.get("output_schema").cloned();
        Effect::Model {
            target,
            kind: ModelKind::Llm,
            input,
            schema,
        }
    }
}

impl AgentLoop for ChatLoop {
    fn id(&self) -> &str {
        "chat"
    }

    fn digest(&self) -> &str {
        "std_chat_loop_v1"
    }

    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        let drained_steering = ctx.drain_steering();

        if is_phase_entry(results.as_deref()) {
            self.retries_used = 0;
            return LoopAction::Yield(vec![self.build_model_effect(ctx)]);
        }

        let res_list = results.unwrap_or_default();
        if let Some(StepResult::Model {
            kind: ModelKind::Llm,
            output,
            ..
        }) = res_list.first()
        {
            self.retries_used = 0;
            let parsed: LlmOutput =
                serde_json::from_value(output.clone()).unwrap_or_else(|_| LlmOutput {
                    text: output.as_str().unwrap_or_default().to_string(),
                    ..LlmOutput::default()
                });

            if !parsed.tool_calls.is_empty() {
                let effects = parsed
                    .tool_calls
                    .into_iter()
                    .map(|tc| Effect::Tool {
                        side_effect: tool_side_effect(&self.allowed_tools, &tc.name),
                        name: tc.name,
                        args: tc.args,
                    })
                    .collect();
                return LoopAction::Yield(effects);
            }

            if !drained_steering.is_empty() {
                return LoopAction::Yield(vec![self.build_model_effect(ctx)]);
            }

            return LoopAction::Done(json!({
                "text": parsed.text,
                "reasoning": parsed.reasoning,
                "stop_reason": parsed.stop_reason,
            }));
        }

        if let Some(StepResult::Error {
            message, retryable, ..
        }) = res_list.first()
            && res_list.len() == 1
        {
            let max_retries = ctx
                .config()
                .params
                .get("max_retries")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_MAX_RETRIES, |n| n as u32);
            if *retryable && self.retries_used < max_retries {
                self.retries_used += 1;
                return LoopAction::Yield(vec![self.build_model_effect(ctx)]);
            }
            return LoopAction::Done(json!({
                "error": message,
                "retryable": retryable,
            }));
        }

        // Completed a tool batch (Tool / Denied / Aborted) or empty decorator resume -> next LLM step.
        LoopAction::Yield(vec![self.build_model_effect(ctx)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::StandardBuilder;
    use gestalt_kernel::{
        ArtifactStoreView, BudgetCap, DriverDirective, DriverResponse, KernelRuntime, PolicyGate,
        Preset, ReplayEngine, ReplayMode, SandboxPolicy, SideEffectClass, SteeringKind,
        SteeringSource, Usage,
    };

    #[test]
    fn chat_loop_handles_tools_steering_self_correction_and_replays() {
        let mut reg = ExtensionRegistry::new();
        reg.builders
            .insert("standard".into(), Box::new(StandardBuilder::default()));
        reg.loops.insert("chat".into(), Box::new(ChatLoopFactory));
        reg.tools.insert(
            "read".into(),
            ToolSpec {
                name: "read".into(),
                description: "Read file".into(),
                parameters_schema: json!({"type": "object"}),
                default_side_effect: SideEffectClass::Read,
                source_extension: None,
            },
        );
        reg.presets.insert(
            "general".into(),
            Preset {
                base_loop: "chat".into(),
                decorators: vec![],
                builder: "standard".into(),
                config: LoopConfig {
                    allowed_tools: Some(vec!["read".into()]),
                    params: json!({ "model": "mock_llm/v1" }),
                    ..LoopConfig::default()
                },
            },
        );

        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let mut rt = KernelRuntime::new("r1".into(), "/repo".into(), 100, reg, gate).expect("init");
        rt.start_turn(
            json!("check main.rs"),
            "general",
            &[],
            ArtifactStoreView::default(),
            101,
        )
        .expect("start");

        // Step 1: initial model call
        let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
        let eff1 = match d1 {
            DriverDirective::DispatchEffects { effects } => {
                assert_eq!(effects.len(), 1);
                effects[0].effect_id.clone()
            }
            other => panic!("expected DispatchEffects, got {other:?}"),
        };

        // Model requests a valid tool + an unknown tool in parallel
        let d2 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                effect_id: eff1,
                kind: ModelKind::Llm,
                output: json!({
                    "text": "",
                    "tool_calls": [
                        { "id": "tc1", "name": "read", "args": { "path": "main.rs" } },
                        { "id": "tc2", "name": "nonexistent", "args": {} }
                    ]
                }),
                usage: Usage::default(),
            }]))
            .expect("d2");
        let eff_read = match d2 {
            DriverDirective::DispatchEffects { effects } => {
                assert_eq!(effects.len(), 1);
                effects[0].effect_id.clone()
            }
            other => panic!("expected 1 approved tool effect, got {other:?}"),
        };

        // Enqueue intra-turn steer while tool is running
        rt.enqueue_steering(
            SteeringKind::Steer,
            json!("keep it brief"),
            SteeringSource::User,
            102,
        )
        .expect("steer");

        let d3 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Tool {
                effect_id: eff_read,
                output: json!({"content": "fn main() {}"}),
                is_error: false,
                blob_digest: None,
            }]))
            .expect("d3");
        let eff_model2 = match d3 {
            DriverDirective::DispatchEffects { effects } => {
                if let Effect::Model { input, .. } = &effects[0].effect {
                    let s = input.to_string();
                    assert!(s.contains("keep it brief"));
                    assert!(s.contains("unknown tool"));
                } else {
                    panic!("expected model effect");
                }
                effects[0].effect_id.clone()
            }
            other => panic!("expected model2, got {other:?}"),
        };

        let d4 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                effect_id: eff_model2,
                kind: ModelKind::Llm,
                output: json!({ "text": "Done!" }),
                usage: Usage::default(),
            }]))
            .expect("d4");
        assert!(matches!(d4, DriverDirective::TurnComplete { .. }));

        let report = ReplayEngine::new(rt.registry(), rt.policy_gate())
            .replay(rt.session_tree(), ReplayMode::Default, |_| None)
            .expect("replay");
        assert!(report.is_parity(), "divergences: {:?}", report.divergences);
    }
}
