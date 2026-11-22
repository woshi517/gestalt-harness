#![forbid(unsafe_code)]
//! Pure TUI application state, `RunEvent` reducers, keyboard navigation, and
//! scroll-offset-aware mouse hit-testing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use gestalt_kernel::{
    BudgetSnapshot, Effect, ExtensionRegistry, ModelKind, NodePayload, PauseReason, RunEvent,
    SessionNode, SessionTree, SteeringKind, StepResult, TurnStatus, UIActionSpec, UIPanelSpec,
    UIRendererSpec,
};
use gestalt_runtime::TriggerStatus;
use ratatui::layout::Rect;
use serde_json::{Value, json};

use crate::surfaces::tui::components::{SelectListItem, SelectListState};
use crate::surfaces::{PausePrompt, extract_stream_text};

/// Execution status of a Step card inside the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStepStatus {
    /// Queued and awaiting dispatch.
    Pending,
    /// Currently executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Returned an error.
    Error,
    /// Denied by `PolicyGate` or sandbox.
    Denied,
    /// Aborted mid-step.
    Aborted,
}

/// Structured unified diff payload attached to an `edit` Step card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffPreviewData {
    /// Workspace-relative file path.
    pub path: String,
    /// Added line count.
    pub additions: u64,
    /// Deleted line count.
    pub deletions: u64,
    /// Unified diff body.
    pub unified_diff: String,
}

/// Pre-flight status of the active model and provider credentials.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelConfigStatus {
    /// Target model resolved and required credentials are present.
    Ready {
        /// Resolved target string (e.g., `"anthropic/claude-sonnet-4-6"`).
        target: String,
    },
    /// No model configured (`default_llm` unresolved).
    #[default]
    Unconfigured,
    /// Configured target does not match any registered provider adapter.
    UnknownTarget {
        /// Unresolvable target string.
        target: String,
    },
    /// Target resolved, but provider credentials are missing.
    MissingCredential {
        /// Provider ID (e.g., `"anthropic"` or `"openai"`).
        provider: String,
        /// Target model string.
        target: String,
    },
}

/// Filter mode for `/tree` DAG navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TreeFilterMode {
    /// Standard view: turns, model responses, tool calls, compactions, branch heads.
    #[default]
    Default,
    /// Only user prompt (`turn_start`) nodes.
    UserOnly,
    /// Hide `tool` effects and results.
    NoTools,
    /// All `SessionTree` nodes including `session_open` and `phase_*`.
    All,
}

impl TreeFilterMode {
    /// Cycle to the next filter mode.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Default => Self::UserOnly,
            Self::UserOnly => Self::NoTools,
            Self::NoTools => Self::All,
            Self::All => Self::Default,
        }
    }

    /// Short human-readable label for the modal header.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::UserOnly => "User Only",
            Self::NoTools => "No Tools",
            Self::All => "All Nodes",
        }
    }
}

/// Flattened `SessionTree` node entry for the `/tree` selector modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntryItem {
    /// Canonical node ID.
    pub node_id: String,
    /// Canonical or `custom:<ns>` node kind.
    pub kind: String,
    /// Associated Turn ID, if any.
    pub turn_id: Option<String>,
    /// Concise single-line summary.
    pub label: String,
    /// ASCII branch connector prefix (e.g., `"|- "`, `"|  \- "`).
    pub prefix_connectors: String,
    /// `true` if this node lies on the root-to-active-head branch.
    pub on_active_path: bool,
    /// `true` if this node is the current active head.
    pub is_head: bool,
    /// `true` for `turn_start` nodes.
    pub is_user: bool,
    /// `true` for `tool` effects and results.
    pub is_tool: bool,
    /// `true` for bookkeeping nodes (`session_open`, `phase_open`, `phase_seal`, `steering`).
    pub is_settings: bool,
}

/// Metadata for a saved `.gestalt/sessions/*.jsonl` file shown in `/session`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFileItem {
    /// Full path to the session JSONL file.
    pub path: PathBuf,
    /// File name without directory prefix.
    pub filename: String,
    /// Last modification timestamp in seconds since Unix epoch.
    pub modified_secs: u64,
    /// Number of `turn_start` nodes in the file.
    pub turn_count: usize,
    /// Total node count in the file.
    pub node_count: usize,
    /// First user prompt preview (or `"(empty session)"`).
    pub preview: String,
}

/// Kind of list selection modal opened by `/model` or `/thinking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectorKind {
    /// `/model` target picker.
    Model,
    /// `/thinking` budget picker.
    Thinking,
}

/// Single entry rendered inside the primary scrollable transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptItem {
    /// User prompt message.
    UserPrompt {
        /// Prompt text.
        text: String,
    },
    /// Streamed assistant text block.
    AssistantText {
        /// Correlated `effect_id` for delta coalescing.
        effect_id: String,
        /// Accumulated UTF-8 text.
        text: String,
    },
    /// Collapsible normalized reasoning (`"thought"` stream channel).
    Reasoning {
        /// Correlated `effect_id` for delta coalescing.
        effect_id: String,
        /// Accumulated reasoning text.
        text: String,
        /// Whether this individual reasoning block is collapsed.
        collapsed: bool,
    },
    /// Collapsible tool output stream (`"tool_output"` stream channel).
    ToolOutput {
        /// Correlated `effect_id`.
        effect_id: String,
        /// Accumulated output text.
        text: String,
        /// Whether this output card is collapsed.
        collapsed: bool,
    },
    /// Structured inline unified diff preview (`ToolUi { renderer: "diff_preview" }`).
    DiffPreview {
        /// Workspace-relative file path.
        path: String,
        /// Added line count.
        additions: u64,
        /// Deleted line count.
        deletions: u64,
        /// Unified diff body.
        unified_diff: String,
        /// Whether the diff body is collapsed.
        collapsed: bool,
    },
    /// Structured System One triage card (`ToolUi { renderer: "decision_route" }`).
    DecisionRoute {
        /// Structured route payload.
        data: Value,
    },
    /// Generic extension-defined `UISurface::renderers` card.
    CustomToolUi {
        /// Registered renderer ID.
        renderer: String,
        /// Display title from `UIRendererSpec` (or `renderer`).
        title: String,
        /// Structured renderer payload.
        data: Value,
    },
    /// Phase boundary divider (`PhaseSwitched`).
    PhaseDivider {
        /// Newly opened Phase ID.
        phase_id: String,
        /// Optional preset ID.
        preset_id: Option<String>,
        /// Active base loop ID.
        base_loop_id: String,
    },
    /// Policy denial notice (`PolicyDenied`).
    PolicyDenied {
        /// Denied `effect_id`.
        effect_id: String,
        /// Denial reason.
        reason: String,
    },
    /// Informational operator notice (e.g., `/rewind` disk-unchanged notice).
    Notice {
        /// Notice text.
        text: String,
    },
    /// Turn header recording user prompt and turn index.
    TurnHeader {
        /// Turn identifier.
        turn_id: String,
        /// 1-based Turn number in this session branch.
        turn_number: usize,
        /// Prompt text submitted by the user or trigger.
        prompt: String,
    },
    /// Collapsible Step card for an `Effect::Tool` invocation and its result.
    StepToolCall {
        /// Correlated kernel `effect_id`.
        effect_id: String,
        /// 1-based Step index within the Turn.
        step_index: usize,
        /// Registered tool name.
        tool_name: String,
        /// Tool call arguments JSON.
        args: Value,
        /// Current execution status.
        status: ToolStepStatus,
        /// Live streaming output (`"tool_output"` channel).
        streaming_output: String,
        /// Final `StepResult` output JSON.
        result_output: Option<Value>,
        /// Optional structured diff preview coalesced from `ToolUi`.
        diff_preview: Option<DiffPreviewData>,
        /// `true` if the tool returned an error or was denied/aborted.
        is_error: bool,
        /// Optional `.gestalt/blobs/<sha256>` spill digest.
        blob_digest: Option<String>,
        /// Whether the Step body is collapsed/minimized.
        collapsed: bool,
    },
    /// Collapsible Step card for `Effect::Save`, `Effect::Spawn`, or `ModelKind::Decision`.
    StepControl {
        /// Correlated kernel `effect_id`.
        effect_id: String,
        /// 1-based Step index within the Turn.
        step_index: usize,
        /// Control effect label (`"save:stage"`, `"spawn"`, `"decision"`, etc.).
        kind: String,
        /// Short target/artifact summary.
        summary: String,
        /// Current execution status.
        status: ToolStepStatus,
        /// Optional completion detail or denial reason.
        detail: Option<String>,
        /// Whether the detail body is collapsed.
        collapsed: bool,
    },
    /// Collapsible context compaction summary (`custom:std:compaction`).
    CompactionSummary {
        /// Compaction node ID.
        node_id: String,
        /// Generated summary text.
        summary: String,
        /// Number of older nodes compacted.
        compacted_count: u64,
        /// Whether the summary body is collapsed.
        collapsed: bool,
    },
    /// Mid-run `Steering` signal note.
    SteeringNote {
        /// Steering kind (`Steer`, `UrgentSteer`, `FollowUp`, `Abort`).
        kind: SteeringKind,
        /// Signal text.
        text: String,
    },
    /// Trigger activation notice (`RunEvent::TriggerFired` / `custom:trigger:fired`).
    TriggerNote {
        /// Trigger instance ID.
        trigger_id: String,
        /// Trigger kind (`interval`, `cron_schedule`, `fs_watch`, `webhook`).
        kind: String,
        /// `true` if queued as `FollowUp` behind an active Turn.
        queued_as_follow_up: bool,
        /// Trigger prompt summary.
        prompt: String,
    },
    /// Turn completion footer summarizing steps, tokens, and cost.
    TurnFooter {
        /// Completed Turn ID.
        turn_id: String,
        /// Terminal Turn status.
        status: TurnStatus,
        /// Number of Steps executed in this Turn.
        steps: usize,
        /// Input tokens consumed in this Turn.
        input_tokens: u64,
        /// Output tokens produced in this Turn.
        output_tokens: u64,
        /// Micro-USD cost in this Turn.
        cost_micro_usd: u64,
    },
    /// Prominent error card with optional remediation hint.
    ErrorNotice {
        /// Short error title.
        title: String,
        /// Detailed error message.
        message: String,
        /// Optional actionable remediation hint.
        hint: Option<String>,
    },
}

/// Modal overlay currently capturing keyboard & mouse focus.
#[derive(Debug, Clone, PartialEq)]
pub enum ActiveOverlay {
    /// Interactive `Effect::Pause` dialog (`Approval`, `Input`, `Checkpoint`).
    PauseDialog {
        /// Unique pause identifier.
        pause_id: String,
        /// Correlated `effect_id`.
        effect_id: String,
        /// Semantic pause reason.
        reason: PauseReason,
        /// Dialog title.
        title: String,
        /// Raw pause payload.
        payload: Value,
        /// User input buffer (used for `PauseReason::Input`).
        input_buf: String,
        /// When `true`, mask `input_buf` with `*` characters (e.g., `provider_login`).
        secret_masked: bool,
        /// Focused button (`0` = primary approve/submit, `1` = secondary deny/cancel).
        selected_btn: usize,
    },
    /// Tabbed inspector modal displaying `UISurface::panels` (`Ctrl+P`).
    InspectorPanel {
        /// Currently selected panel tab index.
        active_panel_idx: usize,
        /// Vertical scroll inside the inspector panel.
        scroll: usize,
    },
    /// Interactive `SessionTree` DAG browser (`/tree` or double-`Esc`).
    TreeSelector {
        /// Flattened tree entries in display order.
        entries: Vec<TreeEntryItem>,
        /// Current node visibility filter.
        filter_mode: TreeFilterMode,
        /// Incremental substring search query.
        search: String,
        /// Selected index within the filtered entry list.
        selected_idx: usize,
        /// Vertical scroll offset inside the tree list.
        scroll: usize,
    },
    /// Interactive saved session picker (`/session` or `/resume`).
    SessionSelector {
        /// Available session files sorted newest-first.
        sessions: Vec<SessionFileItem>,
        /// Currently active session path.
        current_path: PathBuf,
        /// Substring search query.
        search: String,
        /// Selected index within filtered sessions.
        selected_idx: usize,
    },
    /// Interactive list selection modal (`/model`, `/thinking`).
    SelectorModal {
        /// Modal title.
        title: String,
        /// Selector kind.
        kind: SelectorKind,
        /// Filterable list state.
        list: SelectListState,
        /// Current search filter text.
        filter: String,
    },
}

/// Action emitted by [`TuiAppState::handle_key_event`] or [`TuiAppState::handle_mouse_event`].
#[derive(Debug, Clone, PartialEq)]
pub enum TuiEffect {
    /// No external action needed.
    None,
    /// User submitted a prompt or slash command from the editor.
    SubmitInput(String),
    /// User pressed `Ctrl+C` while a Turn is running.
    AbortTurn,
    /// User requested to exit the TUI.
    Quit,
    /// User resolved an active `PauseDialog`.
    ResolvePause {
        /// Correlated pause ID.
        pause_id: String,
        /// Resolution JSON payload.
        resolution: Value,
    },
    /// User denied/cancelled an active `PauseDialog`.
    DenyPause {
        /// Correlated pause ID.
        pause_id: String,
        /// Cancellation reason.
        reason: String,
    },
    /// User clicked a `UISurface::actions` chip or pressed its shortcut.
    TriggerAction(String),
    /// User triggered double-`Esc` on an empty editor to open `/tree`.
    OpenTreeSelector,
    /// User selected a `SessionTree` node to `/rewind` to.
    RewindToNode(String),
    /// User selected a `SessionTree` node to `/fork` from.
    ForkFromNode(String),
    /// User selected a session JSONL file to switch to.
    SwitchSession(PathBuf),
}

/// Exact terminal coordinates recorded during the most recent `render_tui` pass
/// so mouse hit-testing always matches what the user sees.
#[derive(Debug, Clone, Default)]
pub struct TuiLayoutRects {
    /// Primary transcript viewport `Rect` (inner content area).
    pub transcript_inner: Rect,
    /// Clickable `"Jump to bottom"` pill `Rect` when scrolled up.
    pub jump_to_bottom_btn: Option<Rect>,
    /// Virtual `(start_line, end_line, item_idx)` spans inside the transcript.
    pub transcript_line_spans: Vec<(usize, usize, usize)>,
    /// Autocomplete popup `Rect`.
    pub autocomplete_popup: Option<Rect>,
    /// Autocomplete row hit boxes `(filtered_idx, Rect)`.
    pub autocomplete_rows: Vec<(usize, Rect)>,
    /// Dock action chip hit boxes `(action_id, command_or_signal, Rect)`.
    pub action_chips: Vec<(String, String, Rect)>,
    /// Editor box `Rect`.
    pub editor: Rect,
    /// Active modal overlay outer `Rect`.
    pub overlay_box: Option<Rect>,
    /// Inspector panel tab hit boxes `(panel_idx, Rect)`.
    pub overlay_tabs: Vec<(usize, Rect)>,
    /// Modal list row hit boxes `(filtered_idx, Rect)`.
    pub overlay_list_rows: Vec<(usize, Rect)>,
    /// Tree selector `[ Fork Here ]` button `Rect`.
    pub overlay_fork_btn: Option<Rect>,
    /// Pause dialog primary button `Rect`.
    pub overlay_primary_btn: Option<Rect>,
    /// Pause dialog secondary button `Rect`.
    pub overlay_secondary_btn: Option<Rect>,
}

/// Pure, `TestBackend`-testable state for the Gestalt TUI surface.
#[derive(Debug, Clone)]
pub struct TuiAppState {
    /// Workspace label displayed in footer.
    pub workspace_label: String,
    /// Active preset or base loop ID.
    pub active_preset: String,
    /// Active model override (`LoopConfig::params["model"]`), if set.
    pub active_model: Option<String>,
    /// Active extended thinking token budget (`LoopConfig::params["reasoning_budget_tokens"]`).
    pub reasoning_budget_tokens: Option<u64>,
    /// Pre-flight model & credential status.
    pub model_status: ModelConfigStatus,
    /// Optional live status message shown in the loader bar (e.g., retry countdown/notice).
    pub active_status_message: Option<String>,
    /// Default collapsed state for newly created Step/Tool cards (`Ctrl+O` toggles).
    pub tools_collapsed_default: bool,
    /// Node IDs already ingested into `items` to keep live & post-turn sync idempotent.
    pub seen_node_ids: BTreeSet<String>,
    /// Number of Turns observed in the active branch.
    pub turn_count: usize,
    /// Number of Steps observed in the current/latest Turn.
    pub current_turn_step_count: usize,
    /// Timestamp (ms) of the last `Esc` press on an empty idle editor (for double-`Esc` `/tree`).
    pub last_escape_ms: Option<u64>,
    /// Live `BudgetSnapshot` for footer & `budget_meter` panel.
    pub budget: BudgetSnapshot,
    /// Active `PolicyGate` rule IDs & allowed tools summary for `policy_gate` panel.
    pub policy_summary: Value,
    /// Active/paused trigger snapshots for footer badge & `triggers_inspector` panel.
    pub active_triggers: Vec<TriggerStatus>,
    /// Ordered transcript entries.
    pub items: Vec<TranscriptItem>,
    /// Vertical scroll offset in lines from the top of the virtual transcript.
    pub scroll_offset: usize,
    /// Maximum valid `scroll_offset` computed during the last render.
    pub max_scroll: usize,
    /// When `true` (`follow: end`), new output pins the viewport to `max_scroll`.
    pub auto_follow: bool,
    /// Global toggle for displaying `Reasoning` blocks (`Ctrl+R`).
    pub show_reasoning: bool,
    /// Queued steering/follow-up messages shown in the dock's `pending_messages` banner.
    pub queued_steering: Vec<String>,
    /// Whether a Turn is currently executing.
    pub is_running: bool,
    /// Spinner animation frame counter.
    pub spinner_tick: usize,
    /// Current editor input text.
    pub editor_text: String,
    /// Byte cursor position in `editor_text`.
    pub cursor_pos: usize,
    /// Submitted input history.
    pub history: Vec<String>,
    /// Current history navigation index.
    pub history_idx: Option<usize>,
    /// Inline `/` slash-command autocomplete state.
    pub autocomplete: Option<SelectListState>,
    /// Available command items for `/` autocomplete.
    pub command_items: Vec<SelectListItem>,
    /// Registered `UISurface::renderers`.
    pub ui_renderers: BTreeMap<String, UIRendererSpec>,
    /// Registered `UISurface::panels` (ordered for `Ctrl+P` tabs).
    pub ui_panels: Vec<UIPanelSpec>,
    /// Registered `UISurface::actions`.
    pub ui_actions: Vec<UIActionSpec>,
    /// Active modal overlay (`PauseDialog` or `InspectorPanel`).
    pub overlay: Option<ActiveOverlay>,
    /// Layout rectangles from the most recent frame.
    pub layout_rects: TuiLayoutRects,
}

impl TuiAppState {
    /// Construct a new [`TuiAppState`] initialized with `workspace_label` and `active_preset`.
    #[must_use]
    pub fn new(workspace_label: impl Into<String>, active_preset: impl Into<String>) -> Self {
        Self {
            workspace_label: workspace_label.into(),
            active_preset: active_preset.into(),
            active_model: None,
            reasoning_budget_tokens: None,
            model_status: ModelConfigStatus::Unconfigured,
            active_status_message: None,
            tools_collapsed_default: false,
            seen_node_ids: BTreeSet::new(),
            turn_count: 0,
            current_turn_step_count: 0,
            last_escape_ms: None,
            budget: BudgetSnapshot::default(),
            policy_summary: Value::Null,
            active_triggers: Vec::new(),
            items: Vec::new(),
            scroll_offset: 0,
            max_scroll: 0,
            auto_follow: true,
            show_reasoning: true,
            queued_steering: Vec::new(),
            is_running: false,
            spinner_tick: 0,
            editor_text: String::new(),
            cursor_pos: 0,
            history: Vec::new(),
            history_idx: None,
            autocomplete: None,
            command_items: default_kernel_command_items(),
            ui_renderers: BTreeMap::new(),
            ui_panels: Vec::new(),
            ui_actions: Vec::new(),
            overlay: None,
            layout_rects: TuiLayoutRects::default(),
        }
    }

    /// Synchronize commands, `UISurface` specs, and budget from [`ExtensionRegistry`].
    pub fn sync_from_registry(
        &mut self,
        registry: &ExtensionRegistry,
        budget: BudgetSnapshot,
        policy_summary: Value,
    ) {
        self.budget = budget;
        self.policy_summary = policy_summary;
        if let Some(preset) = registry.presets.get(&self.active_preset) {
            self.active_model = preset
                .config
                .params
                .get("model")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            self.reasoning_budget_tokens = preset
                .config
                .params
                .get("reasoning_budget_tokens")
                .and_then(Value::as_u64);
        }
        self.ui_renderers = registry.ui.renderers.clone();
        self.ui_panels = registry.ui.panels.values().cloned().collect();
        self.ui_actions = registry.ui.actions.values().cloned().collect();

        let mut cmds = default_kernel_command_items();
        for spec in registry.commands.values() {
            let val = format!("/{}", spec.name);
            if !cmds.iter().any(|c| c.value == val) {
                cmds.push(SelectListItem {
                    value: val.clone(),
                    label: spec.usage.clone(),
                    description: spec.description.clone(),
                });
            }
        }
        self.command_items = cmds;
    }

    /// Clear and re-project the entire transcript from the active branch of `tree`.
    pub fn hydrate_from_tree(&mut self, tree: &SessionTree) {
        self.items.clear();
        self.seen_node_ids.clear();
        self.turn_count = 0;
        self.current_turn_step_count = 0;
        self.active_status_message = None;
        self.auto_follow = true;
        if let Ok(slice) = tree.slice_to(None) {
            for node in slice.nodes() {
                self.ingest_node(node);
            }
        }
    }

    /// Ingest a single canonical [`SessionNode`] idempotently into `items`.
    pub fn ingest_node(&mut self, node: &SessionNode) {
        if !self.seen_node_ids.insert(node.id.clone()) {
            return;
        }
        let Ok(payload) = node.decode_payload() else {
            return;
        };
        match payload {
            NodePayload::TurnStart {
                turn_id, prompt, ..
            } => {
                self.turn_count += 1;
                self.current_turn_step_count = 0;
                self.active_status_message = None;
                let prompt_text = match &prompt {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                if !prompt_text.trim().is_empty()
                    && self.history.last().is_none_or(|h| h != &prompt_text)
                {
                    self.history.push(prompt_text.clone());
                }
                self.items.push(TranscriptItem::TurnHeader {
                    turn_id,
                    turn_number: self.turn_count,
                    prompt: prompt_text,
                });
            }
            NodePayload::PhaseOpen {
                phase_id,
                preset_id,
                base_loop_id,
                ..
            } => {
                if let Some(preset) = preset_id.as_ref() {
                    self.active_preset = preset.clone();
                }
                self.items.push(TranscriptItem::PhaseDivider {
                    phase_id,
                    preset_id,
                    base_loop_id,
                });
            }
            NodePayload::Effect { envelope, .. } => match envelope.effect {
                Effect::Tool { name, args, .. } => {
                    self.current_turn_step_count += 1;
                    self.items.push(TranscriptItem::StepToolCall {
                        effect_id: envelope.effect_id,
                        step_index: self.current_turn_step_count,
                        tool_name: name,
                        args,
                        status: ToolStepStatus::Running,
                        streaming_output: String::new(),
                        result_output: None,
                        diff_preview: None,
                        is_error: false,
                        blob_digest: None,
                        collapsed: self.tools_collapsed_default,
                    });
                }
                Effect::Save { artifact, mode, .. } => {
                    self.current_turn_step_count += 1;
                    self.items.push(TranscriptItem::StepControl {
                        effect_id: envelope.effect_id,
                        step_index: self.current_turn_step_count,
                        kind: format!("save:{mode:?}").to_ascii_lowercase(),
                        summary: artifact,
                        status: ToolStepStatus::Running,
                        detail: None,
                        collapsed: self.tools_collapsed_default,
                    });
                }
                Effect::Spawn {
                    loop_id,
                    workspace_mode,
                    ..
                } => {
                    self.current_turn_step_count += 1;
                    self.items.push(TranscriptItem::StepControl {
                        effect_id: envelope.effect_id,
                        step_index: self.current_turn_step_count,
                        kind: "spawn".to_string(),
                        summary: format!("{loop_id} ({workspace_mode:?})"),
                        status: ToolStepStatus::Running,
                        detail: None,
                        collapsed: self.tools_collapsed_default,
                    });
                }
                Effect::Model {
                    kind: ModelKind::Decision,
                    target,
                    ..
                } => {
                    self.current_turn_step_count += 1;
                    self.items.push(TranscriptItem::StepControl {
                        effect_id: envelope.effect_id,
                        step_index: self.current_turn_step_count,
                        kind: "decision".to_string(),
                        summary: target,
                        status: ToolStepStatus::Running,
                        detail: None,
                        collapsed: self.tools_collapsed_default,
                    });
                }
                Effect::Model { .. } => {
                    self.current_turn_step_count += 1;
                }
                Effect::Switch { .. } | Effect::Pause { .. } => {}
            },
            NodePayload::StepResult(res) => self.ingest_step_result(res),
            NodePayload::Steering { kind, content, .. } => {
                let text = match &content {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                self.items.push(TranscriptItem::SteeringNote { kind, text });
            }
            NodePayload::Custom { namespace, data } if namespace == "std:compaction" => {
                let summary = data
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("(compacted)")
                    .to_string();
                let compacted_count = data
                    .get("compacted_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                self.items.push(TranscriptItem::CompactionSummary {
                    node_id: node.id.clone(),
                    summary,
                    compacted_count,
                    collapsed: true,
                });
            }
            NodePayload::Custom { namespace, data } if namespace == "trigger:fired" => {
                let trigger_id = data
                    .get("trigger_id")
                    .and_then(Value::as_str)
                    .unwrap_or("trigger")
                    .to_string();
                let kind = data
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("custom")
                    .to_string();
                let queued_as_follow_up = data
                    .get("queued_as_follow_up")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let prompt = data
                    .get("prompt")
                    .map(|p| match p {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                self.items.push(TranscriptItem::TriggerNote {
                    trigger_id,
                    kind,
                    queued_as_follow_up,
                    prompt,
                });
            }
            NodePayload::TurnEnd {
                turn_id,
                status,
                output,
                usage,
            } => {
                self.active_status_message = None;
                if let Some(err_msg) = output.get("error").and_then(Value::as_str) {
                    self.push_error_once("Turn Failed", err_msg);
                }
                self.items.push(TranscriptItem::TurnFooter {
                    turn_id,
                    status,
                    steps: self.current_turn_step_count,
                    input_tokens: usage.input_units,
                    output_tokens: usage.output_units,
                    cost_micro_usd: usage.cost_micro_usd,
                });
            }
            _ => {}
        }
    }

    fn ingest_step_result(&mut self, res: StepResult) {
        match res {
            StepResult::Model {
                effect_id,
                kind: ModelKind::Llm,
                output,
                ..
            } => {
                self.active_status_message = None;
                let has_streamed_text = self.items.iter().any(|item| {
                    matches!(
                        item,
                        TranscriptItem::AssistantText { effect_id: eid, .. }
                            | TranscriptItem::Reasoning { effect_id: eid, .. }
                            if eid == &effect_id
                    )
                });
                if !has_streamed_text {
                    if let Some(arr) = output.get("reasoning").and_then(Value::as_array) {
                        let thoughts: Vec<&str> = arr
                            .iter()
                            .filter_map(|t| t.get("text").and_then(Value::as_str))
                            .filter(|s| !s.trim().is_empty())
                            .collect();
                        if !thoughts.is_empty() {
                            self.items.push(TranscriptItem::Reasoning {
                                effect_id: effect_id.clone(),
                                text: thoughts.join("\n\n"),
                                collapsed: true,
                            });
                        }
                    }
                    if let Some(txt) = output.get("text").and_then(Value::as_str)
                        && !txt.trim().is_empty()
                    {
                        self.items.push(TranscriptItem::AssistantText {
                            effect_id,
                            text: txt.to_string(),
                        });
                    }
                }
                if output.get("stop_reason").and_then(Value::as_str) == Some("length") {
                    self.items.push(TranscriptItem::Notice {
                        text: "Response was truncated at the model output token limit.".to_string(),
                    });
                }
            }
            StepResult::Model {
                effect_id, output, ..
            } => {
                self.update_step_control(
                    &effect_id,
                    ToolStepStatus::Completed,
                    Some(output.to_string()),
                );
            }
            StepResult::Tool {
                effect_id,
                output,
                is_error,
                blob_digest,
            } => {
                for item in self.items.iter_mut().rev() {
                    if let TranscriptItem::StepToolCall {
                        effect_id: eid,
                        status,
                        result_output,
                        is_error: err_flag,
                        blob_digest: bd,
                        ..
                    } = item
                        && eid == &effect_id
                    {
                        *status = if is_error {
                            ToolStepStatus::Error
                        } else {
                            ToolStepStatus::Completed
                        };
                        *result_output = Some(output);
                        *err_flag = is_error;
                        *bd = blob_digest;
                        break;
                    }
                }
            }
            StepResult::Saved {
                effect_id,
                stage_id,
                diff_summary,
                ..
            } => {
                self.update_step_control(
                    &effect_id,
                    ToolStepStatus::Completed,
                    Some(format!("stage={stage_id} ({diff_summary})")),
                );
            }
            StepResult::SpawnCompleted {
                effect_id,
                child_head_id,
                ..
            } => {
                self.update_step_control(
                    &effect_id,
                    ToolStepStatus::Completed,
                    Some(format!("child_head={child_head_id}")),
                );
            }
            StepResult::Denied { effect_id, reason } => {
                self.mark_step_terminated(&effect_id, ToolStepStatus::Denied, &reason);
            }
            StepResult::Aborted { effect_id, reason } => {
                self.mark_step_terminated(&effect_id, ToolStepStatus::Aborted, &reason);
            }
            StepResult::Error {
                effect_id,
                message,
                retryable,
            } => {
                self.mark_step_terminated(&effect_id, ToolStepStatus::Error, &message);
                if retryable {
                    self.active_status_message =
                        Some(format!("Retrying after transient error: {message}"));
                } else {
                    self.push_error_once("Provider / Execution Error", &message);
                }
            }
            StepResult::PauseResolved { .. } | StepResult::Switched { .. } => {}
        }
    }

    fn update_step_control(
        &mut self,
        effect_id: &str,
        new_status: ToolStepStatus,
        new_detail: Option<String>,
    ) {
        for item in self.items.iter_mut().rev() {
            if let TranscriptItem::StepControl {
                effect_id: eid,
                status,
                detail,
                ..
            } = item
                && eid == effect_id
            {
                *status = new_status;
                *detail = new_detail;
                break;
            }
        }
    }

    fn mark_step_terminated(&mut self, effect_id: &str, new_status: ToolStepStatus, reason: &str) {
        for item in self.items.iter_mut().rev() {
            match item {
                TranscriptItem::StepToolCall {
                    effect_id: eid,
                    status,
                    result_output,
                    is_error,
                    ..
                } if eid == effect_id => {
                    *status = new_status;
                    *result_output = Some(Value::String(reason.to_string()));
                    *is_error = true;
                    return;
                }
                TranscriptItem::StepControl {
                    effect_id: eid,
                    status,
                    detail,
                    ..
                } if eid == effect_id => {
                    *status = new_status;
                    *detail = Some(reason.to_string());
                    return;
                }
                _ => {}
            }
        }
    }

    fn push_error_once(&mut self, title: &str, message: &str) {
        let already = self.items.iter().rev().take(4).any(
            |item| matches!(item, TranscriptItem::ErrorNotice { message: m, .. } if m == message),
        );
        if !already {
            self.items.push(TranscriptItem::ErrorNotice {
                title: title.to_string(),
                message: message.to_string(),
                hint: classify_error_hint(message),
            });
        }
    }

    /// Toggle collapse/expand across all Step/Tool, Diff, and Compaction cards (`Ctrl+O`).
    pub fn toggle_all_steps_collapsed(&mut self) {
        self.tools_collapsed_default = !self.tools_collapsed_default;
        let target = self.tools_collapsed_default;
        for item in &mut self.items {
            match item {
                TranscriptItem::StepToolCall { collapsed, .. }
                | TranscriptItem::StepControl { collapsed, .. }
                | TranscriptItem::ToolOutput { collapsed, .. }
                | TranscriptItem::DiffPreview { collapsed, .. }
                | TranscriptItem::CompactionSummary { collapsed, .. } => {
                    *collapsed = target;
                }
                _ => {}
            }
        }
    }

    /// Open the `/tree` DAG selector overlay from `tree`.
    pub fn open_tree_selector(&mut self, tree: &SessionTree) {
        let entries = build_tree_entries(tree);
        let visible = filtered_tree_indices(&entries, TreeFilterMode::Default, "");
        let selected_idx = visible
            .iter()
            .position(|&idx| entries[idx].is_head)
            .unwrap_or_else(|| visible.len().saturating_sub(1));
        self.overlay = Some(ActiveOverlay::TreeSelector {
            entries,
            filter_mode: TreeFilterMode::Default,
            search: String::new(),
            selected_idx,
            scroll: 0,
        });
    }

    /// Open the `/session` selector overlay with discovered session files.
    pub fn open_session_selector(&mut self, sessions: Vec<SessionFileItem>, current_path: PathBuf) {
        let selected_idx = sessions
            .iter()
            .position(|s| s.path == current_path)
            .unwrap_or(0);
        self.overlay = Some(ActiveOverlay::SessionSelector {
            sessions,
            current_path,
            search: String::new(),
            selected_idx,
        });
    }

    /// Open a filterable list selector modal (`/model` or `/thinking`).
    pub fn open_selector_modal(
        &mut self,
        title: impl Into<String>,
        kind: SelectorKind,
        items: Vec<SelectListItem>,
    ) {
        self.overlay = Some(ActiveOverlay::SelectorModal {
            title: title.into(),
            kind,
            list: SelectListState::new(items, 8),
            filter: String::new(),
        });
    }

    /// Apply a canonical or transient [`RunEvent`] to the transcript and state.
    pub fn apply_run_event(&mut self, event: &RunEvent) {
        match event {
            RunEvent::StreamDelta {
                effect_id,
                channel,
                payload,
                ..
            } => {
                let Some(text) = extract_stream_text(payload) else {
                    return;
                };
                match channel.as_str() {
                    "text" => {
                        if let Some(TranscriptItem::AssistantText {
                            effect_id: last_id,
                            text: buf,
                        }) = self.items.last_mut()
                            && last_id == effect_id
                        {
                            buf.push_str(text);
                            return;
                        }
                        self.items.push(TranscriptItem::AssistantText {
                            effect_id: effect_id.clone(),
                            text: text.to_string(),
                        });
                    }
                    "thought" => {
                        if let Some(TranscriptItem::Reasoning {
                            effect_id: last_id,
                            text: buf,
                            ..
                        }) = self.items.last_mut()
                            && last_id == effect_id
                        {
                            buf.push_str(text);
                            return;
                        }
                        self.items.push(TranscriptItem::Reasoning {
                            effect_id: effect_id.clone(),
                            text: text.to_string(),
                            collapsed: false,
                        });
                    }
                    "tool_output" => {
                        for item in self.items.iter_mut().rev() {
                            if let TranscriptItem::StepToolCall {
                                effect_id: eid,
                                streaming_output,
                                ..
                            } = item
                                && eid == effect_id
                            {
                                streaming_output.push_str(text);
                                return;
                            }
                        }
                        if let Some(TranscriptItem::ToolOutput {
                            effect_id: last_id,
                            text: buf,
                            ..
                        }) = self.items.last_mut()
                            && last_id == effect_id
                        {
                            buf.push_str(text);
                            return;
                        }
                        self.items.push(TranscriptItem::ToolOutput {
                            effect_id: effect_id.clone(),
                            text: text.to_string(),
                            collapsed: true,
                        });
                    }
                    _ => {}
                }
            }
            RunEvent::ToolUi {
                effect_id,
                renderer,
                data,
                ..
            } => match renderer.as_str() {
                "diff_preview" => {
                    let path = data
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();
                    let additions = data.get("additions").and_then(Value::as_u64).unwrap_or(0);
                    let deletions = data.get("deletions").and_then(Value::as_u64).unwrap_or(0);
                    let unified_diff = data
                        .get("unified_diff")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    for item in self.items.iter_mut().rev() {
                        if let TranscriptItem::StepToolCall {
                            effect_id: eid,
                            diff_preview,
                            ..
                        } = item
                            && eid == effect_id
                        {
                            *diff_preview = Some(DiffPreviewData {
                                path,
                                additions,
                                deletions,
                                unified_diff,
                            });
                            return;
                        }
                    }
                    self.items.push(TranscriptItem::DiffPreview {
                        path,
                        additions,
                        deletions,
                        unified_diff,
                        collapsed: false,
                    });
                }
                "decision_route" => {
                    self.items
                        .push(TranscriptItem::DecisionRoute { data: data.clone() });
                }
                other => {
                    let title = self
                        .ui_renderers
                        .get(other)
                        .map_or_else(|| other.to_string(), |spec| spec.title.clone());
                    self.items.push(TranscriptItem::CustomToolUi {
                        renderer: other.to_string(),
                        title,
                        data: data.clone(),
                    });
                }
            },
            RunEvent::PhaseSwitched {
                phase_id,
                preset_id,
                base_loop_id,
            } => {
                if let Some(preset) = preset_id {
                    self.active_preset = preset.clone();
                }
                self.items.push(TranscriptItem::PhaseDivider {
                    phase_id: phase_id.clone(),
                    preset_id: preset_id.clone(),
                    base_loop_id: base_loop_id.clone(),
                });
            }
            RunEvent::PolicyDenied { effect_id, reason } => {
                self.mark_step_terminated(effect_id, ToolStepStatus::Denied, reason);
                self.items.push(TranscriptItem::PolicyDenied {
                    effect_id: effect_id.clone(),
                    reason: reason.clone(),
                });
            }
            RunEvent::TurnCompleted { output, usage, .. } => {
                self.is_running = false;
                self.active_status_message = None;
                self.queued_steering.clear();
                self.budget.steps_used = self.budget.steps_used.saturating_add(1);
                self.budget.tokens_used = self
                    .budget
                    .tokens_used
                    .saturating_add(usage.input_units + usage.output_units);
                self.budget.cost_micro_usd_used = self
                    .budget
                    .cost_micro_usd_used
                    .saturating_add(usage.cost_micro_usd);
                if let Some(err_msg) = output.get("error").and_then(Value::as_str) {
                    self.push_error_once("Turn Failed", err_msg);
                }
            }
            RunEvent::TurnAborted { reason, .. } => {
                self.is_running = false;
                self.active_status_message = None;
                self.queued_steering.clear();
                self.items.push(TranscriptItem::Notice {
                    text: format!("Turn aborted: {reason}"),
                });
            }
            RunEvent::TriggerFired {
                trigger_id,
                kind,
                queued_as_follow_up,
                payload,
                ..
            } => {
                let prompt = payload
                    .get("prompt")
                    .map(|p| match p {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                let already_shown = self.items.last().is_some_and(|item| {
                    matches!(
                        item,
                        TranscriptItem::TriggerNote {
                            trigger_id: tid,
                            prompt: p,
                            ..
                        } if tid == trigger_id && p == &prompt
                    )
                });
                if !already_shown {
                    self.items.push(TranscriptItem::TriggerNote {
                        trigger_id: trigger_id.clone(),
                        kind: kind.clone(),
                        queued_as_follow_up: *queued_as_follow_up,
                        prompt,
                    });
                }
            }
            RunEvent::NodeAppended { .. } | RunEvent::Paused { .. } => {}
        }
    }

    /// Open a modal [`ActiveOverlay::PauseDialog`] for an incoming [`PausePrompt`].
    pub fn open_pause_dialog(&mut self, prompt: PausePrompt) {
        let secret_masked = prompt
            .payload
            .get("secret")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || prompt.payload.get("dialog").and_then(Value::as_str) == Some("provider_login");
        let title = prompt
            .payload
            .get("prompt")
            .or_else(|| prompt.payload.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(match prompt.reason {
                PauseReason::Approval => "Approval Required",
                PauseReason::Input => "Input Required",
                PauseReason::Checkpoint => "Checkpoint",
            })
            .to_string();
        self.overlay = Some(ActiveOverlay::PauseDialog {
            pause_id: prompt.pause_id,
            effect_id: prompt.effect_id,
            reason: prompt.reason,
            title,
            payload: prompt.payload,
            input_buf: String::new(),
            secret_masked,
            selected_btn: 0,
        });
    }

    /// Recompute `/` slash-command autocomplete state from `editor_text`.
    pub fn update_autocomplete(&mut self) {
        if self.editor_text.starts_with('/') && !self.editor_text.contains(' ') {
            let mut list = self
                .autocomplete
                .take()
                .unwrap_or_else(|| SelectListState::new(self.command_items.clone(), 6));
            list.set_filter(&self.editor_text);
            self.autocomplete = if list.is_empty() { None } else { Some(list) };
        } else {
            self.autocomplete = None;
        }
    }

    /// Handle a keyboard event against overlays, autocomplete, shortcuts, and editor.
    pub fn handle_key_event(&mut self, key: KeyEvent) -> TuiEffect {
        // 1. Modal overlays capture keyboard focus first.
        if let Some(overlay) = self.overlay.take() {
            match overlay {
                ActiveOverlay::PauseDialog {
                    pause_id,
                    effect_id,
                    reason,
                    title,
                    payload,
                    mut input_buf,
                    secret_masked,
                    mut selected_btn,
                } => {
                    match key.code {
                        KeyCode::Esc => {
                            return TuiEffect::DenyPause {
                                pause_id,
                                reason: "cancelled by user".to_string(),
                            };
                        }
                        KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                            selected_btn = 1 - selected_btn;
                        }
                        KeyCode::Enter => {
                            if selected_btn == 1 {
                                return TuiEffect::DenyPause {
                                    pause_id,
                                    reason: "denied by user".to_string(),
                                };
                            }
                            let resolution = match reason {
                                PauseReason::Input => json!({ "secret": input_buf }),
                                PauseReason::Approval | PauseReason::Checkpoint => {
                                    json!({ "approved": true })
                                }
                            };
                            return TuiEffect::ResolvePause {
                                pause_id,
                                resolution,
                            };
                        }
                        KeyCode::Char('y') if reason != PauseReason::Input => {
                            return TuiEffect::ResolvePause {
                                pause_id,
                                resolution: json!({ "approved": true }),
                            };
                        }
                        KeyCode::Char('n') if reason != PauseReason::Input => {
                            return TuiEffect::DenyPause {
                                pause_id,
                                reason: "denied by user".to_string(),
                            };
                        }
                        KeyCode::Backspace if reason == PauseReason::Input => {
                            input_buf.pop();
                        }
                        KeyCode::Char(c)
                            if reason == PauseReason::Input
                                && !key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            input_buf.push(c);
                        }
                        _ => {}
                    }
                    self.overlay = Some(ActiveOverlay::PauseDialog {
                        pause_id,
                        effect_id,
                        reason,
                        title,
                        payload,
                        input_buf,
                        secret_masked,
                        selected_btn,
                    });
                    return TuiEffect::None;
                }
                ActiveOverlay::InspectorPanel {
                    mut active_panel_idx,
                    mut scroll,
                } => {
                    let count = self.ui_panels.len().max(1);
                    match key.code {
                        KeyCode::Esc => return TuiEffect::None,
                        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            return TuiEffect::None;
                        }
                        KeyCode::Tab | KeyCode::Right => {
                            active_panel_idx = (active_panel_idx + 1) % count;
                            scroll = 0;
                        }
                        KeyCode::Left => {
                            active_panel_idx = (active_panel_idx + count - 1) % count;
                            scroll = 0;
                        }
                        KeyCode::Up => scroll = scroll.saturating_sub(1),
                        KeyCode::Down => scroll = scroll.saturating_add(1),
                        _ => {}
                    }
                    self.overlay = Some(ActiveOverlay::InspectorPanel {
                        active_panel_idx,
                        scroll,
                    });
                    return TuiEffect::None;
                }
                ActiveOverlay::TreeSelector {
                    entries,
                    mut filter_mode,
                    mut search,
                    mut selected_idx,
                    scroll,
                } => {
                    let visible = filtered_tree_indices(&entries, filter_mode, &search);
                    match key.code {
                        KeyCode::Esc => return TuiEffect::None,
                        KeyCode::Tab => {
                            filter_mode = filter_mode.next();
                            selected_idx = 0;
                        }
                        KeyCode::Up => {
                            if !visible.is_empty() {
                                selected_idx = if selected_idx == 0 {
                                    visible.len() - 1
                                } else {
                                    selected_idx - 1
                                };
                            }
                        }
                        KeyCode::Down => {
                            if !visible.is_empty() {
                                selected_idx = (selected_idx + 1) % visible.len();
                            }
                        }
                        KeyCode::Enter => {
                            if let Some(&raw_idx) = visible.get(selected_idx)
                                && let Some(entry) = entries.get(raw_idx)
                            {
                                return TuiEffect::RewindToNode(entry.node_id.clone());
                            }
                        }
                        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(&raw_idx) = visible.get(selected_idx)
                                && let Some(entry) = entries.get(raw_idx)
                            {
                                return TuiEffect::ForkFromNode(entry.node_id.clone());
                            }
                        }
                        KeyCode::Backspace => {
                            search.pop();
                            selected_idx = 0;
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            search.push(c);
                            selected_idx = 0;
                        }
                        _ => {}
                    }
                    self.overlay = Some(ActiveOverlay::TreeSelector {
                        entries,
                        filter_mode,
                        search,
                        selected_idx,
                        scroll,
                    });
                    return TuiEffect::None;
                }
                ActiveOverlay::SessionSelector {
                    sessions,
                    current_path,
                    mut search,
                    mut selected_idx,
                } => {
                    let visible = filtered_session_indices(&sessions, &search);
                    match key.code {
                        KeyCode::Esc => return TuiEffect::None,
                        KeyCode::Up => {
                            if !visible.is_empty() {
                                selected_idx = if selected_idx == 0 {
                                    visible.len() - 1
                                } else {
                                    selected_idx - 1
                                };
                            }
                        }
                        KeyCode::Down => {
                            if !visible.is_empty() {
                                selected_idx = (selected_idx + 1) % visible.len();
                            }
                        }
                        KeyCode::Enter => {
                            if let Some(&raw_idx) = visible.get(selected_idx)
                                && let Some(item) = sessions.get(raw_idx)
                            {
                                return TuiEffect::SwitchSession(item.path.clone());
                            }
                        }
                        KeyCode::Backspace => {
                            search.pop();
                            selected_idx = 0;
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            search.push(c);
                            selected_idx = 0;
                        }
                        _ => {}
                    }
                    self.overlay = Some(ActiveOverlay::SessionSelector {
                        sessions,
                        current_path,
                        search,
                        selected_idx,
                    });
                    return TuiEffect::None;
                }
                ActiveOverlay::SelectorModal {
                    title,
                    kind,
                    mut list,
                    mut filter,
                } => {
                    match key.code {
                        KeyCode::Esc => return TuiEffect::None,
                        KeyCode::Up => list.move_up(),
                        KeyCode::Down => list.move_down(),
                        KeyCode::Enter => {
                            if let Some(item) = list.selected_item() {
                                return TuiEffect::TriggerAction(item.value.clone());
                            }
                        }
                        KeyCode::Backspace => {
                            filter.pop();
                            list.set_filter(&filter);
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            filter.push(c);
                            list.set_filter(&filter);
                        }
                        _ => {}
                    }
                    self.overlay = Some(ActiveOverlay::SelectorModal {
                        title,
                        kind,
                        list,
                        filter,
                    });
                    return TuiEffect::None;
                }
            }
        }

        // 2. Global shortcuts.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    return if self.is_running {
                        TuiEffect::AbortTurn
                    } else {
                        TuiEffect::Quit
                    };
                }
                KeyCode::Char('q') => return TuiEffect::Quit,
                KeyCode::Char('r') => {
                    self.show_reasoning = !self.show_reasoning;
                    return TuiEffect::None;
                }
                KeyCode::Char('o') => {
                    self.toggle_all_steps_collapsed();
                    return TuiEffect::None;
                }
                KeyCode::Char('p') => {
                    self.overlay = Some(ActiveOverlay::InspectorPanel {
                        active_panel_idx: 0,
                        scroll: 0,
                    });
                    return TuiEffect::None;
                }
                KeyCode::Char('k') => {
                    return TuiEffect::TriggerAction("/compact".to_string());
                }
                _ => {}
            }
        }

        if key.code == KeyCode::Esc
            && self.autocomplete.is_none()
            && self.editor_text.trim().is_empty()
            && !self.is_running
        {
            let now = epoch_now_ms();
            if let Some(prev) = self.last_escape_ms
                && now.saturating_sub(prev) <= 500
            {
                self.last_escape_ms = None;
                return TuiEffect::OpenTreeSelector;
            }
            self.last_escape_ms = Some(now);
            return TuiEffect::None;
        }
        self.last_escape_ms = None;

        // 3. Inline slash-command autocomplete navigation.
        if let Some(ref mut ac) = self.autocomplete {
            match key.code {
                KeyCode::Up => {
                    ac.move_up();
                    return TuiEffect::None;
                }
                KeyCode::Down => {
                    ac.move_down();
                    return TuiEffect::None;
                }
                KeyCode::Tab => {
                    if let Some(item) = ac.selected_item() {
                        self.editor_text = format!("{} ", item.value);
                        self.cursor_pos = self.editor_text.len();
                    }
                    self.autocomplete = None;
                    return TuiEffect::None;
                }
                KeyCode::Esc => {
                    self.autocomplete = None;
                    return TuiEffect::None;
                }
                _ => {}
            }
        }

        // 4. Transcript scrolling & editor input.
        match key.code {
            KeyCode::PageUp => {
                self.auto_follow = false;
                self.scroll_offset = self.scroll_offset.saturating_sub(5);
            }
            KeyCode::PageDown => {
                self.scroll_offset = (self.scroll_offset + 5).min(self.max_scroll);
                self.auto_follow = self.scroll_offset >= self.max_scroll;
            }
            KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.scroll_offset = self.max_scroll;
                self.auto_follow = true;
            }
            KeyCode::Enter => {
                if let Some(ac) = self.autocomplete.take()
                    && let Some(item) = ac.selected_item()
                    && self.editor_text.trim() != item.value
                {
                    self.editor_text = format!("{} ", item.value);
                    self.cursor_pos = self.editor_text.len();
                    return TuiEffect::None;
                }
                let submitted = std::mem::take(&mut self.editor_text);
                self.cursor_pos = 0;
                self.history_idx = None;
                let trimmed = submitted.trim().to_string();
                if trimmed.is_empty() {
                    return TuiEffect::None;
                }
                self.history.push(trimmed.clone());
                return TuiEffect::SubmitInput(trimmed);
            }
            KeyCode::Up => {
                if !self.history.is_empty() {
                    let next_idx = match self.history_idx {
                        None => self.history.len() - 1,
                        Some(i) => i.saturating_sub(1),
                    };
                    self.history_idx = Some(next_idx);
                    self.editor_text = self.history[next_idx].clone();
                    self.cursor_pos = self.editor_text.len();
                    self.autocomplete = None;
                }
            }
            KeyCode::Down => {
                if let Some(idx) = self.history_idx {
                    if idx + 1 < self.history.len() {
                        self.history_idx = Some(idx + 1);
                        self.editor_text = self.history[idx + 1].clone();
                    } else {
                        self.history_idx = None;
                        self.editor_text.clear();
                    }
                    self.cursor_pos = self.editor_text.len();
                    self.autocomplete = None;
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    let mut prev = self.cursor_pos - 1;
                    while !self.editor_text.is_char_boundary(prev) {
                        prev -= 1;
                    }
                    self.cursor_pos = prev;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.editor_text.len() {
                    let mut next = self.cursor_pos + 1;
                    while !self.editor_text.is_char_boundary(next) {
                        next += 1;
                    }
                    self.cursor_pos = next;
                }
            }
            KeyCode::Home => self.cursor_pos = 0,
            KeyCode::End => self.cursor_pos = self.editor_text.len(),
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    let mut prev = self.cursor_pos - 1;
                    while !self.editor_text.is_char_boundary(prev) {
                        prev -= 1;
                    }
                    self.editor_text.drain(prev..self.cursor_pos);
                    self.cursor_pos = prev;
                    self.update_autocomplete();
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.editor_text.len() {
                    let mut next = self.cursor_pos + 1;
                    while !self.editor_text.is_char_boundary(next) {
                        next += 1;
                    }
                    self.editor_text.drain(self.cursor_pos..next);
                    self.update_autocomplete();
                }
            }
            KeyCode::Char(c) => {
                self.editor_text.insert(self.cursor_pos, c);
                self.cursor_pos += c.len_utf8();
                self.update_autocomplete();
            }
            _ => {}
        }

        TuiEffect::None
    }

    /// Handle scroll-offset-aware mouse wheel and click events (`AGENTS.md`).
    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) -> TuiEffect {
        let col = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                match self.overlay.as_mut() {
                    Some(ActiveOverlay::InspectorPanel { scroll, .. }) => {
                        *scroll = scroll.saturating_sub(3);
                        return TuiEffect::None;
                    }
                    Some(ActiveOverlay::TreeSelector { selected_idx, .. }) => {
                        *selected_idx = selected_idx.saturating_sub(1);
                        return TuiEffect::None;
                    }
                    Some(ActiveOverlay::SessionSelector { selected_idx, .. }) => {
                        *selected_idx = selected_idx.saturating_sub(1);
                        return TuiEffect::None;
                    }
                    Some(ActiveOverlay::SelectorModal { list, .. }) => {
                        list.move_up();
                        return TuiEffect::None;
                    }
                    _ => {}
                }
                if let Some(ac_rect) = self.layout_rects.autocomplete_popup
                    && rect_contains(ac_rect, col, row)
                    && let Some(ref mut ac) = self.autocomplete
                {
                    ac.move_up();
                    return TuiEffect::None;
                }
                self.auto_follow = false;
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
                TuiEffect::None
            }
            MouseEventKind::ScrollDown => {
                match self.overlay.as_mut() {
                    Some(ActiveOverlay::InspectorPanel { scroll, .. }) => {
                        *scroll = scroll.saturating_add(3);
                        return TuiEffect::None;
                    }
                    Some(ActiveOverlay::TreeSelector {
                        entries,
                        filter_mode,
                        search,
                        selected_idx,
                        ..
                    }) => {
                        let len = filtered_tree_indices(entries, *filter_mode, search).len();
                        if len > 0 {
                            *selected_idx = (*selected_idx + 1).min(len - 1);
                        }
                        return TuiEffect::None;
                    }
                    Some(ActiveOverlay::SessionSelector {
                        sessions,
                        search,
                        selected_idx,
                        ..
                    }) => {
                        let len = filtered_session_indices(sessions, search).len();
                        if len > 0 {
                            *selected_idx = (*selected_idx + 1).min(len - 1);
                        }
                        return TuiEffect::None;
                    }
                    Some(ActiveOverlay::SelectorModal { list, .. }) => {
                        list.move_down();
                        return TuiEffect::None;
                    }
                    _ => {}
                }
                if let Some(ac_rect) = self.layout_rects.autocomplete_popup
                    && rect_contains(ac_rect, col, row)
                    && let Some(ref mut ac) = self.autocomplete
                {
                    ac.move_down();
                    return TuiEffect::None;
                }
                self.scroll_offset = (self.scroll_offset + 3).min(self.max_scroll);
                self.auto_follow = self.scroll_offset >= self.max_scroll;
                TuiEffect::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // 1. Active overlay hit-testing.
                if let Some(overlay) = self.overlay.take() {
                    match overlay {
                        ActiveOverlay::PauseDialog {
                            pause_id,
                            effect_id,
                            reason,
                            title,
                            payload,
                            input_buf,
                            secret_masked,
                            selected_btn,
                        } => {
                            if let Some(btn) = self.layout_rects.overlay_primary_btn
                                && rect_contains(btn, col, row)
                            {
                                let resolution = match reason {
                                    PauseReason::Input => json!({ "secret": input_buf }),
                                    PauseReason::Approval | PauseReason::Checkpoint => {
                                        json!({ "approved": true })
                                    }
                                };
                                return TuiEffect::ResolvePause {
                                    pause_id,
                                    resolution,
                                };
                            }
                            if let Some(btn) = self.layout_rects.overlay_secondary_btn
                                && rect_contains(btn, col, row)
                            {
                                return TuiEffect::DenyPause {
                                    pause_id,
                                    reason: "denied via mouse click".to_string(),
                                };
                            }
                            self.overlay = Some(ActiveOverlay::PauseDialog {
                                pause_id,
                                effect_id,
                                reason,
                                title,
                                payload,
                                input_buf,
                                secret_masked,
                                selected_btn,
                            });
                            return TuiEffect::None;
                        }
                        ActiveOverlay::InspectorPanel {
                            mut active_panel_idx,
                            mut scroll,
                        } => {
                            for &(idx, tab_rect) in &self.layout_rects.overlay_tabs {
                                if rect_contains(tab_rect, col, row) {
                                    active_panel_idx = idx;
                                    scroll = 0;
                                    self.overlay = Some(ActiveOverlay::InspectorPanel {
                                        active_panel_idx,
                                        scroll,
                                    });
                                    return TuiEffect::None;
                                }
                            }
                            if let Some(box_rect) = self.layout_rects.overlay_box
                                && rect_contains(box_rect, col, row)
                            {
                                self.overlay = Some(ActiveOverlay::InspectorPanel {
                                    active_panel_idx,
                                    scroll,
                                });
                            }
                            return TuiEffect::None;
                        }
                        ActiveOverlay::TreeSelector {
                            entries,
                            filter_mode,
                            search,
                            mut selected_idx,
                            scroll,
                        } => {
                            let visible = filtered_tree_indices(&entries, filter_mode, &search);
                            if let Some(fork_btn) = self.layout_rects.overlay_fork_btn
                                && rect_contains(fork_btn, col, row)
                                && let Some(&raw_idx) = visible.get(selected_idx)
                                && let Some(entry) = entries.get(raw_idx)
                            {
                                return TuiEffect::ForkFromNode(entry.node_id.clone());
                            }
                            for &(vis_idx, row_rect) in &self.layout_rects.overlay_list_rows {
                                if rect_contains(row_rect, col, row) {
                                    selected_idx = vis_idx;
                                    if let Some(&raw_idx) = visible.get(vis_idx)
                                        && let Some(entry) = entries.get(raw_idx)
                                    {
                                        return TuiEffect::RewindToNode(entry.node_id.clone());
                                    }
                                }
                            }
                            if let Some(box_rect) = self.layout_rects.overlay_box
                                && rect_contains(box_rect, col, row)
                            {
                                self.overlay = Some(ActiveOverlay::TreeSelector {
                                    entries,
                                    filter_mode,
                                    search,
                                    selected_idx,
                                    scroll,
                                });
                            }
                            return TuiEffect::None;
                        }
                        ActiveOverlay::SessionSelector {
                            sessions,
                            current_path,
                            search,
                            mut selected_idx,
                        } => {
                            let visible = filtered_session_indices(&sessions, &search);
                            for &(vis_idx, row_rect) in &self.layout_rects.overlay_list_rows {
                                if rect_contains(row_rect, col, row) {
                                    selected_idx = vis_idx;
                                    if let Some(&raw_idx) = visible.get(vis_idx)
                                        && let Some(item) = sessions.get(raw_idx)
                                    {
                                        return TuiEffect::SwitchSession(item.path.clone());
                                    }
                                }
                            }
                            if let Some(box_rect) = self.layout_rects.overlay_box
                                && rect_contains(box_rect, col, row)
                            {
                                self.overlay = Some(ActiveOverlay::SessionSelector {
                                    sessions,
                                    current_path,
                                    search,
                                    selected_idx,
                                });
                            }
                            return TuiEffect::None;
                        }
                        ActiveOverlay::SelectorModal {
                            title,
                            kind,
                            mut list,
                            filter,
                        } => {
                            for &(vis_idx, row_rect) in &self.layout_rects.overlay_list_rows {
                                if rect_contains(row_rect, col, row) {
                                    list.select(vis_idx);
                                    if let Some(item) = list.selected_item() {
                                        return TuiEffect::TriggerAction(item.value.clone());
                                    }
                                }
                            }
                            if let Some(box_rect) = self.layout_rects.overlay_box
                                && rect_contains(box_rect, col, row)
                            {
                                self.overlay = Some(ActiveOverlay::SelectorModal {
                                    title,
                                    kind,
                                    list,
                                    filter,
                                });
                            }
                            return TuiEffect::None;
                        }
                    }
                }

                // 2. Autocomplete popup row click.
                for &(f_idx, row_rect) in &self.layout_rects.autocomplete_rows {
                    if rect_contains(row_rect, col, row)
                        && let Some(ref mut ac) = self.autocomplete
                    {
                        ac.select(f_idx);
                        if let Some(item) = ac.selected_item() {
                            self.editor_text = format!("{} ", item.value);
                            self.cursor_pos = self.editor_text.len();
                        }
                        self.autocomplete = None;
                        return TuiEffect::None;
                    }
                }

                // 3. Scroll-to-end indicator pill click.
                if let Some(pill) = self.layout_rects.jump_to_bottom_btn
                    && rect_contains(pill, col, row)
                {
                    self.scroll_offset = self.max_scroll;
                    self.auto_follow = true;
                    return TuiEffect::None;
                }

                // 4. Dock action chips click.
                for (_, cmd, chip_rect) in &self.layout_rects.action_chips {
                    if rect_contains(*chip_rect, col, row) {
                        return TuiEffect::TriggerAction(cmd.clone());
                    }
                }

                // 5. Scroll-offset-aware transcript click hit-testing (`AGENTS.md`).
                let vp = self.layout_rects.transcript_inner;
                if rect_contains(vp, col, row) {
                    let viewport_row = (row - vp.y) as usize;
                    let virtual_line = viewport_row + self.scroll_offset;
                    if let Some(&(_, _, item_idx)) = self
                        .layout_rects
                        .transcript_line_spans
                        .iter()
                        .find(|&&(start, end, _)| virtual_line >= start && virtual_line < end)
                        && let Some(item) = self.items.get_mut(item_idx)
                    {
                        match item {
                            TranscriptItem::Reasoning { collapsed, .. }
                            | TranscriptItem::ToolOutput { collapsed, .. }
                            | TranscriptItem::DiffPreview { collapsed, .. }
                            | TranscriptItem::StepToolCall { collapsed, .. }
                            | TranscriptItem::StepControl { collapsed, .. }
                            | TranscriptItem::CompactionSummary { collapsed, .. } => {
                                *collapsed = !*collapsed;
                            }
                            _ => {}
                        }
                    }
                }
                TuiEffect::None
            }
            _ => TuiEffect::None,
        }
    }
}

fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

fn epoch_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Derive a concise actionable remediation hint from a provider or runtime error message.
#[must_use]
pub fn classify_error_hint(message: &str) -> Option<String> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("missing credential")
        || lower.contains("[auth_error]")
        || lower.contains("http 401")
        || lower.contains("http 403")
    {
        return Some("Run /login <provider> (e.g. /login anthropic or /login openai) or set your API key env var.".to_string());
    }
    if lower.contains("default_llm") || lower.contains("unable to resolve model target") {
        return Some("Run /model <provider/model> (e.g. /model anthropic/claude-sonnet-4-6) and /login <provider>.".to_string());
    }
    if lower.contains("[context_length_exceeded]") || lower.contains("context window") {
        return Some(
            "Run /compact (Ctrl+K) to summarize older turns, or /new for a fresh session."
                .to_string(),
        );
    }
    if lower.contains("[retryable]") || lower.contains("http 429") || lower.contains("http 529") {
        return Some(
            "Rate limit or transient upstream error; retry or switch models via /model."
                .to_string(),
        );
    }
    None
}

/// Filter `TreeEntryItem` indices according to `filter_mode` and `search` query.
#[must_use]
pub fn filtered_tree_indices(
    entries: &[TreeEntryItem],
    filter_mode: TreeFilterMode,
    search: &str,
) -> Vec<usize> {
    let needle = search.trim().to_ascii_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let passes_mode = match filter_mode {
                TreeFilterMode::Default => !e.is_settings,
                TreeFilterMode::UserOnly => e.is_user,
                TreeFilterMode::NoTools => !e.is_settings && !e.is_tool,
                TreeFilterMode::All => true,
            };
            if !passes_mode {
                return false;
            }
            if needle.is_empty() {
                return true;
            }
            e.label.to_ascii_lowercase().contains(&needle)
                || e.node_id.to_ascii_lowercase().contains(&needle)
                || e.kind.to_ascii_lowercase().contains(&needle)
        })
        .map(|(idx, _)| idx)
        .collect()
}

/// Filter `SessionFileItem` indices by substring match on filename or preview.
#[must_use]
pub fn filtered_session_indices(sessions: &[SessionFileItem], search: &str) -> Vec<usize> {
    let needle = search.trim().to_ascii_lowercase();
    sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            needle.is_empty()
                || s.filename.to_ascii_lowercase().contains(&needle)
                || s.preview.to_ascii_lowercase().contains(&needle)
        })
        .map(|(idx, _)| idx)
        .collect()
}

/// Flatten a [`SessionTree`] DAG into display-ready [`TreeEntryItem`]s with ASCII
/// branch connectors and active-path markers.
#[must_use]
pub fn build_tree_entries(tree: &SessionTree) -> Vec<TreeEntryItem> {
    let nodes = tree.all_nodes();
    if nodes.is_empty() {
        return Vec::new();
    }

    let head_id = tree.head_id();
    let mut active_path_ids = BTreeSet::new();
    if let Ok(slice) = tree.slice_to(None) {
        for n in slice.nodes() {
            active_path_ids.insert(n.id.clone());
        }
    }

    let mut children_map: BTreeMap<Option<String>, Vec<&SessionNode>> = BTreeMap::new();
    for node in nodes {
        children_map
            .entry(node.parent_id.clone())
            .or_default()
            .push(node);
    }

    let mut contains_active: BTreeMap<String, bool> = BTreeMap::new();
    for node in nodes.iter().rev() {
        let mut has = active_path_ids.contains(&node.id);
        if let Some(children) = children_map.get(&Some(node.id.clone())) {
            for child in children {
                if contains_active.get(&child.id).copied().unwrap_or(false) {
                    has = true;
                }
            }
        }
        contains_active.insert(node.id.clone(), has);
    }

    let mut result = Vec::with_capacity(nodes.len());
    // Stack items: (node, ancestor_continues, is_last, show_connector)
    let mut stack: Vec<(&SessionNode, Vec<bool>, bool, bool)> = Vec::new();

    if let Some(roots) = children_map.get(&None) {
        let mut ordered = roots.clone();
        ordered.sort_by_key(|n| {
            std::cmp::Reverse(contains_active.get(&n.id).copied().unwrap_or(false))
        });
        let multi = ordered.len() > 1;
        for (i, root) in ordered.iter().enumerate().rev() {
            stack.push((root, Vec::new(), i + 1 == ordered.len(), multi));
        }
    }

    while let Some((node, gutters, is_last, show_connector)) = stack.pop() {
        let mut prefix = String::new();
        for &cont in &gutters {
            prefix.push_str(if cont { "|  " } else { "   " });
        }
        if show_connector {
            prefix.push_str(if is_last { "\\- " } else { "|- " });
        }

        let (label, is_user, is_tool, is_settings) = summarize_tree_node(node);
        result.push(TreeEntryItem {
            node_id: node.id.clone(),
            kind: node.kind.clone(),
            turn_id: node.turn_id.clone(),
            label,
            prefix_connectors: prefix,
            on_active_path: active_path_ids.contains(&node.id),
            is_head: head_id == Some(node.id.as_str()),
            is_user,
            is_tool,
            is_settings,
        });

        if let Some(children) = children_map.get(&Some(node.id.clone())) {
            let mut ordered = children.clone();
            ordered.sort_by_key(|n| {
                std::cmp::Reverse(contains_active.get(&n.id).copied().unwrap_or(false))
            });
            let multi = ordered.len() > 1;
            let mut next_gutters = gutters;
            if show_connector {
                next_gutters.push(!is_last);
            }
            for (i, child) in ordered.iter().enumerate().rev() {
                stack.push((child, next_gutters.clone(), i + 1 == ordered.len(), multi));
            }
        }
    }

    result
}

fn summarize_tree_node(node: &SessionNode) -> (String, bool, bool, bool) {
    match node.decode_payload() {
        Ok(NodePayload::SessionOpen { workspace_root, .. }) => (
            format!("session_open ({workspace_root})"),
            false,
            false,
            true,
        ),
        Ok(NodePayload::TurnStart { prompt, .. }) => {
            let text = match &prompt {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (
                format!("user: {}", truncate_single_line(&text, 64)),
                true,
                false,
                false,
            )
        }
        Ok(NodePayload::PhaseOpen {
            preset_id,
            base_loop_id,
            ..
        }) => {
            let label = preset_id.unwrap_or(base_loop_id);
            (format!("phase_open ({label})"), false, false, true)
        }
        Ok(NodePayload::PhaseSeal { phase_id, .. }) => {
            (format!("phase_seal ({phase_id})"), false, false, true)
        }
        Ok(NodePayload::Effect { envelope, .. }) => match envelope.effect {
            Effect::Tool { name, args, .. } => {
                let preview = truncate_single_line(&args.to_string(), 48);
                (format!("tool {name} {preview}"), false, true, false)
            }
            Effect::Model { target, kind, .. } => {
                (format!("model ({kind:?}) {target}"), false, false, true)
            }
            Effect::Save { artifact, mode, .. } => {
                (format!("save:{mode:?} {artifact}"), false, false, false)
            }
            Effect::Spawn { loop_id, .. } => (format!("spawn {loop_id}"), false, false, false),
            Effect::Switch { next_loop, .. } => {
                (format!("switch -> {next_loop}"), false, false, true)
            }
            Effect::Pause { reason, .. } => (format!("pause ({reason:?})"), false, false, true),
        },
        Ok(NodePayload::StepResult(res)) => match res {
            StepResult::Model {
                kind: ModelKind::Llm,
                output,
                ..
            } => {
                let text = output.get("text").and_then(Value::as_str).unwrap_or("");
                (
                    format!("assistant: {}", truncate_single_line(text, 60)),
                    false,
                    false,
                    false,
                )
            }
            StepResult::Tool {
                is_error, output, ..
            } => {
                let tag = if is_error { "tool error" } else { "tool ok" };
                let preview = truncate_single_line(&output.to_string(), 48);
                (format!("{tag}: {preview}"), false, true, false)
            }
            StepResult::Error { message, .. } => (
                format!("error: {}", truncate_single_line(&message, 56)),
                false,
                false,
                false,
            ),
            StepResult::Denied { reason, .. } => (
                format!("denied: {}", truncate_single_line(&reason, 56)),
                false,
                false,
                false,
            ),
            StepResult::Aborted { reason, .. } => (
                format!("aborted: {}", truncate_single_line(&reason, 56)),
                false,
                false,
                false,
            ),
            other => (format!("step_result ({other:?})"), false, false, true),
        },
        Ok(NodePayload::Steering { kind, content, .. }) => (
            format!(
                "steering ({kind:?}): {}",
                truncate_single_line(&content.to_string(), 52)
            ),
            false,
            false,
            true,
        ),
        Ok(NodePayload::BranchHead {
            branch_id,
            target_node_id,
            action,
        }) => (
            format!("branch ({action:?}) {branch_id} -> {target_node_id}"),
            false,
            false,
            false,
        ),
        Ok(NodePayload::TurnEnd { status, usage, .. }) => (
            format!("turn_end ({status:?}, {} tok)", usage.total_tokens()),
            false,
            false,
            true,
        ),
        Ok(NodePayload::Custom { namespace, data }) => (
            format!(
                "custom:{namespace} {}",
                truncate_single_line(&data.to_string(), 48)
            ),
            false,
            false,
            false,
        ),
        _ => (node.kind.clone(), false, false, true),
    }
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let single: String = text
        .chars()
        .map(|c| {
            if c == '\n' || c == '\r' || c == '\t' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let trimmed = single.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        let mut out: String = trimmed.chars().take(max_chars.saturating_sub(3)).collect();
        out.push_str("...");
        out
    }
}

fn default_kernel_command_items() -> Vec<SelectListItem> {
    crate::surfaces::KERNEL_COMMAND_SPECS
        .iter()
        .map(|(value, label, description)| SelectListItem {
            value: (*value).into(),
            label: (*label).into(),
            description: (*description).into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autocomplete_opens_on_slash_and_completes_on_tab() {
        let mut state = TuiAppState::new("repo", "general");
        for c in "/mo".chars() {
            state.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert!(state.autocomplete.is_some());
        state.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(state.editor_text, "/mode ");
        assert!(state.autocomplete.is_none());
    }

    #[test]
    fn keyboard_shortcuts_toggle_reasoning_inspector_and_abort() {
        let mut state = TuiAppState::new("repo", "general");
        assert!(state.show_reasoning);
        state.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(!state.show_reasoning);

        state.handle_key_event(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert!(matches!(
            state.overlay,
            Some(ActiveOverlay::InspectorPanel { .. })
        ));
        state.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(state.overlay.is_none());

        state.is_running = true;
        let eff = state.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(eff, TuiEffect::AbortTurn);
    }

    #[test]
    fn hydrate_from_tree_projects_turns_steps_errors_and_ctrl_o_minimizes() {
        use gestalt_kernel::{EffectEnvelope, SESSION_SCHEMA_VERSION, SideEffectClass, Usage};

        let mut tree = SessionTree::new();
        tree.append(
            "n0".into(),
            None,
            None,
            1,
            &NodePayload::SessionOpen {
                schema_version: SESSION_SCHEMA_VERSION,
                workspace_root: "/repo".into(),
                created_at_ms: 1,
            },
        )
        .expect("n0");
        tree.append(
            "n1".into(),
            Some("t1".into()),
            None,
            2,
            &NodePayload::TurnStart {
                turn_id: "t1".into(),
                prompt: json!("read lib.rs"),
                artifact_snapshots: BTreeMap::new(),
            },
        )
        .expect("n1");
        tree.append(
            "n2".into(),
            Some("t1".into()),
            Some("p1".into()),
            3,
            &NodePayload::Effect {
                envelope: EffectEnvelope {
                    effect_id: "e_read".into(),
                    effect: Effect::Tool {
                        name: "read".into(),
                        args: json!({ "path": "src/lib.rs" }),
                        side_effect: SideEffectClass::Read,
                    },
                },
                state_token: None,
            },
        )
        .expect("n2");
        tree.append(
            "n3".into(),
            Some("t1".into()),
            Some("p1".into()),
            4,
            &NodePayload::StepResult(StepResult::Tool {
                effect_id: "e_read".into(),
                output: json!({ "content": "1: pub fn a() {}\n", "total_lines": 1 }),
                is_error: false,
                blob_digest: None,
            }),
        )
        .expect("n3");
        tree.append(
            "n4".into(),
            Some("t1".into()),
            Some("p1".into()),
            5,
            &NodePayload::StepResult(StepResult::Error {
                effect_id: "e_llm".into(),
                message: "[auth_error] HTTP 401: invalid key".into(),
                retryable: false,
            }),
        )
        .expect("n4");
        tree.append(
            "n5".into(),
            Some("t1".into()),
            None,
            6,
            &NodePayload::TurnEnd {
                turn_id: "t1".into(),
                status: TurnStatus::Completed,
                output: json!({ "error": "[auth_error] HTTP 401: invalid key" }),
                usage: Usage {
                    input_units: 10,
                    output_units: 0,
                    cost_micro_usd: 0,
                },
            },
        )
        .expect("n5");

        let mut state = TuiAppState::new("repo", "general");
        state.hydrate_from_tree(&tree);

        assert!(
            state
                .items
                .iter()
                .any(|i| matches!(i, TranscriptItem::TurnHeader { turn_number: 1, .. }))
        );
        assert!(state.items.iter().any(|i| matches!(
            i,
            TranscriptItem::StepToolCall { tool_name, status: ToolStepStatus::Completed, collapsed: false, .. } if tool_name == "read"
        )));
        assert!(state.items.iter().any(|i| matches!(
            i,
            TranscriptItem::ErrorNotice { hint: Some(h), .. } if h.contains("/login")
        )));
        assert!(
            state
                .items
                .iter()
                .any(|i| matches!(i, TranscriptItem::TurnFooter { .. }))
        );

        // Ctrl+O minimizes all step cards
        state.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert!(state.items.iter().any(|i| matches!(
            i,
            TranscriptItem::StepToolCall {
                collapsed: true,
                ..
            }
        )));

        // Tree selector builds entries and cycles filter modes
        state.open_tree_selector(&tree);
        assert!(matches!(
            state.overlay,
            Some(ActiveOverlay::TreeSelector { .. })
        ));
        state.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(matches!(
            state.overlay,
            Some(ActiveOverlay::TreeSelector {
                filter_mode: TreeFilterMode::UserOnly,
                ..
            })
        ));
    }
}
