use crate::tui::state::TranscriptEntry;
use ratatui::{
    layout::Alignment,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

const LOGO: &str = include_str!("../../../../../assets/tui-icon.txt");

pub fn derive_session_title(events: &[TranscriptEntry], default_title: &str) -> String {
    for entry in events {
        if let TranscriptEntry::User(content) = entry {
            let first_line = content.lines().next().unwrap_or("").trim();
            if !first_line.is_empty() {
                let mut title = first_line.chars().take(40).collect::<String>();
                if first_line.chars().count() > 40 {
                    title.push_str("...");
                }
                return format!(" Session: {} ", title);
            }
        }
    }
    format!(" {} ", default_title)
}

pub fn draw_event_log(
    f: &mut Frame,
    area: Rect,
    events: &[TranscriptEntry],
    scroll_offset: usize,
    is_focused: bool,
) {
    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let default_title = if is_focused {
        "Active Session (Focused)"
    } else {
        "Active Session"
    };

    let mut title = derive_session_title(events, default_title);
    if is_focused && !title.contains("(Focused)") {
        title = format!("{} (Focused) ", title.trim_end());
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    if events.is_empty() {
        // Draw centered logo splash screen
        let logo_lines: Vec<&str> = LOGO.lines().collect();
        let total_lines = logo_lines.len();

        let inner_area = block.inner(area);
        let vertical_pad = (inner_area.height.saturating_sub(total_lines as u16)) / 2;

        let mut paragraph_lines = Vec::new();
        for _ in 0..vertical_pad {
            paragraph_lines.push(Line::from(""));
        }
        for line in logo_lines {
            paragraph_lines.push(Line::from(Span::styled(
                line,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let paragraph = Paragraph::new(paragraph_lines)
            .block(block)
            .alignment(Alignment::Center);

        f.render_widget(paragraph, area);
    } else {
        // Render events transcript
        let mut lines = Vec::new();
        for ev in events {
            lines.extend(format_transcript_entry(ev));
        }

        // Apply scroll offset
        // In chat, scroll_offset scroll up from bottom. So if scroll_offset is 0,
        // we want to display the bottom-most lines.
        let inner_area = block.inner(area);
        let height = inner_area.height as usize;
        let total_lines = lines.len();

        let max_scroll = total_lines.saturating_sub(height);
        let scroll_y = max_scroll.saturating_sub(scroll_offset);

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: true })
            .scroll((scroll_y as u16, 0));

        f.render_widget(paragraph, area);
    }
}

fn split_and_format_block(
    prefix: &'static str,
    text: &str,
    prefix_style: Style,
    text_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut is_first = true;
    let indent = " ".repeat(prefix.len());

    if text.is_empty() {
        return vec![Line::from(vec![Span::styled(prefix, prefix_style)])];
    }

    let parts: Vec<&str> = text.split('\n').collect();
    for part in &parts {
        let mut line_spans = Vec::new();
        if is_first {
            line_spans.push(Span::styled(prefix, prefix_style));
            is_first = false;
        } else {
            line_spans.push(Span::styled(indent.clone(), prefix_style));
        }
        line_spans.push(Span::styled((*part).to_string(), text_style));
        lines.push(Line::from(line_spans));
    }
    lines
}

fn format_transcript_entry(entry: &TranscriptEntry) -> Vec<Line<'static>> {
    match entry {
        TranscriptEntry::User(content) => {
            let mut user_lines = split_and_format_block(
                "User> ",
                content,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(Color::White),
            );
            user_lines.push(Line::from(""));
            user_lines
        }
        TranscriptEntry::Agent(content) => split_and_format_block(
            "Agent> ",
            content,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            Style::default().fg(Color::LightGreen),
        ),
        TranscriptEntry::Thinking(content) => split_and_format_block(
            "Thinking: ",
            content,
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::DarkGray),
        ),
        TranscriptEntry::System(content) => split_and_format_block(
            "System: ",
            content,
            Style::default().fg(Color::DarkGray),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        ),
        TranscriptEntry::ModelRequest { model } => vec![Line::from(vec![
            Span::styled("Model Request: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                model.clone(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
        ])],
        TranscriptEntry::ToolCall { name } => vec![Line::from(vec![
            Span::styled(
                "Tool proposed: ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(name.clone(), Style::default().fg(Color::Yellow)),
        ])],
        TranscriptEntry::ToolResult { name, is_error } => {
            let color = if *is_error { Color::Red } else { Color::Green };
            vec![Line::from(vec![
                Span::styled("Tool result (", Style::default().fg(color)),
                Span::styled(
                    name.clone(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(") [Error: {}]", is_error),
                    Style::default().fg(color),
                ),
            ])]
        }
        TranscriptEntry::Checkpoint => vec![Line::from(Span::styled(
            "✓ Checkpoint saved",
            Style::default().fg(Color::Green),
        ))],
        TranscriptEntry::Interrupted(reason) => vec![Line::from(vec![
            Span::styled(
                "⚠ Interrupted: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(reason.clone(), Style::default().fg(Color::Red)),
        ])],
        TranscriptEntry::Stop(reason) => {
            let mut stop_lines = split_and_format_block(
                "Stop: ",
                reason,
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(Color::Blue),
            );
            stop_lines.push(Line::from(""));
            stop_lines
        }
        TranscriptEntry::Error(message) => {
            let mut err_lines = split_and_format_block(
                "Error: ",
                message,
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                Style::default().fg(Color::Red),
            );
            err_lines.push(Line::from(""));
            err_lines
        }
        TranscriptEntry::Policy { tool_name, risk } => vec![Line::from(vec![
            Span::styled("Policy decision for: ", Style::default().fg(Color::Magenta)),
            Span::styled(
                tool_name.clone(),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" (Risk: {})", risk),
                Style::default().fg(Color::Magenta),
            ),
        ])],
        TranscriptEntry::Other(text) => vec![Line::from(Span::styled(
            text.clone(),
            Style::default().fg(Color::DarkGray),
        ))],
    }
}
