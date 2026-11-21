#![forbid(unsafe_code)]
//! Composable `ratatui` frame rendering (`ScrollView` + `Dock` + `Overlay` stack).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use serde_json::{Value, json};

use crate::surfaces::tui::components::{DockSlot, TuiComponentRegistry};
use crate::surfaces::tui::state::{
    ActiveOverlay, ModelConfigStatus, ToolStepStatus, TranscriptItem, TuiAppState,
    filtered_session_indices, filtered_tree_indices,
};
use gestalt_kernel::{PauseReason, SteeringKind, TurnStatus};

const SPINNER_FRAMES: &[&str] = &["|", "/", "-", "\\"];

/// Render one complete TUI frame and record hit-testing rectangles in `state.layout_rects`.
pub fn render_tui(frame: &mut Frame<'_>, state: &mut TuiAppState, registry: &TuiComponentRegistry) {
    let area = frame.area();
    state.layout_rects = Default::default();

    let above_widgets: Vec<_> = registry
        .dock_widgets
        .iter()
        .filter(|w| w.slot() == DockSlot::AboveEditor)
        .collect();
    let below_widgets: Vec<_> = registry
        .dock_widgets
        .iter()
        .filter(|w| w.slot() == DockSlot::BelowEditor)
        .collect();

    let above_widget_rows: usize = above_widgets
        .iter()
        .map(|w| w.render_lines(area.width).len())
        .sum();
    let action_chip_rows: usize = usize::from(!state.ui_actions.is_empty());
    let above_height = (action_chip_rows + above_widget_rows) as u16;
    let has_pending = !state.queued_steering.is_empty();
    let has_actions = above_height > 0;
    let has_loader = state.is_running;

    let mut constraints = vec![Constraint::Min(1)]; // Primary ScrollView
    if has_pending {
        constraints.push(Constraint::Length(1));
    }
    if has_loader {
        constraints.push(Constraint::Length(1));
    }
    if has_actions {
        constraints.push(Constraint::Length(above_height));
    }
    constraints.push(Constraint::Length(3)); // Editor
    if !below_widgets.is_empty() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // Footer

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;
    let transcript_area = chunks[idx];
    idx += 1;
    render_transcript(frame, transcript_area, state, registry);

    if has_pending {
        render_pending_banner(frame, chunks[idx], state);
        idx += 1;
    }
    if has_loader {
        render_loader_line(frame, chunks[idx], state);
        idx += 1;
    }
    if has_actions {
        render_action_bar(frame, chunks[idx], state, &above_widgets);
        idx += 1;
    }

    let editor_area = chunks[idx];
    idx += 1;
    render_editor(frame, editor_area, state);

    if !below_widgets.is_empty() {
        let mut lines = Vec::new();
        for w in &below_widgets {
            lines.extend(w.render_lines(chunks[idx].width));
        }
        frame.render_widget(Paragraph::new(lines), chunks[idx]);
        idx += 1;
    }

    render_footer(frame, chunks[idx], state);

    // Floating `/` autocomplete popup anchored above the editor.
    if state.autocomplete.is_some() && state.overlay.is_none() {
        render_autocomplete_popup(frame, editor_area, area, state);
    }

    // Centered modal overlay (`PauseDialog` or `InspectorPanel`).
    if state.overlay.is_some() {
        render_overlay(frame, area, state, registry);
    }
}

fn render_transcript(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut TuiAppState,
    registry: &TuiComponentRegistry,
) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" Gestalt [{}] ", state.active_preset));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    state.layout_rects.transcript_inner = inner;

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if state.items.is_empty() {
        let mut empty = vec![
            Line::from(Span::styled(
                format!(
                    "Ready in '{}' (preset: {})",
                    state.workspace_label, state.active_preset
                ),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];
        match &state.model_status {
            ModelConfigStatus::Ready { target } => {
                empty.push(Line::from(Span::styled(
                    format!("Active model: {target}"),
                    Style::default().fg(Color::Green),
                )));
            }
            ModelConfigStatus::Unconfigured => {
                empty.push(Line::from(Span::styled(
                    "No model configured. Run /model <provider/model> (e.g. /model anthropic/claude-sonnet-4-6) and /login <provider>.",
                    Style::default().fg(Color::Yellow),
                )));
            }
            ModelConfigStatus::UnknownTarget { target } => {
                empty.push(Line::from(Span::styled(
                    format!("Unknown model target '{target}'. Run /model to pick a valid provider/model."),
                    Style::default().fg(Color::Red),
                )));
            }
            ModelConfigStatus::MissingCredential { provider, target } => {
                empty.push(Line::from(Span::styled(
                    format!("Model '{target}' needs credentials for '{provider}'. Run /login {provider}."),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
        empty.push(Line::from(""));
        empty.push(Line::from(Span::styled(
            "Commands: /model, /thinking, /login, /tree, /session, /skills, /mode, /help",
            Style::default().fg(Color::Gray),
        )));
        empty.push(Line::from(Span::styled(
            "Shortcuts: [Ctrl+O] Steps  [Esc Esc] Tree  [Ctrl+P] Panels  [Ctrl+R] Thinking  [Ctrl+K] Compact  [Ctrl+C] Abort/Quit",
            Style::default().fg(Color::DarkGray),
        )));
        frame.render_widget(Paragraph::new(empty).wrap(Wrap { trim: false }), inner);
        state.max_scroll = 0;
        state.scroll_offset = 0;
        return;
    }

    let mut all_lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();

    for (item_idx, item) in state.items.iter().enumerate() {
        let start = all_lines.len();
        render_transcript_item(
            item,
            inner.width,
            state.show_reasoning,
            registry,
            &mut all_lines,
        );
        let end = all_lines.len();
        if end > start {
            spans.push((start, end, item_idx));
            all_lines.push(Line::from(""));
        }
    }

    state.layout_rects.transcript_line_spans = spans;
    let total_lines = all_lines.len();
    let viewport_h = inner.height as usize;
    state.max_scroll = total_lines.saturating_sub(viewport_h);

    if state.auto_follow {
        state.scroll_offset = state.max_scroll;
    } else {
        state.scroll_offset = state.scroll_offset.min(state.max_scroll);
    }

    let visible_end = (state.scroll_offset + viewport_h).min(total_lines);
    let visible_slice = all_lines[state.scroll_offset..visible_end].to_vec();
    frame.render_widget(Paragraph::new(visible_slice), inner);

    if !state.auto_follow && state.max_scroll > 0 && inner.width >= 24 {
        let label = " [Jump to bottom] ";
        let pill_w = label.len() as u16;
        let pill_x = inner.x + inner.width.saturating_sub(pill_w) / 2;
        let pill_y = inner.y + inner.height.saturating_sub(1);
        let pill_rect = Rect::new(pill_x, pill_y, pill_w, 1);
        state.layout_rects.jump_to_bottom_btn = Some(pill_rect);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                label,
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))),
            pill_rect,
        );
    }
}

fn render_transcript_item(
    item: &TranscriptItem,
    width: u16,
    show_reasoning: bool,
    registry: &TuiComponentRegistry,
    out: &mut Vec<Line<'static>>,
) {
    match item {
        TranscriptItem::UserPrompt { text } => {
            out.push(Line::from(vec![
                Span::styled(
                    "> ",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(text.clone(), Style::default().fg(Color::White)),
            ]));
        }
        TranscriptItem::AssistantText { text, .. } => {
            for line in text.lines() {
                out.push(Line::from(line.to_string()));
            }
            if text.is_empty() {
                out.push(Line::from(""));
            }
        }
        TranscriptItem::Reasoning {
            text, collapsed, ..
        } => {
            if !show_reasoning {
                out.push(Line::from(Span::styled(
                    "[Thinking hidden (Ctrl+R)]",
                    Style::default().fg(Color::DarkGray),
                )));
                return;
            }
            let marker = if *collapsed { "[+]" } else { "[-]" };
            out.push(Line::from(Span::styled(
                format!("{marker} Thinking..."),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            )));
            if !*collapsed {
                for line in text.lines() {
                    out.push(Line::from(Span::styled(
                        format!("  {line}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }
        TranscriptItem::ToolOutput {
            effect_id,
            text,
            collapsed,
        } => {
            let marker = if *collapsed { "[+]" } else { "[-]" };
            let line_count = text.lines().count();
            out.push(Line::from(Span::styled(
                format!("{marker} Tool output ({effect_id}, {line_count} lines)"),
                Style::default().fg(Color::Cyan),
            )));
            if !*collapsed {
                for line in text.lines().take(50) {
                    out.push(Line::from(Span::styled(
                        format!("  {line}"),
                        Style::default().fg(Color::Gray),
                    )));
                }
            }
        }
        TranscriptItem::DiffPreview {
            path,
            additions,
            deletions,
            unified_diff,
            collapsed,
        } => {
            if let Some(custom) = registry.renderers.get("diff_preview") {
                out.extend(custom.render_lines(
                    width,
                    &json!({
                        "path": path,
                        "additions": additions,
                        "deletions": deletions,
                        "unified_diff": unified_diff,
                        "collapsed": collapsed,
                    }),
                ));
                return;
            }
            let marker = if *collapsed { "[+]" } else { "[-]" };
            out.push(Line::from(vec![
                Span::styled(
                    format!("{marker} Diff {path} "),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("+{additions} "), Style::default().fg(Color::Green)),
                Span::styled(format!("-{deletions}"), Style::default().fg(Color::Red)),
            ]));
            if !*collapsed {
                for line in unified_diff.lines() {
                    let style = if line.starts_with('+') && !line.starts_with("+++") {
                        Style::default().fg(Color::Green)
                    } else if line.starts_with('-') && !line.starts_with("---") {
                        Style::default().fg(Color::Red)
                    } else if line.starts_with("@@") {
                        Style::default().fg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    out.push(Line::from(Span::styled(format!("  {line}"), style)));
                }
            }
        }
        TranscriptItem::DecisionRoute { data } => {
            if let Some(custom) = registry.renderers.get("decision_route") {
                out.extend(custom.render_lines(width, data));
                return;
            }
            let choice = data
                .get("choice")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let conf = data
                .get("confidence")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            out.push(Line::from(Span::styled(
                format!("[Decision Route] choice={choice} confidence={conf:.2}"),
                Style::default().fg(Color::Magenta),
            )));
        }
        TranscriptItem::CustomToolUi {
            renderer,
            title,
            data,
        } => {
            if let Some(custom) = registry.renderers.get(renderer) {
                out.extend(custom.render_lines(width, data));
                return;
            }
            out.push(Line::from(Span::styled(
                format!("[{title}] {data}"),
                Style::default().fg(Color::Blue),
            )));
        }
        TranscriptItem::PhaseDivider {
            phase_id,
            preset_id,
            base_loop_id,
        } => {
            let label = preset_id.as_deref().unwrap_or(base_loop_id.as_str());
            out.push(Line::from(Span::styled(
                format!("--- Phase {phase_id} ({label}) ---"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        TranscriptItem::PolicyDenied { effect_id, reason } => {
            out.push(Line::from(Span::styled(
                format!("[Policy Denied] ({effect_id}) {reason}"),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
        }
        TranscriptItem::Notice { text } => {
            out.push(Line::from(Span::styled(
                format!("[Notice] {text}"),
                Style::default().fg(Color::Yellow),
            )));
        }
        TranscriptItem::TurnHeader {
            turn_number,
            prompt,
            ..
        } => {
            out.push(Line::from(vec![
                Span::styled(
                    format!("[Turn {turn_number}] > "),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    prompt.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        TranscriptItem::StepToolCall {
            step_index,
            tool_name,
            args,
            status,
            streaming_output,
            result_output,
            diff_preview,
            is_error,
            blob_digest,
            collapsed,
            ..
        } => {
            render_step_tool_call(
                *step_index,
                tool_name,
                args,
                *status,
                streaming_output,
                result_output.as_ref(),
                diff_preview.as_ref(),
                *is_error,
                blob_digest.as_deref(),
                *collapsed,
                width,
                registry,
                out,
            );
        }
        TranscriptItem::StepControl {
            step_index,
            kind,
            summary,
            status,
            detail,
            collapsed,
            ..
        } => {
            let marker = if *collapsed { "[+]" } else { "[-]" };
            let (badge, badge_color) = status_badge(*status);
            out.push(Line::from(vec![
                Span::styled(
                    format!("{marker} Step {step_index}: {kind} "),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(summary.clone(), Style::default().fg(Color::White)),
                Span::raw(" "),
                Span::styled(badge, Style::default().fg(badge_color)),
            ]));
            if !*collapsed
                && let Some(d) = detail
                && !d.is_empty()
            {
                out.push(Line::from(Span::styled(
                    format!("    {d}"),
                    Style::default().fg(Color::Gray),
                )));
            }
        }
        TranscriptItem::CompactionSummary {
            summary,
            compacted_count,
            collapsed,
            ..
        } => {
            let marker = if *collapsed { "[+]" } else { "[-]" };
            out.push(Line::from(vec![
                Span::styled(
                    format!("{marker} [compaction] "),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("Compacted {compacted_count} node(s)"),
                    Style::default().fg(Color::Gray),
                ),
            ]));
            if !*collapsed {
                for line in summary.lines().take(30) {
                    out.push(Line::from(Span::styled(
                        format!("    {line}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }
        TranscriptItem::SteeringNote { kind, text } => {
            let tag = match kind {
                SteeringKind::Steer => "steer",
                SteeringKind::UrgentSteer => "urgent steer",
                SteeringKind::FollowUp => "follow-up",
                SteeringKind::Abort => "abort",
            };
            out.push(Line::from(Span::styled(
                format!("[{tag}] {text}"),
                Style::default().fg(Color::Yellow),
            )));
        }
        TranscriptItem::TurnFooter {
            status,
            steps,
            input_tokens,
            output_tokens,
            cost_micro_usd,
            ..
        } => {
            let status_str = match status {
                TurnStatus::Completed => "completed",
                TurnStatus::Aborted => "aborted",
                TurnStatus::Failed => "failed",
            };
            let cost = *cost_micro_usd as f64 / 1_000_000.0;
            out.push(Line::from(Span::styled(
                format!(
                    "--- Turn {status_str} | {steps} step(s) | in:{input_tokens} out:{output_tokens} (${cost:.4}) ---"
                ),
                Style::default().fg(Color::DarkGray),
            )));
        }
        TranscriptItem::ErrorNotice {
            title,
            message,
            hint,
        } => {
            out.push(Line::from(vec![
                Span::styled(
                    format!("[Error] {title}: "),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(message.clone(), Style::default().fg(Color::Red)),
            ]));
            if let Some(h) = hint {
                out.push(Line::from(Span::styled(
                    format!("  Hint: {h}"),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
    }
}

fn render_pending_banner(frame: &mut Frame<'_>, area: Rect, state: &TuiAppState) {
    let joined = state.queued_steering.join(" | ");
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("Queued steering: {joined}"),
            Style::default().fg(Color::Yellow),
        ))),
        area,
    );
}

fn render_loader_line(frame: &mut Frame<'_>, area: Rect, state: &TuiAppState) {
    let spin = SPINNER_FRAMES[state.spinner_tick % SPINNER_FRAMES.len()];
    let (msg, msg_color) = match &state.active_status_message {
        Some(status) => (format!("{status}  [Ctrl+C] Abort"), Color::Yellow),
        None => (
            format!(
                "Working ({})... [Ctrl+C] Abort  [Enter] Steer",
                state.active_preset
            ),
            Color::Gray,
        ),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("[RUN {spin}] "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(msg, Style::default().fg(msg_color)),
        ])),
        area,
    );
}

fn render_action_bar(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut TuiAppState,
    above_widgets: &[&std::sync::Arc<dyn crate::surfaces::tui::components::TuiDockWidget>],
) {
    let mut x = area.x;
    let mut spans = Vec::new();
    for action in &state.ui_actions {
        let chip = format!("[{}]", action.label);
        let w = chip.len() as u16;
        if x + w <= area.x + area.width {
            state.layout_rects.action_chips.push((
                action.id.clone(),
                action.command_or_signal.clone(),
                Rect::new(x, area.y, w, 1),
            ));
            spans.push(Span::styled(chip, Style::default().fg(Color::Cyan)));
            spans.push(Span::raw(" "));
            x = x.saturating_add(w + 1);
        }
    }
    let mut lines = Vec::new();
    if !state.ui_actions.is_empty() {
        lines.push(Line::from(spans));
    }
    for w in above_widgets {
        lines.extend(w.render_lines(area.width));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_editor(frame: &mut Frame<'_>, area: Rect, state: &mut TuiAppState) {
    state.layout_rects.editor = area;
    let title = if state.is_running {
        " Steer active Turn (or /stop) "
    } else {
        " Prompt (/ for commands) "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);
    let inner = block.inner(area);
    let text = format!("> {}", state.editor_text);
    frame.render_widget(Paragraph::new(text).block(block), area);

    if state.overlay.is_none() && inner.width > 2 && inner.height > 0 {
        let prefix_chars = state.editor_text[..state.cursor_pos.min(state.editor_text.len())]
            .chars()
            .count() as u16;
        let cx = inner.x + 2 + prefix_chars.min(inner.width.saturating_sub(3));
        frame.set_cursor_position((cx, inner.y));
    }
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, state: &TuiAppState) {
    let cost_usd = state.budget.cost_micro_usd_used as f64 / 1_000_000.0;
    let (model_seg, model_color) = match &state.model_status {
        ModelConfigStatus::Ready { target } => (format!(" | model: {target}"), Color::Gray),
        ModelConfigStatus::Unconfigured => (" | model: [none]".to_string(), Color::Yellow),
        ModelConfigStatus::UnknownTarget { target } => {
            (format!(" | model: {target} [invalid]"), Color::Red)
        }
        ModelConfigStatus::MissingCredential { target, .. } => {
            (format!(" | model: {target} [needs /login]"), Color::Yellow)
        }
    };
    let think_seg = state
        .reasoning_budget_tokens
        .map(|t| format!(" | thinking: {t}"))
        .unwrap_or_default();
    let left_prefix = format!(
        " {} | preset: {}",
        state.workspace_label, state.active_preset
    );
    let left_suffix = format!(
        "{} | steps: {} tokens: {} (${:.4})",
        think_seg, state.budget.steps_used, state.budget.tokens_used, cost_usd
    );
    let right = "[Ctrl+O] Steps  [Ctrl+P] Panels  [Ctrl+R] Thinking ";
    let line = Line::from(vec![
        Span::styled(left_prefix, Style::default().fg(Color::Gray)),
        Span::styled(model_seg, Style::default().fg(model_color)),
        Span::styled(left_suffix, Style::default().fg(Color::Gray)),
        Span::raw("  "),
        Span::styled(right, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_autocomplete_popup(
    frame: &mut Frame<'_>,
    editor_area: Rect,
    screen: Rect,
    state: &mut TuiAppState,
) {
    let Some(ref ac) = state.autocomplete else {
        return;
    };
    let visible = ac.visible_items();
    if visible.is_empty() {
        return;
    }
    let height = (visible.len() as u16 + 2).min(editor_area.y);
    if height < 3 {
        return;
    }
    let width = screen.width.min(64);
    let popup_rect = Rect::new(
        editor_area.x,
        editor_area.y.saturating_sub(height),
        width,
        height,
    );
    state.layout_rects.autocomplete_popup = Some(popup_rect);

    frame.render_widget(Clear, popup_rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(" Commands ");
    let inner = block.inner(popup_rect);
    frame.render_widget(block, popup_rect);

    let mut lines = Vec::new();
    for (row_offset, (f_idx, item)) in visible.into_iter().enumerate() {
        let selected = f_idx == ac.selected_index();
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(Span::styled(
            format!(" {:<14} {}", item.value, item.description),
            style,
        )));
        if (row_offset as u16) < inner.height {
            state.layout_rects.autocomplete_rows.push((
                f_idx,
                Rect::new(inner.x, inner.y + row_offset as u16, inner.width, 1),
            ));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_overlay(
    frame: &mut Frame<'_>,
    screen: Rect,
    state: &mut TuiAppState,
    registry: &TuiComponentRegistry,
) {
    let Some(overlay) = state.overlay.clone() else {
        return;
    };
    let (max_w, max_h) = match &overlay {
        ActiveOverlay::TreeSelector { .. } | ActiveOverlay::SessionSelector { .. } => (96, 22),
        ActiveOverlay::SelectorModal { .. } => (76, 16),
        _ => (72, 18),
    };
    let box_w = screen
        .width
        .clamp(24.min(screen.width), max_w.min(screen.width));
    let box_h = screen
        .height
        .clamp(8.min(screen.height), max_h.min(screen.height));
    let box_x = screen.x + screen.width.saturating_sub(box_w) / 2;
    let box_y = screen.y + screen.height.saturating_sub(box_h) / 2;
    let overlay_rect = Rect::new(box_x, box_y, box_w, box_h);
    state.layout_rects.overlay_box = Some(overlay_rect);

    frame.render_widget(Clear, overlay_rect);

    match overlay {
        ActiveOverlay::PauseDialog {
            reason,
            title,
            payload,
            input_buf,
            secret_masked,
            selected_btn,
            ..
        } => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(format!(" {title} "));
            let inner = block.inner(overlay_rect);
            frame.render_widget(block, overlay_rect);
            if inner.height < 3 || inner.width < 10 {
                return;
            }

            let mut body_lines = Vec::new();
            if reason == PauseReason::Input {
                let shown = if secret_masked {
                    "*".repeat(input_buf.chars().count())
                } else {
                    input_buf
                };
                body_lines.push(Line::from(format!("Input: {shown}_")));
            } else {
                format_pause_details(&payload, &mut body_lines);
            }

            let content_h = inner.height.saturating_sub(2);
            frame.render_widget(
                Paragraph::new(body_lines).wrap(Wrap { trim: false }),
                Rect::new(inner.x, inner.y, inner.width, content_h),
            );

            let btn_y = inner.y + inner.height.saturating_sub(1);
            let approve_label = if reason == PauseReason::Input {
                "[ Submit ]"
            } else {
                "[ Approve ]"
            };
            let deny_label = "[ Deny ]";
            let approve_rect = Rect::new(inner.x, btn_y, approve_label.len() as u16, 1);
            let deny_rect = Rect::new(
                inner.x + approve_rect.width + 2,
                btn_y,
                deny_label.len() as u16,
                1,
            );
            state.layout_rects.overlay_primary_btn = Some(approve_rect);
            state.layout_rects.overlay_secondary_btn = Some(deny_rect);

            let approve_style = if selected_btn == 0 {
                Style::default().fg(Color::Black).bg(Color::Green)
            } else {
                Style::default().fg(Color::Green)
            };
            let deny_style = if selected_btn == 1 {
                Style::default().fg(Color::Black).bg(Color::Red)
            } else {
                Style::default().fg(Color::Red)
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(approve_label, approve_style))),
                approve_rect,
            );
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(deny_label, deny_style))),
                deny_rect,
            );
        }
        ActiveOverlay::InspectorPanel {
            active_panel_idx,
            scroll,
        } => {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Inspector Panels ([Tab] Switch, [Esc] Close) ");
            let inner = block.inner(overlay_rect);
            frame.render_widget(block, overlay_rect);
            if inner.height < 3 || inner.width < 10 {
                return;
            }

            let mut tab_x = inner.x;
            let mut tab_spans = Vec::new();
            for (idx, panel) in state.ui_panels.iter().enumerate() {
                let label = format!(" {} ", panel.title);
                let w = label.len() as u16;
                if tab_x + w <= inner.x + inner.width {
                    state
                        .layout_rects
                        .overlay_tabs
                        .push((idx, Rect::new(tab_x, inner.y, w, 1)));
                    let style = if idx == active_panel_idx {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    tab_spans.push(Span::styled(label, style));
                    tab_spans.push(Span::raw(" "));
                    tab_x = tab_x.saturating_add(w + 1);
                }
            }
            frame.render_widget(
                Paragraph::new(Line::from(tab_spans)),
                Rect::new(inner.x, inner.y, inner.width, 1),
            );

            let body_area = Rect::new(
                inner.x,
                inner.y + 2,
                inner.width,
                inner.height.saturating_sub(2),
            );
            let mut body_lines = Vec::new();
            if let Some(panel) = state.ui_panels.get(active_panel_idx) {
                if let Some(custom) = registry.panels.get(&panel.id) {
                    body_lines.extend(custom.render_lines(body_area.width, &state.policy_summary));
                } else if panel.id == "budget_meter" {
                    let cost = state.budget.cost_micro_usd_used as f64 / 1_000_000.0;
                    body_lines.push(Line::from(format!(
                        "Steps used: {}",
                        state.budget.steps_used
                    )));
                    body_lines.push(Line::from(format!(
                        "Tokens used: {}",
                        state.budget.tokens_used
                    )));
                    body_lines.push(Line::from(format!("Cost: ${cost:.6}")));
                } else {
                    body_lines.push(Line::from(format!("Panel: {}", panel.title)));
                    body_lines.push(Line::from(state.policy_summary.to_string()));
                }
            } else {
                body_lines.push(Line::from("No panels registered."));
            }
            let start = scroll.min(body_lines.len());
            frame.render_widget(
                Paragraph::new(body_lines[start..].to_vec()).wrap(Wrap { trim: false }),
                body_area,
            );
        }
        ActiveOverlay::TreeSelector {
            entries,
            filter_mode,
            search,
            selected_idx,
            ..
        } => {
            render_tree_selector(
                frame,
                overlay_rect,
                state,
                &entries,
                filter_mode,
                &search,
                selected_idx,
            );
        }
        ActiveOverlay::SessionSelector {
            sessions,
            current_path,
            search,
            selected_idx,
        } => {
            render_session_selector(
                frame,
                overlay_rect,
                state,
                &sessions,
                &current_path,
                &search,
                selected_idx,
            );
        }
        ActiveOverlay::SelectorModal {
            title,
            list,
            filter,
            ..
        } => {
            render_selector_modal(frame, overlay_rect, state, &title, &list, &filter);
        }
    }
}

fn status_badge(status: ToolStepStatus) -> (&'static str, Color) {
    match status {
        ToolStepStatus::Pending => ("[...]", Color::DarkGray),
        ToolStepStatus::Running => ("[RUN]", Color::Cyan),
        ToolStepStatus::Completed => ("[OK]", Color::Green),
        ToolStepStatus::Error => ("[ERR]", Color::Red),
        ToolStepStatus::Denied => ("[DENIED]", Color::Red),
        ToolStepStatus::Aborted => ("[ABORTED]", Color::Yellow),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_step_tool_call(
    step_index: usize,
    tool_name: &str,
    args: &Value,
    status: ToolStepStatus,
    streaming_output: &str,
    result_output: Option<&Value>,
    diff_preview: Option<&crate::surfaces::tui::state::DiffPreviewData>,
    is_error: bool,
    blob_digest: Option<&str>,
    collapsed: bool,
    width: u16,
    registry: &TuiComponentRegistry,
    out: &mut Vec<Line<'static>>,
) {
    let marker = if collapsed { "[+]" } else { "[-]" };
    let (badge, badge_color) = status_badge(status);
    let summary_header = format_tool_header(tool_name, args, diff_preview);

    out.push(Line::from(vec![
        Span::styled(
            format!("{marker} Step {step_index}: "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            summary_header,
            Style::default()
                .fg(if is_error { Color::Red } else { Color::White })
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(badge, Style::default().fg(badge_color)),
    ]));

    if collapsed {
        if let Some(compact) = format_collapsed_tool_summary(
            tool_name,
            streaming_output,
            result_output,
            diff_preview,
            is_error,
        ) {
            out.push(Line::from(Span::styled(
                format!("    {compact}"),
                Style::default().fg(if is_error {
                    Color::Red
                } else {
                    Color::DarkGray
                }),
            )));
        }
        return;
    }

    match tool_name {
        "edit" => {
            if let Some(dp) = diff_preview {
                if let Some(custom) = registry.renderers.get("diff_preview") {
                    out.extend(custom.render_lines(
                        width,
                        &json!({
                            "path": dp.path,
                            "additions": dp.additions,
                            "deletions": dp.deletions,
                            "unified_diff": dp.unified_diff,
                            "collapsed": false,
                        }),
                    ));
                } else {
                    for line in dp.unified_diff.lines().take(60) {
                        let style = if line.starts_with('+') && !line.starts_with("+++") {
                            Style::default().fg(Color::Green)
                        } else if line.starts_with('-') && !line.starts_with("---") {
                            Style::default().fg(Color::Red)
                        } else if line.starts_with("@@") {
                            Style::default().fg(Color::Cyan)
                        } else {
                            Style::default().fg(Color::Gray)
                        };
                        out.push(Line::from(Span::styled(format!("    {line}"), style)));
                    }
                }
                return;
            }
        }
        "bash" => {
            let combined = collect_bash_output_text(streaming_output, result_output);
            if !combined.is_empty() {
                let lines: Vec<&str> = combined.lines().collect();
                let start = lines.len().saturating_sub(40);
                if start > 0 {
                    out.push(Line::from(Span::styled(
                        format!("    ... ({start} earlier lines)"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                for line in &lines[start..] {
                    out.push(Line::from(Span::styled(
                        format!("    {line}"),
                        Style::default().fg(if is_error { Color::Red } else { Color::Gray }),
                    )));
                }
            }
            if let Some(digest) = blob_digest {
                out.push(Line::from(Span::styled(
                    format!("    [spilled to .gestalt/blobs/{digest}]"),
                    Style::default().fg(Color::Yellow),
                )));
            }
            return;
        }
        "read" => {
            if let Some(content) = result_output
                .and_then(|v| v.get("content"))
                .and_then(Value::as_str)
            {
                let lines: Vec<&str> = content.lines().collect();
                for line in lines.iter().take(25) {
                    out.push(Line::from(Span::styled(
                        format!("    {line}"),
                        Style::default().fg(Color::Gray),
                    )));
                }
                if lines.len() > 25 {
                    out.push(Line::from(Span::styled(
                        format!("    ... ({} more lines)", lines.len() - 25),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                return;
            }
        }
        "glob" => {
            if let Some(matches) = result_output
                .and_then(|v| v.get("matches"))
                .and_then(Value::as_array)
            {
                for m in matches.iter().take(25) {
                    if let Some(s) = m.as_str() {
                        out.push(Line::from(Span::styled(
                            format!("    {s}"),
                            Style::default().fg(Color::Gray),
                        )));
                    }
                }
                if matches.len() > 25 {
                    out.push(Line::from(Span::styled(
                        format!("    ... ({} more files)", matches.len() - 25),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                return;
            }
        }
        "grep" => {
            if let Some(matches) = result_output
                .and_then(|v| v.get("matches"))
                .and_then(Value::as_array)
            {
                for m in matches.iter().take(25) {
                    let path = m.get("path").and_then(Value::as_str).unwrap_or("");
                    let line_no = m.get("line").and_then(Value::as_u64).unwrap_or(0);
                    let text = m.get("text").and_then(Value::as_str).unwrap_or("");
                    out.push(Line::from(Span::styled(
                        format!("    {path}:{line_no}: {text}"),
                        Style::default().fg(Color::Gray),
                    )));
                }
                if matches.len() > 25 {
                    out.push(Line::from(Span::styled(
                        format!("    ... ({} more matches)", matches.len() - 25),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                return;
            }
        }
        _ => {}
    }

    if let Some(res_val) = result_output {
        let text = match res_val {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
        };
        for line in text.lines().take(20) {
            out.push(Line::from(Span::styled(
                format!("    {line}"),
                Style::default().fg(if is_error { Color::Red } else { Color::Gray }),
            )));
        }
    }
    if let Some(digest) = blob_digest {
        out.push(Line::from(Span::styled(
            format!("    [spilled to .gestalt/blobs/{digest}]"),
            Style::default().fg(Color::Yellow),
        )));
    }
}

fn format_tool_header(
    tool_name: &str,
    args: &Value,
    diff_preview: Option<&crate::surfaces::tui::state::DiffPreviewData>,
) -> String {
    match tool_name {
        "read" => {
            let path = args.get("path").and_then(Value::as_str).unwrap_or("?");
            let start = args.get("start_line").and_then(Value::as_u64);
            let end = args.get("end_line").and_then(Value::as_u64);
            let range = match (start, end) {
                (Some(s), Some(e)) => format!(":{s}-{e}"),
                (Some(s), None) => format!(":{s}"),
                _ => String::new(),
            };
            format!("read {path}{range}")
        }
        "write" => {
            let path = args.get("path").and_then(Value::as_str).unwrap_or("?");
            let bytes = args
                .get("content")
                .and_then(Value::as_str)
                .map_or(0, str::len);
            format!("write {path} ({bytes} bytes)")
        }
        "edit" => {
            let path = args.get("path").and_then(Value::as_str).unwrap_or("?");
            if let Some(dp) = diff_preview {
                format!("edit {path} (+{} -{})", dp.additions, dp.deletions)
            } else {
                format!("edit {path}")
            }
        }
        "bash" => {
            let cmd = args.get("command").and_then(Value::as_str).unwrap_or("?");
            let short = if cmd.chars().count() > 64 {
                let mut s: String = cmd.chars().take(61).collect();
                s.push_str("...");
                s
            } else {
                cmd.to_string()
            };
            format!("$ {short}")
        }
        "glob" => {
            let pat = args.get("pattern").and_then(Value::as_str).unwrap_or("?");
            format!("glob {pat}")
        }
        "grep" => {
            let pat = args.get("pattern").and_then(Value::as_str).unwrap_or("?");
            let path = args.get("path").and_then(Value::as_str).unwrap_or(".");
            format!("grep /{pat}/ in {path}")
        }
        other => format!("{other} {}", args),
    }
}

fn format_collapsed_tool_summary(
    tool_name: &str,
    streaming_output: &str,
    result_output: Option<&Value>,
    diff_preview: Option<&crate::surfaces::tui::state::DiffPreviewData>,
    is_error: bool,
) -> Option<String> {
    if is_error && let Some(Value::String(msg)) = result_output {
        return Some(msg.lines().next().unwrap_or(msg).to_string());
    }
    match tool_name {
        "bash" => {
            let combined = collect_bash_output_text(streaming_output, result_output);
            let lines: Vec<&str> = combined.lines().collect();
            if lines.is_empty() {
                result_output
                    .and_then(|v| v.get("exit_code").and_then(Value::as_i64))
                    .map(|c| format!("exit {c}"))
            } else {
                let tail_start = lines.len().saturating_sub(3);
                Some(format!(
                    "... ({} lines) {}",
                    lines.len(),
                    lines[tail_start..].join(" | ")
                ))
            }
        }
        "read" => result_output
            .and_then(|v| v.get("total_lines").and_then(Value::as_u64))
            .map(|n| format!("{n} total lines")),
        "edit" => {
            diff_preview.map(|dp| format!("+{} -{} in {}", dp.additions, dp.deletions, dp.path))
        }
        "glob" | "grep" => result_output
            .and_then(|v| v.get("matches"))
            .and_then(Value::as_array)
            .map(|m| format!("{} match(es)", m.len())),
        _ => None,
    }
}

fn collect_bash_output_text(streaming_output: &str, result_output: Option<&Value>) -> String {
    if let Some(res) = result_output {
        let stdout = res.get("stdout").and_then(Value::as_str).unwrap_or("");
        let stderr = res.get("stderr").and_then(Value::as_str).unwrap_or("");
        if !stdout.is_empty() || !stderr.is_empty() {
            return if stderr.is_empty() {
                stdout.to_string()
            } else if stdout.is_empty() {
                stderr.to_string()
            } else {
                format!("{stdout}\n{stderr}")
            };
        }
    }
    streaming_output.to_string()
}

fn format_pause_details(payload: &Value, out: &mut Vec<Line<'static>>) {
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            let val_str = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out.push(Line::from(vec![
                Span::styled(
                    format!("{k}: "),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(val_str),
            ]));
        }
    } else {
        out.push(Line::from(format!("Details: {payload}")));
    }
}

fn render_tree_selector(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut TuiAppState,
    entries: &[crate::surfaces::tui::state::TreeEntryItem],
    filter_mode: crate::surfaces::tui::state::TreeFilterMode,
    search: &str,
    selected_idx: usize,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(format!(
            " SessionTree ([Tab] Filter: {}) ",
            filter_mode.label()
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 20 {
        return;
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Search: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{search}_")),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let visible = filtered_tree_indices(entries, filter_mode, search);
    let list_h = inner.height.saturating_sub(3) as usize;
    let total = visible.len();
    let start = if selected_idx >= list_h {
        selected_idx + 1 - list_h
    } else {
        0
    };

    let mut rows = Vec::new();
    for (row_offset, vis_idx) in (start..total.min(start + list_h)).enumerate() {
        let entry = &entries[visible[vis_idx]];
        let is_sel = vis_idx == selected_idx;
        let active_mark = if entry.on_active_path { "*" } else { " " };
        let head_tag = if entry.is_head { " <- active" } else { "" };
        let line_text = format!(
            "{active_mark} {}{}{head_tag}",
            entry.prefix_connectors, entry.label
        );
        let style = if is_sel {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if entry.on_active_path {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        rows.push(Line::from(Span::styled(line_text, style)));
        state.layout_rects.overlay_list_rows.push((
            vis_idx,
            Rect::new(inner.x, inner.y + 1 + row_offset as u16, inner.width, 1),
        ));
    }
    if rows.is_empty() {
        rows.push(Line::from(Span::styled(
            "(no matching nodes)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(
        Paragraph::new(rows),
        Rect::new(inner.x, inner.y + 1, inner.width, list_h as u16),
    );

    let footer_y = inner.y + inner.height.saturating_sub(1);
    let fork_label = "[ Fork Here (Ctrl+F) ]";
    let fork_w = fork_label.len() as u16;
    if inner.width > fork_w + 2 {
        let fork_rect = Rect::new(inner.x + inner.width - fork_w, footer_y, fork_w, 1);
        state.layout_rects.overlay_fork_btn = Some(fork_rect);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                fork_label,
                Style::default().fg(Color::Green),
            ))),
            fork_rect,
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[Enter] Rewind  [Esc] Close",
            Style::default().fg(Color::DarkGray),
        ))),
        Rect::new(inner.x, footer_y, inner.width.saturating_sub(fork_w + 1), 1),
    );
}

fn render_session_selector(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut TuiAppState,
    sessions: &[crate::surfaces::tui::state::SessionFileItem],
    current_path: &std::path::Path,
    search: &str,
    selected_idx: usize,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Sessions ([Enter] Switch, [Esc] Close) ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 20 {
        return;
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{search}_")),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let visible = filtered_session_indices(sessions, search);
    let list_h = inner.height.saturating_sub(2) as usize;
    let total = visible.len();
    let start = if selected_idx >= list_h {
        selected_idx + 1 - list_h
    } else {
        0
    };

    let mut rows = Vec::new();
    for (row_offset, vis_idx) in (start..total.min(start + list_h)).enumerate() {
        let item = &sessions[visible[vis_idx]];
        let is_sel = vis_idx == selected_idx;
        let cur_mark = if item.path == current_path {
            "* "
        } else {
            "  "
        };
        let label = format!(
            "{cur_mark}{} ({} turns, {} nodes) - {}",
            item.filename, item.turn_count, item.node_count, item.preview
        );
        let style = if is_sel {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        rows.push(Line::from(Span::styled(label, style)));
        state.layout_rects.overlay_list_rows.push((
            vis_idx,
            Rect::new(inner.x, inner.y + 1 + row_offset as u16, inner.width, 1),
        ));
    }
    if rows.is_empty() {
        rows.push(Line::from(Span::styled(
            "(no saved sessions found)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(
        Paragraph::new(rows),
        Rect::new(inner.x, inner.y + 1, inner.width, list_h as u16),
    );
}

fn render_selector_modal(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &mut TuiAppState,
    title: &str,
    list: &crate::surfaces::tui::components::SelectListState,
    filter: &str,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(format!(" {title} ([Enter] Select, [Esc] Close) "));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 20 {
        return;
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{filter}_")),
        ])),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let mut rows = Vec::new();
    for (row_offset, (f_idx, item)) in list.visible_items().into_iter().enumerate() {
        let selected = f_idx == list.selected_index();
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        rows.push(Line::from(Span::styled(
            format!(" {:<30} {}", item.label, item.description),
            style,
        )));
        if (row_offset as u16) + 1 < inner.height {
            state.layout_rects.overlay_list_rows.push((
                f_idx,
                Rect::new(inner.x, inner.y + 1 + row_offset as u16, inner.width, 1),
            ));
        }
    }
    if rows.is_empty() {
        rows.push(Line::from(Span::styled(
            "(no matching items)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(
        Paragraph::new(rows),
        Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1),
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::tui::components::TuiComponent;
    use crate::surfaces::tui::state::TuiEffect;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use gestalt_kernel::RunEvent;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::sync::Arc;

    #[test]
    fn scroll_offset_aware_mouse_hit_testing_toggles_correct_block() {
        let mut state = TuiAppState::new("repo", "general");
        // Block 0 at top (~5 lines), then filler lines, then Block 2 near line 20.
        state.apply_run_event(&RunEvent::StreamDelta {
            turn_id: "t1".into(),
            phase_id: "p1".into(),
            effect_id: "thought_1".into(),
            channel: "thought".into(),
            payload: json!({ "text": "first\nline2\nline3\nline4" }),
        });
        for i in 0..16 {
            state.items.push(TranscriptItem::Notice {
                text: format!("filler_{i}"),
            });
        }
        state.apply_run_event(&RunEvent::StreamDelta {
            turn_id: "t1".into(),
            phase_id: "p1".into(),
            effect_id: "thought_2".into(),
            channel: "thought".into(),
            payload: json!({ "text": "second\nline2" }),
        });

        let backend = TestBackend::new(80, 18);
        let mut terminal = Terminal::new(backend).expect("term");
        let reg = TuiComponentRegistry::default();

        // Scroll to top (offset = 0) and click viewport row 1 -> toggles thought_1 (item 0)
        state.auto_follow = false;
        state.scroll_offset = 0;
        terminal
            .draw(|f| render_tui(f, &mut state, &reg))
            .expect("draw 1");
        let vp_y = state.layout_rects.transcript_inner.y;
        let vp_x = state.layout_rects.transcript_inner.x;
        state.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: vp_x + 2,
            row: vp_y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(
            state.items[0],
            TranscriptItem::Reasoning {
                collapsed: true,
                ..
            }
        ));
        assert!(matches!(
            state.items[17],
            TranscriptItem::Reasoning {
                collapsed: false,
                ..
            }
        ));

        // Now scroll down to the bottom (where thought_2 is visible), re-render, and click row 1.
        // Because scroll_offset > 0, clicking the viewport must hit thought_2 (NOT thought_1!).
        state.scroll_offset = state.max_scroll;
        terminal
            .draw(|f| render_tui(f, &mut state, &reg))
            .expect("draw 2");
        let t2_start = state
            .layout_rects
            .transcript_line_spans
            .iter()
            .find(|&&(_, _, idx)| idx == 17)
            .expect("thought_2 span")
            .0;
        let screen_row = vp_y + (t2_start - state.scroll_offset) as u16;
        state.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: vp_x + 2,
            row: screen_row,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(
            state.items[17],
            TranscriptItem::Reasoning {
                collapsed: true,
                ..
            }
        ));
        // thought_1 stayed collapsed:true (was not toggled back)
        assert!(matches!(
            state.items[0],
            TranscriptItem::Reasoning {
                collapsed: true,
                ..
            }
        ));
    }

    #[test]
    fn provider_login_masks_secret_and_custom_component_overrides_renderer() {
        let mut state = TuiAppState::new("repo", "general");
        state.open_pause_dialog(crate::surfaces::PausePrompt {
            pause_id: "p_login".into(),
            effect_id: "e_login".into(),
            reason: PauseReason::Input,
            payload: json!({
                "dialog": "provider_login",
                "provider": "anthropic",
                "secret": true,
                "prompt": "Enter API key",
            }),
        });
        for c in "sk-secret-xyz".chars() {
            state.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }

        struct CustomDiff;
        impl TuiComponent for CustomDiff {
            fn id(&self) -> &str {
                "diff_preview"
            }
            fn render_lines(&self, _width: u16, _data: &Value) -> Vec<Line<'static>> {
                vec![Line::from("CUSTOM_DIFF_CARD")]
            }
        }
        let mut reg = TuiComponentRegistry::default();
        reg.register_renderer(Arc::new(CustomDiff));
        state.apply_run_event(&RunEvent::ToolUi {
            turn_id: "t1".into(),
            effect_id: "e1".into(),
            renderer: "diff_preview".into(),
            data: json!({ "path": "a.rs", "additions": 1, "deletions": 0, "unified_diff": "+hi" }),
        });

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("term");
        terminal
            .draw(|f| render_tui(f, &mut state, &reg))
            .expect("draw");

        let buf = terminal.backend().buffer();
        let mut rendered = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                rendered.push_str(buf[(x, y)].symbol());
            }
            rendered.push('\n');
        }
        assert!(!rendered.contains("sk-secret-xyz"));
        assert!(rendered.contains("*************"));
        assert!(rendered.contains("CUSTOM_DIFF_CARD"));

        // Clicking primary Submit button resolves with the unmasked input buffer
        let btn = state
            .layout_rects
            .overlay_primary_btn
            .expect("submit button rect");
        let eff = state.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: btn.x + 1,
            row: btn.y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            eff,
            TuiEffect::ResolvePause {
                pause_id: "p_login".into(),
                resolution: json!({ "secret": "sk-secret-xyz" }),
            }
        );
    }

    #[test]
    fn renders_turn_steps_minimizes_with_ctrl_o_and_renders_tree_selector() {
        use gestalt_kernel::{
            Effect, EffectEnvelope, NodePayload, SESSION_SCHEMA_VERSION, SessionTree,
            SideEffectClass, StepResult, TurnStatus, Usage,
        };

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
                prompt: json!("run cargo check"),
                artifact_snapshots: std::collections::BTreeMap::new(),
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
                    effect_id: "e_bash".into(),
                    effect: Effect::Tool {
                        name: "bash".into(),
                        args: json!({ "command": "cargo check" }),
                        side_effect: SideEffectClass::Irreversible,
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
                effect_id: "e_bash".into(),
                output: json!({ "exit_code": 0, "stdout": "Checking gestalt v0.1.0\nFinished", "stderr": "" }),
                is_error: false,
                blob_digest: None,
            }),
        )
        .expect("n3");
        tree.append(
            "n4".into(),
            Some("t1".into()),
            None,
            5,
            &NodePayload::TurnEnd {
                turn_id: "t1".into(),
                status: TurnStatus::Completed,
                output: json!({ "text": "All good." }),
                usage: Usage {
                    input_units: 100,
                    output_units: 25,
                    cost_micro_usd: 420,
                },
            },
        )
        .expect("n4");

        let mut state = TuiAppState::new("repo", "general");
        state.hydrate_from_tree(&tree);
        let reg = TuiComponentRegistry::default();
        let backend = TestBackend::new(100, 26);
        let mut terminal = Terminal::new(backend).expect("term");

        terminal
            .draw(|f| render_tui(f, &mut state, &reg))
            .expect("draw expanded");
        let buf = terminal.backend().buffer();
        let mut screen = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                screen.push_str(buf[(x, y)].symbol());
            }
            screen.push('\n');
        }
        assert!(screen.contains("[Turn 1] > run cargo check"));
        assert!(screen.contains("[-] Step 1: $ cargo check [OK]"));
        assert!(screen.contains("Checking gestalt v0.1.0"));
        assert!(screen.contains("--- Turn completed | 1 step(s) | in:100 out:25"));

        // Ctrl+O minimizes step body
        state.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        terminal
            .draw(|f| render_tui(f, &mut state, &reg))
            .expect("draw collapsed");
        let buf2 = terminal.backend().buffer();
        let mut screen2 = String::new();
        for y in 0..buf2.area.height {
            for x in 0..buf2.area.width {
                screen2.push_str(buf2[(x, y)].symbol());
            }
            screen2.push('\n');
        }
        assert!(screen2.contains("[+] Step 1: $ cargo check [OK]"));

        // Open TreeSelector and trigger Rewind (last visible node in Default mode is n3)
        state.open_tree_selector(&tree);
        terminal
            .draw(|f| render_tui(f, &mut state, &reg))
            .expect("draw tree");
        let eff = state.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(eff, TuiEffect::RewindToNode(ref id) if id == "n3"));
    }
}
