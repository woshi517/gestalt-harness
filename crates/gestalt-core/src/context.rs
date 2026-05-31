use crate::message::Message;

pub trait ContextPipeline: Send + Sync {
    fn process(&self, history: &[Message], budget: &TokenBudget) -> Vec<Message>;

    fn version(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenBudget {
    pub model_limit: usize,
    pub reserved_output: usize,
    pub used_system: usize,
    pub used_history: usize,
    pub used_sources: usize,
    pub used_tools: usize,
    pub used_memory: usize,
    pub minimum_turn_budget: usize,
}

impl TokenBudget {
    pub fn available_total(&self) -> usize {
        self.model_limit
            .saturating_sub(self.reserved_output)
            .saturating_sub(self.used_system)
            .saturating_sub(self.used_history)
            .saturating_sub(self.used_sources)
            .saturating_sub(self.used_tools)
            .saturating_sub(self.used_memory)
    }

    pub fn exhausted(&self) -> bool {
        self.available_total() < self.minimum_turn_budget
    }

    pub fn record_usage(&mut self, input_tokens: usize, _output_tokens: usize) {
        let non_history = self.used_system
            .saturating_add(self.used_sources)
            .saturating_add(self.used_tools)
            .saturating_add(self.used_memory);
        self.used_history = input_tokens.saturating_sub(non_history);
    }
}
