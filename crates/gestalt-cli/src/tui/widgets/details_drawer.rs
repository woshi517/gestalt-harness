use crate::config::EffectiveConfig;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

#[allow(clippy::cast_possible_truncation)]
pub fn draw_details_drawer(
    f: &mut Frame,
    area: Rect,
    config: Option<&EffectiveConfig>,
    scroll_offset: usize,
    is_focused: bool,
) {
    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if is_focused {
        " Effective Config (Focused) "
    } else {
        " Effective Config "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let mut lines = Vec::new();
    if let Some(cfg) = config {
        if let Ok(json_str) = serde_json::to_string_pretty(cfg) {
            for line in json_str.lines() {
                // Highlight keys vs values
                if line.contains(':') {
                    let parts: Vec<&str> = line.splitn(2, ':').collect();
                    lines.push(Line::from(vec![
                        Span::styled(parts[0].to_string(), Style::default().fg(Color::LightCyan)),
                        Span::styled(":", Style::default().fg(Color::White)),
                        Span::styled(parts[1].to_string(), Style::default().fg(Color::White)),
                    ]));
                } else {
                    lines.push(Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(Color::White),
                    )));
                }
            }
        } else {
            lines.push(Line::from(Span::styled(
                "Failed to serialize configuration.",
                Style::default().fg(Color::Red),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No configuration loaded.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset as u16, 0));

    f.render_widget(paragraph, area);
}
