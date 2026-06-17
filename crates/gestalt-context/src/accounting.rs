use gestalt_core::context::TokenBudget;
use gestalt_core::message::Message;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurabilityMode {
    Required,
    BestEffort,
    Disabled,
}

impl Default for DurabilityMode {
    fn default() -> Self {
        Self::Required
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextManagementPolicy {
    pub enabled: bool,
    pub buffer_tokens: usize,
    pub keep_recent_tokens: usize,
    pub keep_recent_turns: usize,
    pub tool_result_budget_ratio: f64,
    pub compaction_target_ratio: f64,
    pub durability: DurabilityMode,
    pub profile: String,
}

impl Default for ContextManagementPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            buffer_tokens: 4096,
            keep_recent_tokens: 8192,
            keep_recent_turns: 5,
            tool_result_budget_ratio: 0.5,
            compaction_target_ratio: 0.8,
            durability: DurabilityMode::Required,
            profile: "default".to_string(),
        }
    }
}

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
