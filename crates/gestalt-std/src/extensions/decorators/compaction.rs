#![forbid(unsafe_code)]
//! Composable [`LoopDecorator`] (`"compaction"`) emitting lossless
//! `custom:std:compaction` summary nodes onto `SessionTree`.

use gestalt_kernel::{
    AgentLoop, Effect, ExtensionRegistry, LoopAction, LoopConfig, LoopContext, LoopDecorator,
    ModelKind, SessionSlice, StepResult,
};
use gestalt_runtime::ProviderErrorKind;
use serde_json::{Value, json};

use crate::extensions::loops::{param_str, parse_model_capabilities};
use crate::extensions::{
    ChatMessage, ChatRole, ContentBlock, LlmInput, LlmOutput, PromptTier, estimate_json_tokens,
};

const DEFAULT_COMPACTION_THRESHOLD: f64 = 0.80;
const DEFAULT_KEEP_RECENT_TURNS: usize = 2;

/// Composable [`LoopDecorator`] (`"compaction"`).
#[derive(Debug, Clone, Default)]
pub struct CompactionDecorator;

impl LoopDecorator for CompactionDecorator {
    fn id(&self) -> &str {
        "compaction"
    }

    fn digest(&self) -> &str {
        "std_compaction_v1"
    }

    fn wrap(
        &self,
        inner: Box<dyn AgentLoop>,
        _config: &LoopConfig,
        _registry: &ExtensionRegistry,
    ) -> Box<dyn AgentLoop> {
        Box::new(CompactingLoop {
            inner,
            state: CompactState::RunningInner,
            compacted_this_step: false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompactState {
    RunningInner,
    Compacting { cutoff_node_id: String },
}

struct CompactingLoop {
    inner: Box<dyn AgentLoop>,
    state: CompactState,
    compacted_this_step: bool,
}

impl CompactingLoop {
    fn find_cutoff(slice: &SessionSlice, keep_recent_turns: usize) -> Option<(String, String)> {
        let nodes = session_nodes_after_last_compaction(slice);
        if nodes.is_empty() {
            return None;
        }
        let turn_starts: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter_map(|(idx, n)| (n.kind == "turn_start").then_some(idx))
            .collect();

        let split_idx = if turn_starts.len() > keep_recent_turns {
            turn_starts[turn_starts.len() - keep_recent_turns]
        } else if nodes.len() > 4 {
            nodes.len() / 2
        } else {
            return None;
        };

        if split_idx == 0 {
            return None;
        }
        let cutoff_node_id = nodes[split_idx - 1].id.clone();
        let mut transcript = Vec::new();
        for n in &nodes[..split_idx] {
            transcript.push(format!("[{}] {}", n.kind, n.payload));
        }
        Some((cutoff_node_id, transcript.join("\n")))
    }

    fn has_manual_compact_request(slice: &SessionSlice) -> bool {
        for node in slice.nodes().iter().rev() {
            if node.kind == "custom:std:compaction" {
                return false;
            }
            if node.kind == "custom:std:compact_requested" {
                return true;
            }
        }
        false
    }

    fn build_compaction_effect(&self, ctx: &dyn LoopContext, transcript: String) -> Effect {
        let fallback_model = param_str(ctx.config(), "model", "default_llm");
        let target = param_str(ctx.config(), "compaction_model", fallback_model).to_string();
        let input = LlmInput {
            system_tiers: vec![PromptTier {
                name: "stable".into(),
                cacheable: false,
                content: "Summarize the following session history concisely, preserving file paths, decisions, errors, and next steps.".into(),
            }],
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: vec![ContentBlock::Text { text: transcript }],
            }],
            ..LlmInput::default()
        };
        Effect::Model {
            target,
            kind: ModelKind::Llm,
            input: serde_json::to_value(input).unwrap_or(Value::Null),
            schema: None,
        }
    }
}

fn session_nodes_after_last_compaction(slice: &SessionSlice) -> &[gestalt_kernel::SessionNode] {
    let nodes = slice.nodes();
    let mut start = 0;
    for (idx, node) in nodes.iter().enumerate() {
        if node.kind == "custom:std:compaction"
            && let Some(up_to) = node
                .payload
                .get("compacted_up_to_node_id")
                .and_then(Value::as_str)
            && let Some(pos) = nodes.iter().position(|n| n.id == up_to)
        {
            start = (pos + 1).max(idx + 1);
        }
    }
    &nodes[start..]
}

impl AgentLoop for CompactingLoop {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn digest(&self) -> &str {
        self.inner.digest()
    }

    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        let keep_recent_turns = ctx
            .config()
            .params
            .get("keep_recent_turns")
            .and_then(Value::as_u64)
            .map_or(DEFAULT_KEEP_RECENT_TURNS, |n| n as usize);
        let threshold = ctx
            .config()
            .params
            .get("compaction_threshold")
            .and_then(Value::as_f64)
            .unwrap_or(DEFAULT_COMPACTION_THRESHOLD);

        if let CompactState::Compacting { cutoff_node_id } = self.state.clone() {
            self.state = CompactState::RunningInner;
            let res_list = results.unwrap_or_default();
            if let Some(StepResult::Model { output, .. }) = res_list.first() {
                let summary = serde_json::from_value::<LlmOutput>(output.clone())
                    .map(|o| o.text)
                    .ok()
                    .filter(|s| !s.is_empty())
                    .or_else(|| output.as_str().map(ToString::to_string))
                    .unwrap_or_else(|| output.to_string());
                ctx.emit_custom_node(
                    "std:compaction",
                    json!({
                        "summary": summary,
                        "compacted_up_to_node_id": cutoff_node_id,
                    }),
                );
                return self.inner.step(ctx, None);
            }
            return self.inner.step(ctx, Some(res_list));
        }

        // Reactive ContextLengthExceeded interception.
        if !self.compacted_this_step
            && let Some(ref res_list) = results
            && let Some(StepResult::Error { message, .. }) = res_list.first()
            && ProviderErrorKind::from_error_message(message)
                == Some(ProviderErrorKind::ContextLengthExceeded)
            && let Some((cutoff_node_id, transcript)) =
                Self::find_cutoff(ctx.session(), keep_recent_turns)
        {
            self.compacted_this_step = true;
            self.state = CompactState::Compacting { cutoff_node_id };
            return LoopAction::Yield(vec![self.build_compaction_effect(ctx, transcript)]);
        }

        let action = self.inner.step(ctx, results);
        match &action {
            LoopAction::Yield(effects) if !self.compacted_this_step => {
                let model_input = effects.iter().find_map(|eff| match eff {
                    Effect::Model {
                        kind: ModelKind::Llm,
                        input,
                        ..
                    } => Some(input),
                    _ => None,
                });
                if let Some(input) = model_input {
                    let caps = parse_model_capabilities(ctx.config());
                    let max_tokens = caps.max_context_tokens.max(1);
                    let limit = ((max_tokens as f64) * threshold).round() as u64;
                    let est = estimate_json_tokens(input);
                    let manual = Self::has_manual_compact_request(ctx.session());
                    if (est >= limit || manual)
                        && let Some((cutoff_node_id, transcript)) =
                            Self::find_cutoff(ctx.session(), keep_recent_turns)
                    {
                        self.compacted_this_step = true;
                        self.state = CompactState::Compacting { cutoff_node_id };
                        return LoopAction::Yield(vec![
                            self.build_compaction_effect(ctx, transcript),
                        ]);
                    }
                }
                self.compacted_this_step = false;
                action
            }
            _ => {
                self.compacted_this_step = false;
                action
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
        Preset, ReplayEngine, ReplayMode, SandboxPolicy, Usage,
    };

    #[test]
    fn compaction_emits_custom_node_on_context_length_exceeded_and_replays() {
        let mut reg = ExtensionRegistry::new();
        reg.builders
            .insert("standard".into(), Box::new(StandardBuilder::default()));
        reg.loops.insert("chat".into(), Box::new(ChatLoopFactory));
        reg.decorators
            .insert("compaction".into(), Box::new(CompactionDecorator));
        reg.presets.insert(
            "general".into(),
            Preset {
                base_loop: "chat".into(),
                decorators: vec!["compaction".into()],
                builder: "standard".into(),
                config: LoopConfig {
                    params: json!({ "keep_recent_turns": 1 }),
                    ..LoopConfig::default()
                },
            },
        );

        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let mut rt =
            KernelRuntime::new("r_cmp".into(), "/repo".into(), 10, reg, gate).expect("init");

        // Turn 1 completes normally
        rt.start_turn(
            json!("turn 1"),
            "general",
            &[],
            ArtifactStoreView::default(),
            11,
        )
        .expect("t1");
        let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
        let e1 = match d1 {
            DriverDirective::DispatchEffects { effects } => effects[0].effect_id.clone(),
            _ => panic!("expected dispatch"),
        };
        let _ = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                effect_id: e1,
                kind: ModelKind::Llm,
                output: json!({ "text": "ok 1" }),
                usage: Usage::default(),
            }]))
            .expect("t1 done");

        // Turn 2 hits ContextLengthExceeded -> CompactionDecorator intercepts and compacts
        rt.start_turn(
            json!("turn 2"),
            "general",
            &[],
            ArtifactStoreView::default(),
            20,
        )
        .expect("t2");
        let d2 = rt.next_action(DriverResponse::Poll).expect("d2");
        let e2 = match d2 {
            DriverDirective::DispatchEffects { effects } => effects[0].effect_id.clone(),
            _ => panic!("expected dispatch"),
        };
        let d3 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Error {
                effect_id: e2,
                message: "[context_length_exceeded] prompt too long".into(),
                retryable: false,
            }]))
            .expect("d3");
        let e_cmp = match d3 {
            DriverDirective::DispatchEffects { effects } => effects[0].effect_id.clone(),
            _ => panic!("expected compaction model call"),
        };
        let d4 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                effect_id: e_cmp,
                kind: ModelKind::Llm,
                output: json!({ "text": "Summary of Turn 1" }),
                usage: Usage::default(),
            }]))
            .expect("d4");
        let e_retry = match d4 {
            DriverDirective::DispatchEffects { effects } => {
                if let Effect::Model { input, .. } = &effects[0].effect {
                    assert!(input.to_string().contains("Summary of Turn 1"));
                } else {
                    panic!("expected model");
                }
                effects[0].effect_id.clone()
            }
            _ => panic!("expected post-compaction model"),
        };
        let d5 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                effect_id: e_retry,
                kind: ModelKind::Llm,
                output: json!({ "text": "Final answer" }),
                usage: Usage::default(),
            }]))
            .expect("d5");
        assert!(matches!(d5, DriverDirective::TurnComplete { .. }));

        let report = ReplayEngine::new(rt.registry(), rt.policy_gate())
            .replay(rt.session_tree(), ReplayMode::Default, |_| None)
            .expect("replay");
        assert!(report.is_parity(), "divergences: {:?}", report.divergences);
    }
}
