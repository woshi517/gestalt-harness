use crate::composition_hooks::{BeforeToolPolicyCtx, CompositionHooks, HookOutcome};
use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use async_trait::async_trait;
use gestalt_core::policy::{PolicyDecision, PolicyEngine, PolicyRequest};
use std::sync::Arc;

pub struct RuntimePolicyEngine {
    pub base: Arc<dyn PolicyEngine>,
    pub hooks: Arc<dyn CompositionHooks>,
    pub session_id: String,
    pub event_bus: RuntimeEventBus,
    pub skill_state: Option<Arc<std::sync::Mutex<crate::skill_contributor::SkillContributorState>>>,
}

#[async_trait]
impl PolicyEngine for RuntimePolicyEngine {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        self.event_bus.publish(RuntimeEvent::HookStarted {
            hook_name: "before_tool_policy".to_string(),
            lifecycle_point: "before_tool_policy".to_string(),
        });

        // Skill-scoped enforcement: fail-closed if a skill restricts tools
        let skill_policy = self.skill_state.as_ref().and_then(|state| {
            let guard = state.lock().ok()?;
            let active = guard.active_descriptors();
            let policy = gestalt_skills::effective_tool_policy(&active);
            if policy.restricts_tools {
                Some(policy)
            } else {
                None
            }
        });
        if let Some(ref policy) = skill_policy {
            if !policy.allows(&request.tool_name) {
                let mut allowed_tools: Vec<String> =
                    policy.allowed_tool_names.iter().cloned().collect();
                allowed_tools.sort();
                self.event_bus.publish(RuntimeEvent::SkillPolicyApplied {
                    skill_name: "active_skill_set".to_string(),
                    allowed_tools,
                });
                self.event_bus.publish(RuntimeEvent::HookCompleted {
                    hook_name: "before_tool_policy".to_string(),
                    lifecycle_point: "before_tool_policy".to_string(),
                    outcome: "Block (skill policy)".to_string(),
                });
                return PolicyDecision::denied(
                    format!(
                        "Tool '{}' is outside the active skill allowance",
                        request.tool_name
                    ),
                    "skill.policy".to_string(),
                );
            }
        }

        let ctx = BeforeToolPolicyCtx {
            session_id: self.session_id.clone(),
            tool_name: request.tool_name.clone(),
            tool_input: request.input.clone(),
        };

        match self.hooks.before_tool_policy(&ctx).await {
            Ok(outcome) => {
                self.event_bus.publish(RuntimeEvent::HookCompleted {
                    hook_name: "before_tool_policy".to_string(),
                    lifecycle_point: "before_tool_policy".to_string(),
                    outcome: format!("{:?}", outcome),
                });
                match outcome {
                    HookOutcome::Block { reason } => {
                        PolicyDecision::denied(reason, "hook.before_tool_policy".to_string())
                    }
                    _ => self.base.evaluate(request).await,
                }
            }
            Err(err) => {
                self.event_bus.publish(RuntimeEvent::HookFailed {
                    hook_name: "before_tool_policy".to_string(),
                    lifecycle_point: "before_tool_policy".to_string(),
                    error: err.to_string(),
                });
                PolicyDecision::denied(
                    format!("before_tool_policy hook failed to evaluate: {err}"),
                    "hook.before_tool_policy.error".to_string(),
                )
            }
        }
    }
}
