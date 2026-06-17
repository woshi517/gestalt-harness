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

    pub fn projected_next_turn_growth(&self) -> usize {
        std::cmp::max(
            self.budget.minimum_turn_budget,
            self.budget.reserved_output.saturating_div(2),
        )
    }

    pub fn needs_management(&self, current_total_tokens: usize) -> bool {
        self.policy.enabled && current_total_tokens > self.usable_limit()
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn scaled_limit(&self, ratio: f64) -> usize {
        ((self.usable_limit() as f64) * ratio).floor() as usize
    }
}
