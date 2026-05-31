use serde::{Deserialize, Serialize};

use crate::{context::TokenBudget, message::Message, tool::ToolContext};

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub config: SessionConfig,
    pub history: Vec<Message>,
    pub token_budget: TokenBudget,
    pub tool_ctx: ToolContext,
    pub mode: ExecutionMode,
}

impl Session {
    pub fn new(
        id: impl Into<String>,
        config: SessionConfig,
        token_budget: TokenBudget,
        tool_ctx: ToolContext,
        mode: ExecutionMode,
    ) -> Self {
        Self {
            id: id.into(),
            config,
            history: Vec::new(),
            token_budget,
            tool_ctx,
            mode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub model: String,
    pub provider: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub max_turns: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Confirm,
    Yolo,
    Human,
    DryRun,
    Replay,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    pub session_id: String,
    pub turns: usize,
    pub stop_reason: crate::event::StopReason,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub artifacts: Vec<String>,
}
