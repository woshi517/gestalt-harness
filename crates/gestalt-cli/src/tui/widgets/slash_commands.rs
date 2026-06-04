use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn draw_slash_autocomplete(f: &mut Frame, input_buffer: &str, area_input: Rect) {
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
    ];

    let typed = input_buffer.to_lowercase();
    let filtered_cmds: Vec<_> = commands
        .iter()
        .filter(|(cmd, _)| cmd.starts_with(&typed))
        .collect();

    if filtered_cmds.is_empty() {
        return;
    }

    // Determine popup area directly above input bar
    let popup_height = (filtered_cmds.len() as u16 + 2).min(10);
    if area_input.y < popup_height {
        return; // Not enough space
    }

    let popup_area = Rect::new(
        area_input.x,
        area_input.y.saturating_sub(popup_height),
        area_input.width,
        popup_height,
    );

    let mut lines = Vec::new();
    for (cmd, desc) in filtered_cmds {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<15}", cmd),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" - {}", desc), Style::default().fg(Color::DarkGray)),
        ]));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Slash Commands ");

    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, popup_area);
}
