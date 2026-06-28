use gestalt_core::approval::ApprovalRequest;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

pub fn draw_approval_popup(f: &mut Frame, area: Rect, request: &ApprovalRequest) {
    let popup_area = centered_rect(70, 60, area);
    f.render_widget(Clear, popup_area);

    let pretty_input = serde_json::to_string_pretty(&request.input)
        .unwrap_or_else(|_| format!("{:?}", request.input));

    let mut popup_text = vec![
        Line::from(vec![
            Span::styled("Tool Name: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                &request.tool_name,
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Description: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(&request.description, Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Input Parameters: ",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
    ];

    // Add JSON input parameters line-by-line
    for line in pretty_input.lines() {
        popup_text.push(Line::from(Span::styled(
            format!("  {line}"),
            Style::default().fg(Color::LightCyan),
        )));
    }

    popup_text.extend(vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Risk/Policy: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{:?}", request.decision),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "Action Required: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw("Approve [a/y], Deny [d/n/Esc/c]"),
        ]),
    ]);

    let popup_para = Paragraph::new(popup_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(" Tool Authorization Required "),
        )
        .wrap(Wrap { trim: true });

    f.render_widget(popup_para, popup_area);
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
