use crate::tui::state::TuiFocus;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

#[allow(clippy::cast_possible_truncation)]
pub fn draw_status_bar(
    f: &mut Frame,
    area_status: Rect,
    area_input: Rect,
    status: &str,
    is_running: bool,
    input_buffer: &str,
    active_focus: TuiFocus,
    sidebar_open: bool,
    width: u16,
    autocomplete_index: usize,
    system_mode: &str,
    session_title: &str,
    has_started_session: bool,
) {
    // 1. Render Status Bar
    let status_style = if status.contains("Awaiting") {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else if status.contains("Completed") {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else if status.contains("Failed") {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else if status.contains("Running") {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD)
    };

    let status_display = truncate_for_status_bar(status, if width < 80 { 16 } else { 42 });

    let lineage_hint = if width < 80 {
        Span::styled(
            "Tab: lineage unavailable",
            Style::default().fg(Color::DarkGray),
        )
    } else if sidebar_open {
        Span::styled("Tab: hide lineage", Style::default().fg(Color::Cyan))
    } else {
        Span::styled("Tab: show lineage", Style::default().fg(Color::DarkGray))
    };

    let help_hint = if status.contains("Awaiting") {
        "a: Approve | d: Deny"
    } else if is_running {
        "Esc/Ctrl+C: Interrupt"
    } else {
        "F1: Help | F2: Switcher | F3: Config | F4: Logs"
    };

    let mode_span = Span::styled(
        format!("Mode: {}", system_mode.to_uppercase()),
        Style::default()
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    );

    let session_span = Span::styled(
        format!("Session: {}", session_title),
        Style::default().fg(Color::LightCyan),
    );

    let status_line = if width < 80 {
        Line::from(vec![
            Span::raw("Status: "),
            Span::styled(status_display.clone(), status_style),
            Span::raw("  |  "),
            mode_span,
            Span::raw("  |  "),
            lineage_hint,
        ])
    } else if width < 120 {
        let short_session = if session_title.len() > 12 {
            format!("{}...", &session_title[0..9])
        } else {
            session_title.to_string()
        };
        let short_session_span = Span::styled(
            format!("Session: {}", short_session),
            Style::default().fg(Color::LightCyan),
        );
        Line::from(vec![
            Span::raw("Status: "),
            Span::styled(status_display.clone(), status_style),
            Span::raw("  |  "),
            mode_span,
            Span::raw("  |  "),
            short_session_span,
            Span::raw("  |  "),
            lineage_hint,
        ])
    } else {
        Line::from(vec![
            Span::raw("Status: "),
            Span::styled(status_display, status_style),
            Span::raw("  |  "),
            mode_span,
            Span::raw("  |  "),
            session_span,
            Span::raw("  |  "),
            lineage_hint,
            Span::raw("  |  "),
            Span::raw(help_hint),
        ])
    };

    let status_block = Block::default()
        .borders(Borders::ALL)
        .title(" System Status ");

    let status_para = Paragraph::new(status_line).block(status_block);
    f.render_widget(status_para, area_status);

    // 2. Render Prompt Input Bar
    let is_input_focused = active_focus == TuiFocus::ChatPrompt;
    let input_style = if is_running {
        Style::default().fg(Color::DarkGray)
    } else if is_input_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let input_mode_hint = if input_buffer.starts_with("/branch") {
        " [EXPLICIT BRANCH MODE] "
    } else if has_started_session {
        " [CONTINUE CHAT MODE] "
    } else {
        " [NEW SESSION MODE] "
    };

    let input_title = if is_running {
        format!(" Prompt Input{} (Locked during run) ", input_mode_hint)
    } else if is_input_focused {
        format!(
            " Prompt Input{} (Focused - Press Enter to submit) ",
            input_mode_hint
        )
    } else {
        format!(" Prompt Input{} (Press Esc to focus) ", input_mode_hint)
    };

    let input_para = if input_buffer.starts_with('/') {
        let mut lines = vec![Line::from(input_buffer)];
        lines.push(Line::from(""));

        let commands = vec![
            ("/help", "Show keyboard shortcuts and general help"),
            ("/new", "Start a fresh session on the next prompt"),
            ("/quit", "Exit the chat session"),
            ("/exit", "Exit the chat session"),
            (
                "/mode <mode>",
                "Change execution mode (confirm, yolo, human, dry-run, replay)",
            ),
            ("/cost", "Show the aggregated cost of this session"),
            ("/context", "Explain the context pipeline of the latest run"),
            ("/runs", "Display the lineage tree of runs"),
            (
                "/branch <prompt>",
                "Branch the session from the selected run",
            ),
            ("/config", "Toggle the configuration/details drawer"),
            (
                "/export <format>",
                "Export the latest run's trace (markdown, jsonl)",
            ),
            ("/verify", "Run verifiers on the latest run's artifacts"),
        ];

        let typed = input_buffer.to_lowercase();
        let typed_cmd = typed.split_whitespace().next().unwrap_or("");

        let filtered_cmds: Vec<_> = commands
            .iter()
            .filter(|(cmd, _)| cmd.starts_with(typed_cmd))
            .collect();

        for (idx, (cmd, desc)) in filtered_cmds.iter().enumerate() {
            let is_selected = idx == autocomplete_index;
            let style = if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
                    .bg(Color::Rgb(40, 40, 40))
            } else {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            };
            let desc_style = if is_selected {
                Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 40))
            } else {
                Style::default().fg(Color::DarkGray)
            };

            lines.push(Line::from(vec![
                Span::styled(format!("  {:<18}", cmd), style),
                Span::styled(format!(" - {}", desc), desc_style),
            ]));
        }

        Paragraph::new(lines)
    } else {
        Paragraph::new(input_buffer)
    };

    let input_para = input_para.block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(input_style)
            .title(input_title),
    );

    f.render_widget(input_para, area_input);

    // Place the cursor if input is focused and agent is not running
    if !is_running && is_input_focused {
        let cursor_x = (area_input.x + 1 + input_buffer.len() as u16)
            .min(area_input.x + area_input.width.saturating_sub(2));
        f.set_cursor_position((cursor_x, area_input.y + 1));
    }
}

fn truncate_for_status_bar(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}
