use serde::{Deserialize, Serialize};
use std::fmt;

use crate::{
    context::{
        ContextProjectionState, ContextStateDelta, MessageId, MessageNamespace, SessionId,
        SessionMessage, StateUpdate, TokenBudget,
    },
    message::Message,
    tool::ToolContext,
};

use crate::snapshot::{WorkspaceSnapshot, WorkspaceSnapshotter};

/// Ephemeral next-turn override state.
///
/// When present on a `Session`, this override applies to the very next request only
/// and is cleared immediately after the request is assembled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NextTurnOverride {
    pub model: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub variant: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: SessionId,
    pub message_namespace: MessageNamespace,
    pub config: SessionConfig,
    pub history: Vec<SessionMessage>,
    pub context_state: ContextProjectionState,
    pub token_budget: TokenBudget,
    pub tool_ctx: ToolContext,
    pub mode: ExecutionMode,
    pub snapshot: WorkspaceSnapshot,
    pub next_turn_override: Option<NextTurnOverride>,
    pub context_policy: crate::ContextManagementPolicy,
}

impl Session {
    pub fn new(
        id: impl Into<SessionId>,
        config: SessionConfig,
        token_budget: TokenBudget,
        tool_ctx: ToolContext,
        mode: ExecutionMode,
        snapshot: WorkspaceSnapshot,
    ) -> Self {
        Self {
            id: id.into(),
            message_namespace: uuid::Uuid::new_v4().to_string(),
            config,
            history: Vec::new(),
            context_state: ContextProjectionState::default(),
            token_budget,
            tool_ctx,
            mode,
            snapshot,
            next_turn_override: None,
            context_policy: crate::ContextManagementPolicy::default(),
        }
    }

    #[must_use]
    pub fn next_message_id(&self) -> MessageId {
        MessageId {
            origin_session_id: self.id.clone(),
            origin_message_namespace: self.message_namespace.clone(),
            sequence: self.history.len() as u64,
        }
    }

    pub fn append_message(&mut self, message: Message) -> MessageId {
        let id = self.next_message_id();
        let metadata = match &message {
            Message::User { metadata, .. } => metadata.clone(),
            _ => None,
        };
        self.history.push(SessionMessage {
            id: id.clone(),
            message,
            metadata,
        });
        id
    }

    pub fn apply_context_state_delta(&mut self, delta: ContextStateDelta) {
        match delta.active_checkpoint {
            StateUpdate::Unchanged => {}
            StateUpdate::Set(checkpoint) => {
                self.context_state.active_checkpoint = Some(checkpoint);
            }
            StateUpdate::Clear => {
                self.context_state.active_checkpoint = None;
            }
        }
        if !delta.cleared_tool_results.is_empty() {
            self.context_state
                .cleared_tool_results
                .extend(delta.cleared_tool_results.into_iter().map(|entry| {
                    (entry.tool_use_id.clone(), entry)
                }));
        }
        match delta.prompt_snapshot {
            StateUpdate::Unchanged => {}
            StateUpdate::Set(snapshot) => {
                self.context_state.prompt_snapshot = Some(snapshot);
            }
            StateUpdate::Clear => {
                self.context_state.prompt_snapshot = None;
            }
        }
        if let Some(epoch) = delta.context_epoch {
            self.context_state.context_epoch = epoch;
        }
        if delta.policy_fingerprint.is_some() {
            self.context_state.policy_fingerprint = delta.policy_fingerprint;
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
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub reasoning_effort: Option<crate::provider::ReasoningEffort>,
    #[serde(default)]
    pub text_verbosity: Option<crate::provider::TextVerbosity>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Confirm,
    Yolo,
    Human,
    DryRun,
    Replay,
}

impl ExecutionMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Confirm => "confirm",
            Self::Yolo => "yolo",
            Self::Human => "human",
            Self::DryRun => "dry_run",
            Self::Replay => "replay",
        }
    }
}

impl fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str((*self).as_str())
    }
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
