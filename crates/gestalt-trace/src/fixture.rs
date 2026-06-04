use gestalt_core::{
    context::ContextPacket,
    event::AgentEvent,
    session::{ExecutionMode, SessionConfig},
    snapshot::WorkspaceSnapshot,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockToolConfig {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
    pub output: String,
    pub is_error: bool,
    pub parallel_safe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureInput {
    pub user_prompt: String,
    pub session_config: SessionConfig,
    pub execution_mode: ExecutionMode,
    pub tools: Vec<MockToolConfig>,
    pub policy_toml: Option<String>,
    pub mock_turns: Vec<Vec<AgentEvent>>,
    pub workspace_snapshot: Option<WorkspaceSnapshot>,
    #[serde(default)]
    pub approval_decisions: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceFixture {
    pub input: FixtureInput,
    pub context_packet: ContextPacket,
    pub expected: Vec<AgentEvent>,
}
