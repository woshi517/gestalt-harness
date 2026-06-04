use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

pub fn draw_help_modal(f: &mut Frame, area: Rect) {
    let popup_area = centered_rect(60, 45, area);
    f.render_widget(Clear, popup_area);

    let help_text = vec![
        Line::from(vec![Span::styled(
            " Keyboard Shortcuts Guide ",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  F1 / Ctrl+H      ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Toggle this Help Modal"),
        ]),
        Line::from(vec![
            Span::styled(
                "  F2 / Ctrl+S      ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Toggle Session Switcher"),
        ]),
        Line::from(vec![
            Span::styled(
                "  F3 / Ctrl+O      ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Toggle Configuration Drawer"),
        ]),
        Line::from(vec![
            Span::styled(
                "  F4 / Ctrl+L      ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Toggle Diagnostics Log Drawer"),
        ]),
        Line::from(vec![
            Span::styled(
                "  Tab              ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Toggle Lineage Tree Sidebar (width >= 80)"),
        ]),
        Line::from(vec![
            Span::styled(
                "  Esc              ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Return focus to Chat Prompt"),
        ]),
        Line::from(vec![
            Span::styled(
                "  Ctrl+C           ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Exit the application"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            " Navigation: ",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        )]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  Up / Down Arrows ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Scroll active panel or navigate listings"),
        ]),
        Line::from(vec![
            Span::styled(
                "  Left Arrow       ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": Return focus to Chat Prompt from drawers"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            " Press any key to close this guide. ",
            Style::default()
                .add_modifier(Modifier::ITALIC)
                .fg(Color::DarkGray),
        )]),
    ];

    let paragraph = Paragraph::new(help_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(" Help "),
    );

    f.render_widget(paragraph, popup_area);
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
