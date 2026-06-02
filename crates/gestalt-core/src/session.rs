use serde::{Deserialize, Serialize};

use crate::{context::TokenBudget, message::Message, tool::ToolContext};

use crate::snapshot::{WorkspaceSnapshot, WorkspaceSnapshotter};

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub config: SessionConfig,
    pub history: Vec<Message>,
    pub token_budget: TokenBudget,
    pub tool_ctx: ToolContext,
    pub mode: ExecutionMode,
    pub snapshot: WorkspaceSnapshot,
}

impl Session {
    pub fn new(
        id: impl Into<String>,
        config: SessionConfig,
        token_budget: TokenBudget,
        tool_ctx: ToolContext,
        mode: ExecutionMode,
        snapshot: WorkspaceSnapshot,
    ) -> Self {
        Self {
            id: id.into(),
            config,
            history: Vec::new(),
            token_budget,
            tool_ctx,
            mode,
            snapshot,
        }
    }

    pub async fn refresh_snapshot<S: WorkspaceSnapshotter + ?Sized>(
        &mut self,
        snapshotter: &S,
        trace_sink: Option<&dyn crate::trace::TraceSink>,
    ) -> crate::error::Result<()> {
        let root = self
            .tool_ctx
            .workspace_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let new_snapshot = snapshotter.capture(&root).await?;
        self.snapshot = new_snapshot.clone();

        if let Some(sink) = trace_sink {
            sink.update_snapshot(new_snapshot.clone());
            let snapshot_id: String = new_snapshot.content_hash.chars().take(12).collect();
            let event = crate::event::AgentEvent::WorkspaceSnapshotCaptured {
                snapshot_id,
                dirty: new_snapshot.git_dirty.unwrap_or(false),
            };
            sink.emit(event)?;
        }

        Ok(())
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
    #[serde(default)]
    pub workspace_snapshot_id: Option<String>,
}
