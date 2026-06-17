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
        ((self.usable_limit() as f64) * self.policy.tool_result_budget_ratio) as usize
    }

    pub fn compaction_target(&self) -> usize {
        ((self.usable_limit() as f64) * self.policy.compaction_target_ratio) as usize
    }

    pub fn needs_management(&self, current_total_tokens: usize) -> bool {
        self.policy.enabled && current_total_tokens > self.usable_limit()
    }
}
