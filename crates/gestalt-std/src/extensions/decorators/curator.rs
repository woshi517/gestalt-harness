#![forbid(unsafe_code)]
//! Composable [`LoopDecorator`] (`"curator"`) for `v0.1` staged self-improvement
//! (`Effect::Save(Stage)` -> `Effect::Pause(Approval)` -> `Effect::Save(Promote)`).

use gestalt_kernel::{
    AgentLoop, ArtifactPatch, Effect, ExtensionRegistry, LoopAction, LoopConfig, LoopContext,
    LoopDecorator, ModelKind, PauseReason, SaveMode, StepResult,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::extensions::loops::param_str;
use crate::extensions::{ChatMessage, ChatRole, ContentBlock, LlmInput, LlmOutput, PromptTier};

/// Composable [`LoopDecorator`] (`"curator"`).
///
/// In `v0.1`, intercepts `LoopAction::Done`, proposes or reads candidate
/// `MEMORY.md`/`SKILL.md` patches, stages them via `SaveMode::Stage`, requests
/// operator/policy approval via `Effect::Pause(Approval)`, and promotes via
/// `SaveMode::Promote` only when approved. Automated multi-trace replay
/// regression gating lands in `v0.2` (Milestone 2.3).
#[derive(Debug, Clone, Default)]
pub struct CuratorDecorator;

impl LoopDecorator for CuratorDecorator {
    fn id(&self) -> &str {
        "curator"
    }

    fn digest(&self) -> &str {
        "std_curator_v1"
    }

    fn wrap(
        &self,
        inner: Box<dyn AgentLoop>,
        _config: &LoopConfig,
        _registry: &ExtensionRegistry,
    ) -> Box<dyn AgentLoop> {
        Box::new(CuratorLoop {
            inner,
            state: CuratorState::RunningInner,
            final_output: None,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct CandidateProposal {
    artifact: String,
    patch: ArtifactPatch,
}

#[derive(Debug, Clone)]
enum CuratorState {
    RunningInner,
    Proposing,
    Staging {
        proposal: CandidateProposal,
    },
    AwaitingApproval {
        proposal: CandidateProposal,
        stage_id: String,
    },
    Promoting {
        stage_id: String,
    },
}

struct CuratorLoop {
    inner: Box<dyn AgentLoop>,
    state: CuratorState,
    final_output: Option<Value>,
}

impl CuratorLoop {
    fn is_curatable_artifact(path: &str) -> bool {
        let norm = path.trim_start_matches("./");
        norm.starts_with(".gestalt/memories/") || norm.starts_with(".gestalt/skills/")
    }

    fn build_proposal_effect(ctx: &dyn LoopContext, final_output: &Value) -> Effect {
        let fallback = param_str(ctx.config(), "model", "default_llm");
        let target = param_str(ctx.config(), "curator_model", fallback).to_string();
        let input = LlmInput {
            system_tiers: vec![PromptTier {
                name: "stable".into(),
                cacheable: false,
                content: "Inspect the completed task output. If a reusable memory or skill update is warranted, return JSON {\"artifact\": \".gestalt/memories/...\", \"patch\": {\"patch_kind\": \"write_file\", \"content\": \"...\"}}; otherwise return null.".into(),
            }],
            messages: vec![ChatMessage {
                role: ChatRole::User,
                content: vec![ContentBlock::Text {
                    text: format!("Completed output: {final_output}"),
                }],
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

impl AgentLoop for CuratorLoop {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn digest(&self) -> &str {
        self.inner.digest()
    }

    fn step(&mut self, ctx: &mut dyn LoopContext, results: Option<Vec<StepResult>>) -> LoopAction {
        match std::mem::replace(&mut self.state, CuratorState::RunningInner) {
            CuratorState::RunningInner => match self.inner.step(ctx, results) {
                LoopAction::Yield(effs) => LoopAction::Yield(effs),
                LoopAction::Done(output) => {
                    if ctx
                        .config()
                        .params
                        .get("curator_enabled")
                        .and_then(Value::as_bool)
                        == Some(false)
                    {
                        return LoopAction::Done(output);
                    }
                    self.final_output = Some(output.clone());

                    if let Some(cand_val) = ctx.config().params.get("curator_candidate")
                        && let Ok(proposal) =
                            serde_json::from_value::<CandidateProposal>(cand_val.clone())
                        && Self::is_curatable_artifact(&proposal.artifact)
                    {
                        let eff = Effect::Save {
                            artifact: proposal.artifact.clone(),
                            patch: proposal.patch.clone(),
                            mode: SaveMode::Stage,
                        };
                        self.state = CuratorState::Staging { proposal };
                        return LoopAction::Yield(vec![eff]);
                    }

                    self.state = CuratorState::Proposing;
                    LoopAction::Yield(vec![Self::build_proposal_effect(ctx, &output)])
                }
            },
            CuratorState::Proposing => {
                let res_list = results.unwrap_or_default();
                if let Some(StepResult::Model { output, .. }) = res_list.first() {
                    let candidate_val = serde_json::from_value::<LlmOutput>(output.clone())
                        .ok()
                        .and_then(|o| serde_json::from_str::<Value>(&o.text).ok())
                        .unwrap_or_else(|| output.clone());
                    if let Ok(proposal) = serde_json::from_value::<CandidateProposal>(candidate_val)
                        && Self::is_curatable_artifact(&proposal.artifact)
                    {
                        let eff = Effect::Save {
                            artifact: proposal.artifact.clone(),
                            patch: proposal.patch.clone(),
                            mode: SaveMode::Stage,
                        };
                        self.state = CuratorState::Staging { proposal };
                        return LoopAction::Yield(vec![eff]);
                    }
                }
                LoopAction::Done(self.final_output.take().unwrap_or(Value::Null))
            }
            CuratorState::Staging { proposal } => {
                let res_list = results.unwrap_or_default();
                if let Some(StepResult::Saved {
                    stage_id,
                    diff_summary,
                    digest,
                    ..
                }) = res_list.first()
                {
                    let pause = Effect::Pause {
                        reason: PauseReason::Approval,
                        payload: json!({
                            "dialog": "curator_approval",
                            "action": "curator_promote",
                            "stage_id": stage_id,
                            "artifact": proposal.artifact,
                            "diff_summary": diff_summary,
                            "digest": digest,
                        }),
                    };
                    self.state = CuratorState::AwaitingApproval {
                        proposal,
                        stage_id: stage_id.clone(),
                    };
                    return LoopAction::Yield(vec![pause]);
                }
                LoopAction::Done(self.final_output.take().unwrap_or(Value::Null))
            }
            CuratorState::AwaitingApproval { proposal, stage_id } => {
                let res_list = results.unwrap_or_default();
                let approved = matches!(
                    res_list.first(),
                    Some(StepResult::PauseResolved { resolution, .. })
                        if resolution.get("approved").and_then(Value::as_bool) == Some(true)
                );
                if approved {
                    self.state = CuratorState::Promoting { stage_id };
                    LoopAction::Yield(vec![Effect::Save {
                        artifact: proposal.artifact,
                        patch: proposal.patch,
                        mode: SaveMode::Promote,
                    }])
                } else {
                    LoopAction::Done(json!({
                        "output": self.final_output.take().unwrap_or(Value::Null),
                        "curator": {
                            "promoted": false,
                            "stage_id": stage_id,
                        }
                    }))
                }
            }
            CuratorState::Promoting { stage_id } => {
                let res_list = results.unwrap_or_default();
                let promoted = matches!(res_list.first(), Some(StepResult::Saved { .. }));
                LoopAction::Done(json!({
                    "output": self.final_output.take().unwrap_or(Value::Null),
                    "curator": {
                        "promoted": promoted,
                        "stage_id": stage_id,
                    }
                }))
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
        Preset, SandboxPolicy, Usage,
    };

    #[test]
    fn curator_stages_pauses_and_promotes_only_when_approved() {
        let mut reg = ExtensionRegistry::new();
        reg.builders
            .insert("standard".into(), Box::new(StandardBuilder::default()));
        reg.loops.insert("chat".into(), Box::new(ChatLoopFactory));
        reg.decorators
            .insert("curator".into(), Box::new(CuratorDecorator));
        reg.presets.insert(
            "swe_curated".into(),
            Preset {
                base_loop: "chat".into(),
                decorators: vec!["curator".into()],
                builder: "standard".into(),
                config: LoopConfig {
                    params: json!({
                        "curator_candidate": {
                            "artifact": ".gestalt/memories/MEMORY.md",
                            "patch": { "patch_kind": "write_file", "content": "# Lessons\nUse cargo test.\n" }
                        }
                    }),
                    ..LoopConfig::default()
                },
            },
        );

        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let mut rt =
            KernelRuntime::new("r_cur".into(), "/repo".into(), 10, reg, gate).expect("init");
        rt.start_turn(
            json!("remember lesson"),
            "swe_curated",
            &[],
            ArtifactStoreView::default(),
            11,
        )
        .expect("start");

        let d1 = rt.next_action(DriverResponse::Poll).expect("d1");
        let e1 = match d1 {
            DriverDirective::DispatchEffects { effects } => effects[0].effect_id.clone(),
            _ => panic!("expected model"),
        };
        let d2 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                effect_id: e1,
                kind: ModelKind::Llm,
                output: json!({ "text": "Done" }),
                usage: Usage::default(),
            }]))
            .expect("d2");
        let e_stage = match d2 {
            DriverDirective::DispatchEffects { effects } => {
                assert!(matches!(
                    &effects[0].effect,
                    Effect::Save {
                        mode: SaveMode::Stage,
                        ..
                    }
                ));
                effects[0].effect_id.clone()
            }
            other => panic!("expected Save(Stage), got {other:?}"),
        };

        let d3 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Saved {
                effect_id: e_stage,
                stage_id: "stg_1".into(),
                diff_summary: "created MEMORY.md".into(),
                digest: "abc".into(),
            }]))
            .expect("d3");
        let (pause_id, eff_pause) = match d3 {
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

        let d4 = rt
            .next_action(DriverResponse::PauseResolution {
                pause_id,
                effect_id: eff_pause,
                resolution: json!({ "approved": true }),
            })
            .expect("d4");
        let e_promote = match d4 {
            DriverDirective::DispatchEffects { effects } => {
                assert!(matches!(
                    &effects[0].effect,
                    Effect::Save {
                        mode: SaveMode::Promote,
                        ..
                    }
                ));
                effects[0].effect_id.clone()
            }
            other => panic!("expected Save(Promote), got {other:?}"),
        };

        let d5 = rt
            .next_action(DriverResponse::EffectResults(vec![StepResult::Saved {
                effect_id: e_promote,
                stage_id: "stg_1".into(),
                diff_summary: "promoted MEMORY.md".into(),
                digest: "abc".into(),
            }]))
            .expect("d5");
        match d5 {
            DriverDirective::TurnComplete { output, .. } => {
                assert_eq!(output["curator"]["promoted"], true);
            }
            other => panic!("expected TurnComplete, got {other:?}"),
        }
    }
}
