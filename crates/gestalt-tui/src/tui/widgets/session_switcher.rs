use crate::tui::services::SessionListModel;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState},
    Frame,
};

pub fn draw_session_switcher(
    f: &mut Frame,
    area: Rect,
    model: Option<&SessionListModel>,
    selected_index: usize,
) {
    let popup_area = centered_rect(80, 50, area);
    f.render_widget(Clear, popup_area);

    let mut items = Vec::new();

    // Headers
    items.push(ListItem::new(Line::from(vec![Span::styled(
        format!(
            "{:<35} | {:<20} | {:<5} | {:<8}",
            "SESSION TITLE", "CREATED AT", "TURNS", "EST. COST"
        ),
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    )])));
    items.push(ListItem::new(Line::from("-".repeat(78))));

    if let Some(list) = model {
        for (idx, session) in list.sessions.iter().enumerate() {
            let is_selected = idx == selected_index;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let created_str = session.created_at.map_or_else(
                || "unknown".to_string(),
                |t| t.format("%Y-%m-%d %H:%M:%S").to_string(),
            );

            let display_title = if session.title.len() > 33 {
                format!("{}...", &session.title[..30])
            } else {
                session.title.clone()
            };

            let mut spans = vec![Span::styled(
                format!(
                    "{:<35} | {:<20} | {:<5} | ${:<7.4}",
                    display_title, created_str, session.total_turns, session.estimated_cost_usd
                ),
                style,
            )];

            if is_selected {
                spans.push(Span::styled(" ◀", Style::default().fg(Color::Yellow)));
            }

            let item_style = if is_selected {
                Style::default().bg(Color::Rgb(40, 40, 40))
            } else {
                Style::default()
            };

            items.push(ListItem::new(Line::from(spans)).style(item_style));
        }
    }

    let has_sessions = model.is_some_and(|m| !m.sessions.is_empty());

    if items.len() <= 2 {
        items.push(ListItem::new(Line::from(vec![Span::styled(
            "No sessions found.",
            Style::default().fg(Color::DarkGray),
        )])));
    }

    let list_widget = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Session Switcher (Press Enter to Select, Esc to Close) "),
    );

    let mut list_state = ListState::default();
    if has_sessions {
        list_state.select(Some(selected_index + 2));
    }
    f.render_stateful_widget(list_widget, popup_area, &mut list_state);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
