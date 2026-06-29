use crate::tui::services::LineageTreeModel;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

pub fn draw_lineage_tree(
    f: &mut Frame,
    area: Rect,
    model: Option<&LineageTreeModel>,
    selected_index: usize,
    is_focused: bool,
) {
    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if is_focused {
        " Session Lineage (Focused) "
    } else {
        " Session Lineage "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let mut items = Vec::new();

    if let Some(tree) = model {
        for (idx, node) in tree.nodes.iter().enumerate() {
            let is_selected = idx == selected_index;

            // Build connector prefix
            let connector = if node.depth == 0 {
                ""
            } else if node.is_last_child {
                "└── "
            } else {
                "├── "
            };

            let prefix_spans = vec![Span::raw(node.prefix.clone()), Span::raw(connector)];

            // Render run identifier
            let short_id = if node.run_id.len() > 12 {
                &node.run_id[..12]
            } else {
                &node.run_id
            };

            let node_style = if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let status_color = match node.lifecycle_state.as_str() {
                "completed" => Color::Green,
                "running" => Color::Cyan,
                "failed" => Color::Red,
                _ => Color::DarkGray,
            };

            let mut spans = prefix_spans;
            spans.push(Span::styled(format!("● {short_id}"), node_style));
            let turns = node.turns;
            spans.push(Span::raw(format!(" ({turns}t) [")));
            spans.push(Span::styled(
                &node.lifecycle_state,
                Style::default().fg(status_color),
            ));
            spans.push(Span::raw("]"));

            if is_selected {
                spans.push(Span::styled(" ◀", Style::default().fg(Color::Yellow)));
            }

            let item_style = if is_selected && is_focused {
                Style::default().bg(Color::Rgb(40, 40, 40))
            } else {
                Style::default()
            };

            items.push(ListItem::new(Line::from(spans)).style(item_style));
        }
    }

    if items.is_empty() {
        items.push(ListItem::new(Line::from(vec![Span::styled(
            "No lineage records found.",
            Style::default().fg(Color::DarkGray),
        )])));
    }

    let mut list_state = ListState::default();
    if model.is_some() && !items.is_empty() {
        list_state.select(Some(selected_index));
    }

    let list = List::new(items).block(block);
    f.render_stateful_widget(list, area, &mut list_state);
}
