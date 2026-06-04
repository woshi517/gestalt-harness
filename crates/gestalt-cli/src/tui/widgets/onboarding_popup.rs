use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

pub fn draw_onboarding_popup(
    f: &mut Frame,
    area: Rect,
    providers: &[String],
    selected_idx: usize,
    api_key: &str,
    is_key_focused: bool,
    error_message: Option<&str>,
) {
    let popup_area = centered_rect(75, 65, area);
    f.render_widget(Clear, popup_area);

    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // Header
            Constraint::Min(5),    // Provider Selector
            Constraint::Length(4), // Key Input
            Constraint::Length(2), // Error Area
            Constraint::Length(2), // Footer Help
        ])
        .split(popup_area);

    // 1. Header Block
    let header_text = vec![
        Line::from(vec![Span::styled(
            " GESTALT LLM CONNECTION WIZARD ",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        )]),
        Line::from("Welcome! To get started, connect a local or cloud LLM provider below."),
    ];
    let header_para = Paragraph::new(header_text).alignment(Alignment::Center);
    f.render_widget(header_para, vertical_chunks[0]);

    // 2. Providers List
    let mut list_items = Vec::new();
    for (idx, provider) in providers.iter().enumerate() {
        let is_selected = idx == selected_idx;
        let prefix = if is_selected { " ▶ " } else { "   " };

        let provider_label = match provider.as_str() {
            "openrouter" => "OpenRouter (Recommended - Free & Paid cloud models)",
            "openai" => "OpenAI (GPT-4o, GPT-4, etc.)",
            "anthropic" => "Anthropic (Claude 3.5 Sonnet, etc.)",
            "gemini" => "Google Gemini (Gemini 1.5 Pro, Flash, etc.)",
            "groq" => "Groq (Fast open models Llama-3, etc.)",
            "ollama" => "Ollama (Run local models offline - No API Key required)",
            _ => provider.as_str(),
        };

        let style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let item_style = if is_selected && !is_key_focused {
            Style::default().bg(Color::Rgb(40, 40, 40))
        } else {
            Style::default()
        };

        list_items.push(
            ListItem::new(Line::from(vec![
                Span::styled(prefix, Style::default().fg(Color::Cyan)),
                Span::styled(provider_label, style),
            ]))
            .style(item_style),
        );
    }

    let list_border_style = if is_key_focused {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::Cyan)
    };

    let list = List::new(list_items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(list_border_style)
            .title(" Select Provider "),
    );
    f.render_widget(list, vertical_chunks[1]);

    // 3. API Key Input Box
    let is_ollama = providers[selected_idx] == "ollama";
    let input_title = if is_ollama {
        " API Key (Not Required for Ollama) "
    } else if is_key_focused {
        " Enter API Key (Focused - Press Enter to Save) "
    } else {
        " API Key (Press Tab to focus input) "
    };

    let masked_key = if is_ollama {
        "Ollama will connect directly to http://localhost:11434".to_string()
    } else if api_key.is_empty() {
        "".to_string()
    } else {
        "*".repeat(api_key.len())
    };

    let input_border_style = if is_key_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let key_para = Paragraph::new(masked_key).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(input_border_style)
            .title(input_title),
    );
    f.render_widget(key_para, vertical_chunks[2]);

    // 4. Error Message Area
    if let Some(err) = error_message {
        let err_line = Line::from(vec![
            Span::styled(
                "⚠ Error: ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(err, Style::default().fg(Color::Red)),
        ]);
        let err_para = Paragraph::new(err_line).alignment(Alignment::Center);
        f.render_widget(err_para, vertical_chunks[3]);
    }

    // 5. Help Footer
    let footer_text = if is_key_focused {
        "Press [Tab] to change provider | [Enter] to Save and Connect | [Esc] to exit"
    } else {
        "Use [Up/Down] to select provider | [Enter/Tab] to edit API key | [Esc] to exit"
    };
    let footer_para = Paragraph::new(Span::styled(
        footer_text,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    ))
    .alignment(Alignment::Center);
    f.render_widget(footer_para, vertical_chunks[4]);

    // Main Outer Block Border
    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" LLM Provider Connection Setup ");
    f.render_widget(outer_block, popup_area);
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
