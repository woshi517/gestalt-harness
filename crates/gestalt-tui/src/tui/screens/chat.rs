use crate::tui::bridge::get_diagnostics_logs;
use crate::tui::state::{TuiAppState, TuiFocus, TuiModal};
use crate::tui::widgets::{
    approval_popup::draw_approval_popup, details_drawer::draw_details_drawer,
    diagnostics_drawer::draw_diagnostics_drawer, event_log::draw_event_log,
    help_modal::draw_help_modal, lineage_tree::draw_lineage_tree,
    onboarding_popup::draw_onboarding_popup, session_switcher::draw_session_switcher,
    status_bar::draw_status_bar,
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

#[allow(clippy::too_many_lines)]
pub fn draw_chat_screen(f: &mut Frame, state: &TuiAppState) {
    let width = f.area().width;
    let height = f.area().height;

    // Viewport responsive policy: check minimum size limits
    if width < 60 || height < 15 {
        let warning_para = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "⚠ Viewport too small",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("Current: {width}x{height}")),
            Line::from("Please resize your terminal (Min: 60x15)"),
        ])
        .alignment(Alignment::Center);
        f.render_widget(warning_para, f.area());
        return;
    }

    let input_height = if state.chat.input_buffer.starts_with('/') {
        12
    } else {
        3
    };

    // 1. Root Vertical Split: panels vs status vs input prompt
    let root_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),               // Top Panels area
            Constraint::Length(3),            // Status Bar
            Constraint::Length(input_height), // Prompt Input
        ])
        .split(f.area());

    // 2. Horizontal split for top panels
    let sidebar_visible = state.chrome.sidebar_open && width >= 80;

    let top_constraints = if sidebar_visible {
        vec![
            Constraint::Percentage(25), // Sidebar width
            Constraint::Min(10),        // Main area width
        ]
    } else {
        vec![
            Constraint::Min(10), // Main area width
        ]
    };

    let top_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(top_constraints)
        .split(root_chunks[0]);

    let (sidebar_area, main_panel_area) = if sidebar_visible {
        (Some(top_chunks[0]), top_chunks[1])
    } else {
        (None, top_chunks[0])
    };

    // 3. Split main panel if details or diagnostics drawers are open
    let details_visible = state.chrome.details_open;
    let diagnostics_visible = state.chrome.diagnostics_open;

    let main_content_area = if details_visible || diagnostics_visible {
        let main_split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(55), // Left: Chat history
                Constraint::Percentage(45), // Right: Drawers
            ])
            .split(main_panel_area);

        let chat_area = main_split[0];
        let drawers_area = main_split[1];

        // Split drawers vertically if both are open
        if details_visible && diagnostics_visible {
            let drawer_split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(drawers_area);

            draw_details_drawer(
                f,
                drawer_split[0],
                state.details.config.as_ref(),
                state.details.scroll_offset,
                state.chrome.active_focus == TuiFocus::Details,
            );

            draw_diagnostics_drawer(
                f,
                drawer_split[1],
                &get_diagnostics_logs(),
                state.diagnostics.scroll_offset,
                state.chrome.active_focus == TuiFocus::Diagnostics,
            );
        } else if details_visible {
            draw_details_drawer(
                f,
                drawers_area,
                state.details.config.as_ref(),
                state.details.scroll_offset,
                state.chrome.active_focus == TuiFocus::Details,
            );
        } else {
            draw_diagnostics_drawer(
                f,
                drawers_area,
                &get_diagnostics_logs(),
                state.diagnostics.scroll_offset,
                state.chrome.active_focus == TuiFocus::Diagnostics,
            );
        }

        chat_area
    } else {
        main_panel_area
    };

    // 4. Draw Lineage tree sidebar if visible
    if let Some(area) = sidebar_area {
        draw_lineage_tree(
            f,
            area,
            state.lineage.model.as_ref(),
            state.lineage.selected_index,
            state.chrome.active_focus == TuiFocus::LineageTree,
        );
    }

    // 5. Draw Chat history event log
    draw_event_log(
        f,
        main_content_area,
        &state.chat.events,
        state.chat.scroll_offset,
        state.chrome.active_focus == TuiFocus::ChatPrompt,
    );

    // 6. Draw status bar and input prompt
    let system_mode = state
        .config
        .defaults
        .mode
        .map_or("confirm", gestalt_core::ExecutionMode::as_str);
    let session_title = if state.has_started_session() {
        crate::tui::widgets::event_log::derive_session_title(&state.chat.events, &state.session_id)
            .trim()
            .trim_start_matches("Session:")
            .trim()
            .to_string()
    } else {
        "New session".to_string()
    };
    draw_status_bar(
        f,
        root_chunks[1],
        root_chunks[2],
        &state.status,
        state.is_running,
        &state.chat.input_buffer,
        state.chrome.active_focus,
        state.chrome.sidebar_open,
        width,
        state.chat.autocomplete_index,
        system_mode,
        &session_title,
        state.has_started_session(),
    );

    // 7. Overlay Modals
    match state.chrome.active_modal {
        TuiModal::Help => {
            draw_help_modal(f, f.area());
        }
        TuiModal::SessionSwitcher => {
            draw_session_switcher(
                f,
                f.area(),
                state.switcher.model.as_ref(),
                state.switcher.selected_index,
            );
        }
        TuiModal::Approval => {
            if let Some(ref req) = state.approval.active_request {
                draw_approval_popup(f, f.area(), req);
            }
        }
        TuiModal::Onboarding => {
            draw_onboarding_popup(
                f,
                f.area(),
                &state.onboarding.providers,
                state.onboarding.selected_idx,
                &state.onboarding.api_key,
                state.onboarding.is_key_focused,
                state.onboarding.error_message.as_deref(),
            );
        }
        TuiModal::Notification => {
            if let Some(notification) = state.notification.as_ref() {
                crate::tui::widgets::notification_popup::draw_notification_popup(
                    f,
                    f.area(),
                    &notification.title,
                    &notification.message,
                    notification.is_error,
                );
            }
        }
        TuiModal::None => {}
    }
}
