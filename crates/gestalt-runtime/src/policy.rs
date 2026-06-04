use std::sync::Arc;
use async_trait::async_trait;
use gestalt_core::policy::{PolicyEngine, PolicyDecision, PolicyRequest};
use crate::composition_hooks::{BeforeToolPolicyCtx, HookOutcome, CompositionHooks};

pub struct RuntimePolicyEngine {
    pub base: Arc<dyn PolicyEngine>,
    pub hooks: Arc<dyn CompositionHooks>,
    pub session_id: String,
}

#[async_trait]
impl PolicyEngine for RuntimePolicyEngine {
    async fn evaluate(&self, request: PolicyRequest) -> PolicyDecision {
        let ctx = BeforeToolPolicyCtx {
            session_id: self.session_id.clone(),
            tool_name: request.tool_name.clone(),
            tool_input: request.input.clone(),
        };

        match self.hooks.before_tool_policy(&ctx).await {
            Ok(HookOutcome::Block { reason }) => {
                PolicyDecision::denied(reason, "hook.before_tool_policy".to_string())
            }
            Ok(_) => {
                self.base.evaluate(request).await
            }
            Err(err) => {
                PolicyDecision::denied(
                    format!("before_tool_policy hook failed to evaluate: {err}"),
                    "hook.before_tool_policy.error".to_string(),
                )
            }
        }
    }
}
