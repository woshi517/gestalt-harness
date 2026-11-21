#![forbid(unsafe_code)]
//! Pure TUI application state, `RunEvent` reducers, keyboard navigation, and
//! scroll-offset-aware mouse hit-testing.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use gestalt_kernel::{
    BudgetSnapshot, ExtensionRegistry, PauseReason, RunEvent, UIActionSpec, UIPanelSpec,
    UIRendererSpec,
};
use ratatui::layout::Rect;
use serde_json::{Value, json};

use crate::surfaces::tui::components::{SelectListItem, SelectListState};
use crate::surfaces::{PausePrompt, extract_stream_text};

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
}

/// Modal overlay currently capturing keyboard & mouse focus (Pi-style overlay stack).
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
    /// Live `BudgetSnapshot` for footer & `budget_meter` panel.
    pub budget: BudgetSnapshot,
    /// Active `PolicyGate` rule IDs & allowed tools summary for `policy_gate` panel.
    pub policy_summary: Value,
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
            budget: BudgetSnapshot::default(),
            policy_summary: Value::Null,
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
            RunEvent::ToolUi { renderer, data, .. } => match renderer.as_str() {
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
                self.items.push(TranscriptItem::PolicyDenied {
                    effect_id: effect_id.clone(),
                    reason: reason.clone(),
                });
            }
            RunEvent::TurnCompleted { usage, .. } => {
                self.is_running = false;
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
            }
            RunEvent::TurnAborted { reason, .. } => {
                self.is_running = false;
                self.queued_steering.clear();
                self.items.push(TranscriptItem::Notice {
                    text: format!("Turn aborted: {reason}"),
                });
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
                if let Some(ActiveOverlay::InspectorPanel { scroll, .. }) = self.overlay.as_mut() {
                    *scroll = scroll.saturating_sub(3);
                    return TuiEffect::None;
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
                if let Some(ActiveOverlay::InspectorPanel { scroll, .. }) = self.overlay.as_mut() {
                    *scroll = scroll.saturating_add(3);
                    return TuiEffect::None;
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
                            | TranscriptItem::DiffPreview { collapsed, .. } => {
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

fn default_kernel_command_items() -> Vec<SelectListItem> {
    vec![
        SelectListItem {
            value: "/mode".into(),
            label: "/mode <preset>".into(),
            description: "Switch active preset or loop mid-session".into(),
        },
        SelectListItem {
            value: "/steer".into(),
            label: "/steer [--urgent] <msg>".into(),
            description: "Redirect an active Turn between or mid-step".into(),
        },
        SelectListItem {
            value: "/stop".into(),
            label: "/stop".into(),
            description: "Abort the active Turn cleanly".into(),
        },
        SelectListItem {
            value: "/fork".into(),
            label: "/fork [<node_id>] [--new-file]".into(),
            description: "Branch SessionTree from a node".into(),
        },
        SelectListItem {
            value: "/rewind".into(),
            label: "/rewind <node_id>".into(),
            description: "Rewind conversation state (disk files unchanged)".into(),
        },
        SelectListItem {
            value: "/reload".into(),
            label: "/reload <ext>".into(),
            description: "Hot-reload an external extension worker".into(),
        },
        SelectListItem {
            value: "/rollback".into(),
            label: "/rollback <stage_id>".into(),
            description: "Revert a promoted Effect::Save stage".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autocomplete_opens_on_slash_and_completes_on_tab() {
        let mut state = TuiAppState::new("repo", "swe");
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
        let mut state = TuiAppState::new("repo", "swe");
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
}
