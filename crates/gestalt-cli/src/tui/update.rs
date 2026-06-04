use crate::tui::state::{push_event, TuiAppState, TuiFocus, TuiModal};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use gestalt_core::{approval::ApprovalDecision, event::AgentEvent};

pub enum TuiUiAction {
    SubmitPrompt(String),
    InterruptRun,
    ApprovalDecision {
        decision: ApprovalDecision,
    },
    SelectSession(String),
    BranchSession {
        parent_run_id: String,
        prompt: String,
    },
    LoadSessions,
    LoadLineage,
    StartNewSession,
    SaveOnboarding {
        provider: String,
        api_key: String,
    },
    ChangeMode(String),
    CalculateCost,
    Quit,
    ExplainContext(String),
    ExportRun {
        parent_run_id: String,
        format: String,
    },
    VerifyRun(String),
}

pub fn apply_agent_event(state: &mut TuiAppState, event: AgentEvent) -> Vec<TuiUiAction> {
    push_event(&mut state.chat.events, event);
    state.chat.scroll_offset = 0; // Reset scroll to bottom on new event
    Vec::new()
}

pub fn handle_key_event(state: &mut TuiAppState, key_event: KeyEvent) -> Vec<TuiUiAction> {
    if key_event.kind != crossterm::event::KeyEventKind::Press {
        return Vec::new();
    }

    // 1. Handle Active Modals
    match state.chrome.active_modal {
        TuiModal::Help => {
            // Any key closes the help modal
            state.chrome.active_modal = TuiModal::None;
            return Vec::new();
        }
        TuiModal::SessionSwitcher => {
            match key_event.code {
                KeyCode::Esc => {
                    state.chrome.active_modal = TuiModal::None;
                }
                KeyCode::Up => {
                    if let Some(ref model) = state.switcher.model {
                        if !model.sessions.is_empty() {
                            if state.switcher.selected_index > 0 {
                                state.switcher.selected_index -= 1;
                            } else {
                                state.switcher.selected_index = model.sessions.len() - 1;
                            }
                        }
                    }
                }
                KeyCode::Down => {
                    if let Some(ref model) = state.switcher.model {
                        if !model.sessions.is_empty() {
                            if state.switcher.selected_index + 1 < model.sessions.len() {
                                state.switcher.selected_index += 1;
                            } else {
                                state.switcher.selected_index = 0;
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Some(ref model) = state.switcher.model {
                        if state.switcher.selected_index < model.sessions.len() {
                            let session_id = model.sessions[state.switcher.selected_index]
                                .session_id
                                .clone();
                            state.chrome.active_modal = TuiModal::None;
                            return vec![TuiUiAction::SelectSession(session_id)];
                        }
                    }
                    state.chrome.active_modal = TuiModal::None;
                }
                _ => {}
            }
            return Vec::new();
        }
        TuiModal::Approval => {
            match key_event.code {
                KeyCode::Char('a') | KeyCode::Char('y') => {
                    state.chrome.active_modal = TuiModal::None;
                    state.approval.active_request = None;
                    return vec![TuiUiAction::ApprovalDecision {
                        decision: ApprovalDecision::Approve,
                    }];
                }
                KeyCode::Char('d') | KeyCode::Char('n') | KeyCode::Char('c') | KeyCode::Esc => {
                    state.chrome.active_modal = TuiModal::None;
                    state.approval.active_request = None;
                    return vec![TuiUiAction::ApprovalDecision {
                        decision: ApprovalDecision::Deny,
                    }];
                }
                _ => {}
            }
            return Vec::new();
        }
        TuiModal::Onboarding => {
            match key_event.code {
                KeyCode::Esc => {
                    return vec![TuiUiAction::Quit];
                }
                KeyCode::Tab => {
                    state.onboarding.is_key_focused = !state.onboarding.is_key_focused;
                }
                KeyCode::Up => {
                    if !state.onboarding.is_key_focused {
                        if state.onboarding.selected_idx > 0 {
                            state.onboarding.selected_idx -= 1;
                        } else {
                            state.onboarding.selected_idx = state.onboarding.providers.len() - 1;
                        }
                    }
                }
                KeyCode::Down => {
                    if !state.onboarding.is_key_focused {
                        if state.onboarding.selected_idx + 1 < state.onboarding.providers.len() {
                            state.onboarding.selected_idx += 1;
                        } else {
                            state.onboarding.selected_idx = 0;
                        }
                    }
                }
                KeyCode::Backspace => {
                    if state.onboarding.is_key_focused {
                        state.onboarding.api_key.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if state.onboarding.is_key_focused {
                        state.onboarding.api_key.push(c);
                    } else if c == 'q' {
                        return vec![TuiUiAction::Quit];
                    }
                }
                KeyCode::Enter => {
                    if !state.onboarding.is_key_focused
                        && state.onboarding.providers[state.onboarding.selected_idx] != "ollama"
                    {
                        state.onboarding.is_key_focused = true;
                    } else {
                        let provider =
                            state.onboarding.providers[state.onboarding.selected_idx].clone();
                        let api_key = state.onboarding.api_key.trim().to_string();
                        return vec![TuiUiAction::SaveOnboarding { provider, api_key }];
                    }
                }
                _ => {}
            }
            return Vec::new();
        }
        TuiModal::Notification => {
            state.notification = None;
            state.chrome.active_modal = TuiModal::None;
            return Vec::new();
        }
        TuiModal::None => {}
    }

    // 2. Handle Keyboard Shortcuts / Navigation (Global Priority)
    match key_event.code {
        KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.is_running {
                return vec![TuiUiAction::InterruptRun];
            }
            return vec![TuiUiAction::Quit];
        }
        KeyCode::Tab => {
            if state.chat.input_buffer.starts_with('/') {
                let commands = vec![
                    ("/help", false),
                    ("/new", false),
                    ("/quit", false),
                    ("/exit", false),
                    ("/mode", true),
                    ("/cost", false),
                    ("/context", false),
                    ("/runs", false),
                    ("/branch", true),
                    ("/config", false),
                    ("/export", true),
                    ("/verify", false),
                ];
                let typed = state.chat.input_buffer.to_lowercase();
                let typed_cmd = typed.split_whitespace().next().unwrap_or("");
                let filtered: Vec<_> = commands
                    .iter()
                    .filter(|(cmd, _)| cmd.starts_with(typed_cmd))
                    .collect();
                if !filtered.is_empty() {
                    let idx = state.chat.autocomplete_index.min(filtered.len() - 1);
                    let (cmd, takes_args) = filtered[idx];
                    let completed = if *takes_args {
                        format!("{} ", cmd)
                    } else {
                        (*cmd).to_string()
                    };
                    state.chat.input_buffer = completed;
                    return Vec::new();
                }
            }
            if state.chrome.terminal_width < 80 {
                // R5: Lineage tree unavailable in narrow view
                return Vec::new();
            }
            state.chrome.sidebar_open = !state.chrome.sidebar_open;
            if state.chrome.sidebar_open {
                state.chrome.active_focus = TuiFocus::LineageTree;
                return vec![TuiUiAction::LoadLineage];
            } else if state.chrome.active_focus == TuiFocus::LineageTree {
                state.chrome.active_focus = TuiFocus::ChatPrompt;
            }
            return Vec::new();
        }
        KeyCode::F(1) => {
            state.chrome.active_modal = TuiModal::Help;
            return Vec::new();
        }
        KeyCode::Char('h') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            state.chrome.active_modal = TuiModal::Help;
            return Vec::new();
        }
        KeyCode::F(2) => {
            state.chrome.active_modal = TuiModal::SessionSwitcher;
            return vec![TuiUiAction::LoadSessions];
        }
        KeyCode::Char('s') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            state.chrome.active_modal = TuiModal::SessionSwitcher;
            return vec![TuiUiAction::LoadSessions];
        }
        KeyCode::F(3) => {
            state.chrome.details_open = !state.chrome.details_open;
            if state.chrome.details_open {
                state.chrome.active_focus = TuiFocus::Details;
            } else if state.chrome.active_focus == TuiFocus::Details {
                state.chrome.active_focus = TuiFocus::ChatPrompt;
            }
            return Vec::new();
        }
        KeyCode::Char('o') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            state.chrome.details_open = !state.chrome.details_open;
            if state.chrome.details_open {
                state.chrome.active_focus = TuiFocus::Details;
            } else if state.chrome.active_focus == TuiFocus::Details {
                state.chrome.active_focus = TuiFocus::ChatPrompt;
            }
            return Vec::new();
        }
        KeyCode::F(4) => {
            state.chrome.diagnostics_open = !state.chrome.diagnostics_open;
            if state.chrome.diagnostics_open {
                state.chrome.active_focus = TuiFocus::Diagnostics;
            } else if state.chrome.active_focus == TuiFocus::Diagnostics {
                state.chrome.active_focus = TuiFocus::ChatPrompt;
            }
            return Vec::new();
        }
        KeyCode::Char('l') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            state.chrome.diagnostics_open = !state.chrome.diagnostics_open;
            if state.chrome.diagnostics_open {
                state.chrome.active_focus = TuiFocus::Diagnostics;
            } else if state.chrome.active_focus == TuiFocus::Diagnostics {
                state.chrome.active_focus = TuiFocus::ChatPrompt;
            }
            return Vec::new();
        }
        KeyCode::Esc => {
            if state.is_running {
                return vec![TuiUiAction::InterruptRun];
            }
            state.chrome.active_focus = TuiFocus::ChatPrompt;
            return Vec::new();
        }
        _ => {}
    }

    // 3. Handle Running States
    if state.is_running {
        if key_event.code == KeyCode::Esc
            || (key_event.code == KeyCode::Char('c')
                && key_event.modifiers.contains(KeyModifiers::CONTROL))
        {
            return vec![TuiUiAction::InterruptRun];
        }
        // During execution, allow scrolling the drawers if they are open
        match state.chrome.active_focus {
            TuiFocus::Details => {
                handle_scroll_keys(key_event.code, &mut state.details.scroll_offset);
            }
            TuiFocus::Diagnostics => {
                handle_scroll_keys(key_event.code, &mut state.diagnostics.scroll_offset);
            }
            TuiFocus::LineageTree => {
                if let Some(ref model) = state.lineage.model {
                    handle_selection_keys(
                        key_event.code,
                        &mut state.lineage.selected_index,
                        model.nodes.len(),
                    );
                }
            }
            TuiFocus::ChatPrompt => {
                handle_chat_scroll_keys(
                    key_event.code,
                    &mut state.chat.scroll_offset,
                    state.chat.events.len(),
                );
            }
        }
        return Vec::new();
    }

    // 4. Focus Specific Handlers
    match state.chrome.active_focus {
        TuiFocus::ChatPrompt => {
            match key_event.code {
                KeyCode::Backspace => {
                    state.chat.input_buffer.pop();
                    state.chat.autocomplete_index = 0;
                }
                KeyCode::Enter => {
                    let trimmed = state.chat.input_buffer.trim().to_string();
                    if !trimmed.is_empty() {
                        state.chat.input_buffer.clear();
                        state.chat.autocomplete_index = 0;

                        // Check if it is a slash command
                        if trimmed.starts_with('/') {
                            let parts: Vec<&str> = trimmed.split_whitespace().collect();
                            let cmd = parts[0];
                            match cmd {
                                "/quit" | "/exit" => {
                                    return vec![TuiUiAction::Quit];
                                }
                                "/new" => {
                                    return vec![TuiUiAction::StartNewSession];
                                }
                                "/mode" => {
                                    if parts.len() < 2 {
                                        push_event(
                                            &mut state.chat.events,
                                            AgentEvent::Error {
                                                message: "Usage: /mode <mode>".to_string(),
                                                recoverable: true,
                                            },
                                        );
                                        return Vec::new();
                                    }
                                    let new_mode = parts[1].to_lowercase();
                                    match new_mode.as_str() {
                                        "confirm" | "yolo" | "human" | "dry-run" | "replay" => {
                                            return vec![TuiUiAction::ChangeMode(new_mode)];
                                        }
                                        _ => {
                                            push_event(&mut state.chat.events, AgentEvent::Error {
                                                message: "Invalid mode. Supported modes: confirm, yolo, human, dry-run, replay".to_string(),
                                                recoverable: true,
                                            });
                                            return Vec::new();
                                        }
                                    }
                                }
                                "/cost" => {
                                    return vec![TuiUiAction::CalculateCost];
                                }
                                "/help" => {
                                    state.chrome.active_modal = TuiModal::Help;
                                    return Vec::new();
                                }
                                "/runs" => {
                                    if state.chrome.terminal_width < 80 {
                                        push_event(&mut state.chat.events, AgentEvent::Error {
                                            message: "Lineage tree sidebar is unavailable in narrow views (< 80 columns).".to_string(),
                                            recoverable: true,
                                        });
                                        return Vec::new();
                                    }
                                    state.chrome.sidebar_open = true;
                                    state.chrome.active_focus = TuiFocus::LineageTree;
                                    return vec![TuiUiAction::LoadLineage];
                                }
                                "/config" => {
                                    state.chrome.details_open = !state.chrome.details_open;
                                    if state.chrome.details_open {
                                        state.chrome.active_focus = TuiFocus::Details;
                                    } else if state.chrome.active_focus == TuiFocus::Details {
                                        state.chrome.active_focus = TuiFocus::ChatPrompt;
                                    }
                                    return Vec::new();
                                }
                                "/context" => {
                                    if let Some(ref parent) = state.parent_run_id {
                                        return vec![TuiUiAction::ExplainContext(parent.clone())];
                                    } else {
                                        push_event(&mut state.chat.events, AgentEvent::Error {
                                            message: "No runs have been executed in this session yet.".to_string(),
                                            recoverable: true,
                                        });
                                        return Vec::new();
                                    }
                                }
                                "/branch" => {
                                    if parts.len() < 2 {
                                        push_event(
                                            &mut state.chat.events,
                                            AgentEvent::Error {
                                                message: "Usage: /branch <prompt>".to_string(),
                                                recoverable: true,
                                            },
                                        );
                                        return Vec::new();
                                    }
                                    let prompt_text = parts[1..].join(" ");
                                    let target_run_id = if state.chrome.sidebar_open {
                                        if let Some(ref model) = state.lineage.model {
                                            if state.lineage.selected_index < model.nodes.len() {
                                                Some(
                                                    model.nodes[state.lineage.selected_index]
                                                        .run_id
                                                        .clone(),
                                                )
                                            } else {
                                                state.parent_run_id.clone()
                                            }
                                        } else {
                                            state.parent_run_id.clone()
                                        }
                                    } else {
                                        state.parent_run_id.clone()
                                    };

                                    match target_run_id {
                                        Some(run_id) => {
                                            state.is_running = true;
                                            state.status = "Running".to_string();
                                            return vec![TuiUiAction::BranchSession {
                                                parent_run_id: run_id,
                                                prompt: prompt_text,
                                            }];
                                        }
                                        None => {
                                            push_event(
                                                &mut state.chat.events,
                                                AgentEvent::Error {
                                                    message: "No run to branch from.".to_string(),
                                                    recoverable: true,
                                                },
                                            );
                                            return Vec::new();
                                        }
                                    }
                                }
                                "/export" => {
                                    if parts.len() < 2 {
                                        push_event(
                                            &mut state.chat.events,
                                            AgentEvent::Error {
                                                message: "Usage: /export <markdown|jsonl|sharegpt>"
                                                    .to_string(),
                                                recoverable: true,
                                            },
                                        );
                                        return Vec::new();
                                    }
                                    let format = parts[1].to_lowercase();
                                    match format.as_str() {
                                        "markdown" | "jsonl" | "sharegpt" => {
                                            if let Some(ref parent) = state.parent_run_id {
                                                return vec![TuiUiAction::ExportRun {
                                                    parent_run_id: parent.clone(),
                                                    format,
                                                }];
                                            } else {
                                                push_event(&mut state.chat.events, AgentEvent::Error {
                                                    message: "No runs have been executed in this session yet.".to_string(),
                                                    recoverable: true,
                                                });
                                                return Vec::new();
                                            }
                                        }
                                        _ => {
                                            push_event(&mut state.chat.events, AgentEvent::Error {
                                                message: "Invalid export format. Supported: markdown, jsonl, sharegpt".to_string(),
                                                recoverable: true,
                                            });
                                            return Vec::new();
                                        }
                                    }
                                }
                                "/verify" => {
                                    if let Some(ref parent) = state.parent_run_id {
                                        return vec![TuiUiAction::VerifyRun(parent.clone())];
                                    } else {
                                        push_event(&mut state.chat.events, AgentEvent::Error {
                                            message: "No runs have been executed in this session yet.".to_string(),
                                            recoverable: true,
                                        });
                                        return Vec::new();
                                    }
                                }
                                _ => {
                                    push_event(&mut state.chat.events, AgentEvent::Error {
                                        message: format!("Unknown slash command: '{}'. Type /help for a list of commands.", cmd),
                                        recoverable: true,
                                    });
                                    return Vec::new();
                                }
                            }
                        }

                        state.run_error = None;
                        state.is_running = true;
                        state.status = "Running".to_string();
                        return vec![TuiUiAction::SubmitPrompt(trimmed)];
                    }
                }
                KeyCode::Char(c) => {
                    state.chat.input_buffer.push(c);
                    state.chat.autocomplete_index = 0;
                }
                KeyCode::Up => {
                    if state.chat.input_buffer.starts_with('/') {
                        let commands = vec![
                            "/help", "/new", "/quit", "/exit", "/mode", "/cost", "/context",
                            "/runs", "/branch", "/config", "/export", "/verify",
                        ];
                        let typed = state.chat.input_buffer.to_lowercase();
                        let typed_cmd = typed.split_whitespace().next().unwrap_or("");
                        let filtered_count = commands
                            .iter()
                            .filter(|cmd| cmd.starts_with(typed_cmd))
                            .count();
                        if filtered_count > 0 {
                            if state.chat.autocomplete_index > 0 {
                                state.chat.autocomplete_index -= 1;
                            } else {
                                state.chat.autocomplete_index = filtered_count - 1;
                            }
                            return Vec::new();
                        }
                    }
                    if state.chat.scroll_offset < state.chat.events.len() {
                        state.chat.scroll_offset += 1;
                    }
                }
                KeyCode::Down => {
                    if state.chat.input_buffer.starts_with('/') {
                        let commands = vec![
                            "/help", "/new", "/quit", "/exit", "/mode", "/cost", "/context",
                            "/runs", "/branch", "/config", "/export", "/verify",
                        ];
                        let typed = state.chat.input_buffer.to_lowercase();
                        let typed_cmd = typed.split_whitespace().next().unwrap_or("");
                        let filtered_count = commands
                            .iter()
                            .filter(|cmd| cmd.starts_with(typed_cmd))
                            .count();
                        if filtered_count > 0 {
                            if state.chat.autocomplete_index + 1 < filtered_count {
                                state.chat.autocomplete_index += 1;
                            } else {
                                state.chat.autocomplete_index = 0;
                            }
                            return Vec::new();
                        }
                    }
                    if state.chat.scroll_offset > 0 {
                        state.chat.scroll_offset -= 1;
                    }
                }
                _ => {}
            }
        }
        TuiFocus::LineageTree => {
            if let Some(ref model) = state.lineage.model {
                handle_selection_keys(
                    key_event.code,
                    &mut state.lineage.selected_index,
                    model.nodes.len(),
                );
            }
            if key_event.code == KeyCode::Char('b') {
                state.chat.input_buffer = "/branch ".to_string();
                state.chrome.active_focus = TuiFocus::ChatPrompt;
                return Vec::new();
            }
            if key_event.code == KeyCode::Left {
                state.chrome.active_focus = TuiFocus::ChatPrompt;
            }
        }
        TuiFocus::Details => {
            handle_scroll_keys(key_event.code, &mut state.details.scroll_offset);
            if key_event.code == KeyCode::Left {
                state.chrome.active_focus = TuiFocus::ChatPrompt;
            }
        }
        TuiFocus::Diagnostics => {
            handle_scroll_keys(key_event.code, &mut state.diagnostics.scroll_offset);
            if key_event.code == KeyCode::Left {
                state.chrome.active_focus = TuiFocus::ChatPrompt;
            }
        }
    }

    Vec::new()
}

fn handle_scroll_keys(code: KeyCode, offset: &mut usize) {
    match code {
        KeyCode::Up => {
            *offset = offset.saturating_sub(1);
        }
        KeyCode::Down => {
            *offset = offset.saturating_add(1);
        }
        _ => {}
    }
}

fn handle_chat_scroll_keys(code: KeyCode, offset: &mut usize, max_len: usize) {
    match code {
        KeyCode::Up => {
            if *offset < max_len {
                *offset = offset.saturating_add(1);
            }
        }
        KeyCode::Down => {
            *offset = offset.saturating_sub(1);
        }
        _ => {}
    }
}

fn handle_selection_keys(code: KeyCode, index: &mut usize, len: usize) {
    if len == 0 {
        return;
    }
    match code {
        KeyCode::Up => {
            if *index > 0 {
                *index -= 1;
            } else {
                *index = len - 1;
            }
        }
        KeyCode::Down => {
            if *index + 1 < len {
                *index += 1;
            } else {
                *index = 0;
            }
        }
        _ => {}
    }
}
