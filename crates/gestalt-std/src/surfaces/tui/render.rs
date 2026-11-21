#![forbid(unsafe_code)]
//! Pi-inspired `ratatui` frame rendering (`ScrollView` + `Dock` + `Overlay` stack).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use serde_json::{Value, json};

use crate::surfaces::tui::components::{DockSlot, TuiComponentRegistry};
use crate::surfaces::tui::state::{ActiveOverlay, TranscriptItem, TuiAppState};
use gestalt_kernel::PauseReason;

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
        let empty = vec![
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
            Line::from(Span::styled(
                "Type a prompt or '/' for commands (/mode, /steer, /goal, /fork, /rewind, /gate).",
                Style::default().fg(Color::Gray),
            )),
            Line::from(Span::styled(
                "Shortcuts: [Ctrl+P] Panels  [Ctrl+R] Toggle Thinking  [Ctrl+K] Compact  [Ctrl+C] Abort/Quit",
                Style::default().fg(Color::DarkGray),
            )),
        ];
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
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("[RUN {spin}] "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "Working ({})... [Ctrl+C] Abort  [Enter] Steer",
                    state.active_preset
                ),
                Style::default().fg(Color::Gray),
            ),
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
    let left = format!(
        " {} | preset: {} | steps: {} tokens: {} (${:.4})",
        state.workspace_label,
        state.active_preset,
        state.budget.steps_used,
        state.budget.tokens_used,
        cost_usd
    );
    let right = "[Ctrl+P] Panels  [Ctrl+R] Thinking ";
    let line = Line::from(vec![
        Span::styled(left, Style::default().fg(Color::Gray)),
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
    let box_w = screen.width.clamp(24, 72);
    let box_h = screen.height.clamp(8, 18);
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
                body_lines.push(Line::from(format!("Details: {payload}")));
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
    }
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
        let mut state = TuiAppState::new("repo", "swe");
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
        let mut state = TuiAppState::new("repo", "swe");
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
}
