use crate::config::EffectiveConfig;
use crate::tui::services::{LineageTreeModel, SessionListModel};
use gestalt_core::{approval::ApprovalRequest, event::AgentEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiFocus {
    ChatPrompt,
    LineageTree,
    Details,
    Diagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiModal {
    None,
    SessionSwitcher,
    Help,
    Approval,
    Onboarding,
    Notification,
}

pub struct OnboardingState {
    pub providers: Vec<String>,
    pub selected_idx: usize,
    pub api_key: String,
    pub is_key_focused: bool,
    pub error_message: Option<String>,
}

pub struct ChromeState {
    pub active_focus: TuiFocus,
    pub active_modal: TuiModal,
    pub terminal_width: u16,
    pub terminal_height: u16,
    pub sidebar_open: bool,
    pub details_open: bool,
    pub diagnostics_open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    User(String),
    Agent(String),
    Thinking(String),
    System(String),
    ToolCall { name: String },
    ToolResult { name: String, is_error: bool },
    Checkpoint,
    Interrupted(String),
    Stop(String),
    Error(String),
    Policy { tool_name: String, risk: String },
    ModelRequest { model: String },
    Other(String),
}

impl TranscriptEntry {
    pub fn from_agent_event(event: AgentEvent) -> Self {
        match event {
            AgentEvent::UserMessage { content } => TranscriptEntry::User(content),
            AgentEvent::Text { delta } => TranscriptEntry::Agent(delta),
            AgentEvent::Thinking { delta } => TranscriptEntry::Thinking(delta),
            AgentEvent::ContextBuilt {
                prompt_source,
                token_estimate,
                ..
            } => {
                let msg = prompt_source
                    .unwrap_or_else(|| format!("Context built (~{} tokens)", token_estimate));
                TranscriptEntry::System(msg)
            }
            AgentEvent::ModelRequest { model, .. } => TranscriptEntry::ModelRequest { model },
            AgentEvent::ToolCallStreamed { name, .. }
            | AgentEvent::ToolCallProposed { name, .. } => TranscriptEntry::ToolCall { name },
            AgentEvent::ToolResult {
                tool_name,
                is_error,
                ..
            } => TranscriptEntry::ToolResult {
                name: tool_name.unwrap_or_else(|| "unknown".to_string()),
                is_error,
            },
            AgentEvent::Checkpoint { .. } => TranscriptEntry::Checkpoint,
            AgentEvent::Interrupted { reason } => TranscriptEntry::Interrupted(reason),
            AgentEvent::Stop { reason } => TranscriptEntry::Stop(format!("{:?}", reason)),
            AgentEvent::Error { message, .. } => TranscriptEntry::Error(message),
            AgentEvent::PolicyDecision {
                tool_name, risk, ..
            } => TranscriptEntry::Policy {
                tool_name: tool_name.unwrap_or_default(),
                risk: format!("{:?}", risk),
            },
            other => TranscriptEntry::Other(format!("{:?}", other)),
        }
    }
}

pub fn push_event(entries: &mut Vec<TranscriptEntry>, event: AgentEvent) {
    match event {
        AgentEvent::Text { delta } => {
            if let Some(TranscriptEntry::Agent(existing)) = entries.last_mut() {
                existing.push_str(&delta);
            } else {
                entries.push(TranscriptEntry::Agent(delta));
            }
        }
        AgentEvent::Thinking { delta } => {
            if let Some(TranscriptEntry::Thinking(existing)) = entries.last_mut() {
                existing.push_str(&delta);
            } else {
                entries.push(TranscriptEntry::Thinking(delta));
            }
        }
        other => {
            entries.push(TranscriptEntry::from_agent_event(other));
        }
    }
}

pub struct ChatState {
    pub events: Vec<TranscriptEntry>,
    pub input_buffer: String,
    pub scroll_offset: usize,
    pub autocomplete_index: usize,
}

pub struct LineageState {
    pub model: Option<LineageTreeModel>,
    pub selected_index: usize,
}

pub struct DetailsState {
    pub config: Option<EffectiveConfig>,
    pub scroll_offset: usize,
}

pub struct DiagnosticsState {
    pub scroll_offset: usize,
}

pub struct SessionSwitcherState {
    pub model: Option<SessionListModel>,
    pub selected_index: usize,
}

pub struct ApprovalModalState {
    pub active_request: Option<ApprovalRequest>,
}

pub struct NotificationState {
    pub title: String,
    pub message: String,
    pub is_error: bool,
}

pub struct TuiAppState {
    pub chrome: ChromeState,
    pub chat: ChatState,
    pub lineage: LineageState,
    pub details: DetailsState,
    pub diagnostics: DiagnosticsState,
    pub switcher: SessionSwitcherState,
    pub approval: ApprovalModalState,
    pub notification: Option<NotificationState>,
    pub onboarding: OnboardingState,
    pub session_id: String,
    pub parent_run_id: Option<String>,
    pub status: String,
    pub is_running: bool,
    pub run_error: Option<String>,
    pub config: EffectiveConfig,
}

impl TuiAppState {
    pub fn new(config: EffectiveConfig, session_id: String, parent_run_id: Option<String>) -> Self {
        Self {
            chrome: ChromeState {
                active_focus: TuiFocus::ChatPrompt,
                active_modal: TuiModal::None,
                terminal_width: 100,
                terminal_height: 30,
                sidebar_open: false,
                details_open: false,
                diagnostics_open: false,
            },
            chat: ChatState {
                events: Vec::new(),
                input_buffer: String::new(),
                scroll_offset: 0,
                autocomplete_index: 0,
            },
            lineage: LineageState {
                model: None,
                selected_index: 0,
            },
            details: DetailsState {
                config: Some(config.clone()),
                scroll_offset: 0,
            },
            diagnostics: DiagnosticsState { scroll_offset: 0 },
            switcher: SessionSwitcherState {
                model: None,
                selected_index: 0,
            },
            approval: ApprovalModalState {
                active_request: None,
            },
            notification: None,
            onboarding: OnboardingState {
                providers: vec![
                    "openrouter".to_string(),
                    "openai".to_string(),
                    "anthropic".to_string(),
                    "gemini".to_string(),
                    "groq".to_string(),
                    "ollama".to_string(),
                ],
                selected_idx: 0,
                api_key: String::new(),
                is_key_focused: false,
                error_message: None,
            },
            session_id,
            parent_run_id,
            status: "Ready".to_string(),
            is_running: false,
            run_error: None,
            config,
        }
    }

    pub fn has_started_session(&self) -> bool {
        self.parent_run_id.is_some() || !self.chat.events.is_empty()
    }

    pub fn start_new_session(&mut self) {
        self.session_id = format!("session-{}", uuid::Uuid::new_v4());
        self.parent_run_id = None;
        self.chat.events.clear();
        self.chat.input_buffer.clear();
        self.chat.scroll_offset = 0;
        self.chat.autocomplete_index = 0;
        self.lineage.model = None;
        self.lineage.selected_index = 0;
        self.run_error = None;
        self.status = "Ready".to_string();
        self.chrome.sidebar_open = false;
        self.chrome.active_focus = TuiFocus::ChatPrompt;
    }

    pub fn show_notification(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        is_error: bool,
    ) {
        self.notification = Some(NotificationState {
            title: title.into(),
            message: message.into(),
            is_error,
        });
        self.chrome.active_modal = TuiModal::Notification;
    }
}
