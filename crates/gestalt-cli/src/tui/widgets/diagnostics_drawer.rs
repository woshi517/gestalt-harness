use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

#[allow(clippy::cast_possible_truncation)]
pub fn draw_diagnostics_drawer(
    f: &mut Frame,
    area: Rect,
    logs: &[String],
    scroll_offset: usize,
    is_focused: bool,
) {
    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if is_focused {
        " Diagnostics Logs (Focused) "
    } else {
        " Diagnostics Logs "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let mut lines = Vec::new();
    for log in logs {
        let mut spans = Vec::new();
        // Simple log level highlighting
        if log.contains("[INFO]") {
            spans.push(Span::styled(
                "[INFO]",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(
                log.split_once("[INFO]").map_or("", |x| x.1).to_string(),
            ));
        } else if log.contains("[WARN]") {
            spans.push(Span::styled(
                "[WARN]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(
                log.split_once("[WARN]").map_or("", |x| x.1).to_string(),
            ));
        } else if log.contains("[ERROR]") {
            spans.push(Span::styled(
                "[ERROR]",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(
                log.split_once("[ERROR]").map_or("", |x| x.1).to_string(),
            ));
        } else if log.contains("[DEBUG]") {
            spans.push(Span::styled("[DEBUG]", Style::default().fg(Color::Blue)));
            spans.push(Span::raw(
                log.split_once("[DEBUG]").map_or("", |x| x.1).to_string(),
            ));
        } else if log.contains("[TRACE]") {
            spans.push(Span::styled("[TRACE]", Style::default().fg(Color::Magenta)));
            spans.push(Span::raw(
                log.split_once("[TRACE]").map_or("", |x| x.1).to_string(),
            ));
        } else {
            spans.push(Span::raw(log.clone()));
        }
        lines.push(Line::from(spans));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No diagnostic logs captured yet.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true })
        .scroll((scroll_offset as u16, 0));

    f.render_widget(paragraph, area);
}
