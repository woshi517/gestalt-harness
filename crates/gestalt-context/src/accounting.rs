use gestalt_core::context::TokenBudget;
use gestalt_core::message::Message;
pub use gestalt_core::{ContextManagementPolicy, DurabilityMode};

pub struct ContextAccountant<'a> {
    pub budget: &'a TokenBudget,
    pub policy: &'a ContextManagementPolicy,
    pub history: &'a [Message],
}

impl<'a> ContextAccountant<'a> {
    pub fn new(
        budget: &'a TokenBudget,
        policy: &'a ContextManagementPolicy,
        history: &'a [Message],
    ) -> Self {
        Self {
            budget,
            policy,
            history,
        }
    }

    pub fn usable_limit(&self) -> usize {
        self.budget
            .model_limit
            .saturating_sub(self.budget.reserved_output)
            .saturating_sub(self.policy.buffer_tokens)
    }

    pub fn tool_result_budget(&self) -> usize {
        self.scaled_limit(self.policy.tool_result_budget_ratio)
    }

    pub fn compaction_target(&self) -> usize {
        self.scaled_limit(self.policy.compaction_target_ratio)
    }

    pub fn needs_management(&self, current_total_tokens: usize) -> bool {
        self.policy.enabled && current_total_tokens > self.usable_limit()
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    fn scaled_limit(&self, ratio: f64) -> usize {
        let clamped_ratio = if ratio.is_finite() {
            ratio.clamp(0.0, 1.0)
        } else {
            0.0
        };
        ((self.usable_limit() as f64) * clamped_ratio).floor() as usize
    }
}
