use std::sync::Arc;
use std::time::Duration;

use gestalt_core::{
    approval::{ApprovalDecision, ApprovalRequest},
    cancel::CancelToken,
    error::HarnessError,
    event::AgentEvent,
};
use gestalt_trace::run_manifest::RunManifest;

use crate::config::EffectiveConfig;
use crate::run::run_prompt;
use crate::sessions;
use crate::tui::approval::TuiApprovalProvider;
use crate::tui::bridge::{
    get_diagnostics_logs, init_diagnostics_buffer, TuiBridgeMessage, TuiLogLayer,
};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Terminal,
};
use tokio::sync::{mpsc, oneshot};

/// RAII Guard to ensure the terminal is restored to its original state
/// on any exit path, including errors, early returns, cancellations, or panics.
struct TerminalGuard;

impl TerminalGuard {
    fn create() -> Result<Self, HarnessError> {
        crossterm::terminal::enable_raw_mode()
            .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
        crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::EnterAlternateScreen,
            crossterm::event::EnableMouseCapture
        )
        .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show
        );
    }
}

/// Helper to scan for the latest run manifest in a session
fn find_latest_run_id(config: &EffectiveConfig, session_id: &str) -> Option<String> {
    let run_log_dir = config.run_log_dir();
    let mut latest_run: Option<(std::time::SystemTime, String)> = None;
    if run_log_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(run_log_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let manifest_path = entry.path().join("run.json");
                    if manifest_path.exists() {
                        if let Ok(manifest) = RunManifest::load_from(&manifest_path) {
                            if manifest.session_id == session_id {
                                if let Ok(metadata) = entry.metadata() {
                                    if let Ok(modified) = metadata.modified() {
                                        if latest_run.is_none() || modified > latest_run.as_ref().unwrap().0 {
                                            latest_run = Some((modified, manifest.run_id));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    latest_run.map(|(_, id)| id)
}

/// Spawns the background run task and executes the main TUI event loop.
pub async fn run_tui(
    config: &EffectiveConfig,
    resume: Option<String>,
    prompt: Option<String>,
    api_key: Option<String>,
    cancel_token: CancelToken,
) -> Result<(), HarnessError> {
    // 1. Initialize Diagnostics Buffer and Tracing Subscriber
    let max_lines = config
        .tui
        .diagnostics
        .as_ref()
        .and_then(|d| d.max_log_lines)
        .unwrap_or(1000);
    init_diagnostics_buffer(max_lines);

    use tracing_subscriber::prelude::*;
    let subscriber = tracing_subscriber::registry().with(TuiLogLayer);
    let _ = tracing::subscriber::set_global_default(subscriber);

    // 2. Initialize Crossterm and Ratatui using TerminalGuard for guaranteed cleanup
    let _guard = TerminalGuard::create()?;
    let stdout = std::io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)
        .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;

    // Set up panic hook to automatically restore raw mode
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let mut stdout = std::io::stdout();
        let _ = crossterm::execute!(
            stdout,
            crossterm::terminal::LeaveAlternateScreen,
            crossterm::event::DisableMouseCapture,
            crossterm::cursor::Show
        );
        original_hook(panic_info);
    }));

    // 3. Setup Session Lineage Tracking (Finding 1 recovery and attach)
    let mut session_id = format!("session-{}", uuid::Uuid::new_v4());
    let mut parent_run_id: Option<String> = None;

    if let Some(ref target) = resume {
        let parent_run_path = crate::runs::resolve_run_path(config, target)?;
        let manifest_path = parent_run_path.join("run.json");
        if !manifest_path.exists() {
            return Err(HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
                field: "resume".to_string(),
                reason: format!("run.json missing from {}", parent_run_path.display()),
            }));
        }

        let manifest = RunManifest::load_from(&manifest_path)
            .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::ReadFailed { reason: e.to_string() }))?;

        session_id = manifest.session_id;
        parent_run_id = Some(manifest.run_id);
    }

    // 4. Create Bridge & Event Channels
    let (bridge_tx, mut bridge_rx) = mpsc::unbounded_channel();
    let approval_provider = Arc::new(TuiApprovalProvider::new(bridge_tx.clone()));

    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let bridge_tx_event = bridge_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let _ = bridge_tx_event.send(TuiBridgeMessage::AgentEvent(event));
        }
    });

    // Spawn Run Helper
    let spawn_run = {
        let config = config.clone();
        let api_key = api_key.clone();
        let approval_provider = approval_provider.clone();
        let event_tx = event_tx.clone();
        let bridge_tx = bridge_tx.clone();
        move |run_prompt_val: Option<String>, run_resume_val: Option<String>, current_session: String, current_parent: Option<String>, cancel_token: CancelToken| {
            let config = config.clone();
            let api_key = api_key.clone();
            let approval_provider = approval_provider.clone();
            let event_tx = event_tx.clone();
            let bridge_tx = bridge_tx.clone();
            tokio::spawn(async move {
                let res = async {
                    if let Some(p) = run_prompt_val {
                        if let Some(ref target) = current_parent {
                            sessions::run_session_action(
                                &config,
                                "branch",
                                target,
                                Some(p),
                                None,
                                api_key,
                                cancel_token,
                                Some(approval_provider),
                                Some(event_tx),
                            )
                            .await
                        } else {
                            run_prompt(
                                &config,
                                &p,
                                api_key,
                                cancel_token,
                                Some(approval_provider),
                                Some(event_tx),
                                Some(current_session),
                            )
                            .await
                        }
                    } else if let Some(target) = run_resume_val {
                        // Finding 3: Wire to "resume", not "continue"
                        sessions::run_session_action(
                            &config,
                            "resume",
                            &target,
                            None,
                            None,
                            api_key,
                            cancel_token,
                            Some(approval_provider),
                            Some(event_tx),
                        )
                        .await
                    } else {
                        Err(HarnessError::Config(gestalt_core::ConfigError::MissingField(
                            "prompt or resume target".to_string(),
                        )))
                    }
                }
                .await;

                let _ = bridge_tx.send(TuiBridgeMessage::RunCompleted(res));
            });
        }
    };

    // 5. Poll Crossterm Keyboard Events Asynchronously
    let (input_tx, mut input_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(ev) = event::read() {
                    if input_tx.send(ev).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // 6. Main TUI Event/Render Loop State
    let mut agent_events = Vec::new();
    let mut status = "Idle".to_string();
    let mut active_approval: Option<(ApprovalRequest, oneshot::Sender<ApprovalDecision>)> = None;
    let mut is_running = false;
    let mut run_error: Option<String> = None;
    let mut input_buffer = String::new();
    let mut active_cancel_token = cancel_token.clone();

    // Trigger initial run if requested
    if prompt.is_some() || resume.is_some() {
        is_running = true;
        status = "Running".to_string();
        spawn_run(prompt.clone(), resume.clone(), session_id.clone(), parent_run_id.clone(), active_cancel_token.clone());
    }

    loop {
        // Draw the terminal
        let current_status = status.clone();
        let events_ref = &agent_events;
        let approval_ref = &active_approval;
        let _error_ref = &run_error;
        let input_ref = &input_buffer;

        terminal
            .draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(3),      // Main panels
                        Constraint::Length(3),   // Status Bar
                        Constraint::Length(3),   // Prompt Input
                    ])
                    .split(f.area());

                let main_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(chunks[0]);

                // Left Panel: Agent Events
                let event_items: Vec<ListItem> = events_ref
                    .iter()
                    .map(|ev| {
                        let line = format_agent_event(ev);
                        ListItem::new(line)
                    })
                    .collect();
                let events_list = List::new(event_items)
                    .block(Block::default().borders(Borders::ALL).title("Agent Events"));
                f.render_widget(events_list, main_chunks[0]);

                // Right Panel: Diagnostics Logs
                let logs = get_diagnostics_logs();
                let log_items: Vec<ListItem> = logs
                    .iter()
                    .map(|line| ListItem::new(line.clone()))
                    .collect();
                let logs_list = List::new(log_items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Diagnostics Logs"),
                );
                f.render_widget(logs_list, main_chunks[1]);

                // Bottom Status Bar
                let status_style = if current_status.contains("Awaiting") {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else if current_status.contains("Completed") {
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                } else if current_status.contains("Failed") {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                };

                let help_text = if approval_ref.is_some() {
                    "Press 'a' to Approve, 'd' to Deny, 'c' to Cancel"
                } else if is_running {
                    "Press Esc or Ctrl+C to Cancel/Interrupt"
                } else {
                    "Type prompt and press Enter to run | 'q' or Ctrl+C to Quit"
                };

                let status_line = Line::from(vec![
                    Span::raw("Status: "),
                    Span::styled(&current_status, status_style),
                    Span::raw("  |  "),
                    Span::raw(help_text),
                ]);
                let status_para = Paragraph::new(status_line)
                    .block(Block::default().borders(Borders::ALL).title("Status Bar"));
                f.render_widget(status_para, chunks[1]);

                // Input block
                let input_style = if is_running {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::LightCyan)
                };
                let input_title = if is_running {
                    "Prompt Input (Locked during run)"
                } else {
                    "Prompt Input (Press Enter to submit)"
                };
                let input_para = Paragraph::new(input_ref.as_str())
                    .block(Block::default().borders(Borders::ALL).border_style(input_style).title(input_title));
                f.render_widget(input_para, chunks[2]);

                if !is_running {
                    let cursor_x = (chunks[2].x + 1 + input_ref.len() as u16)
                        .min(chunks[2].x + chunks[2].width - 2);
                    f.set_cursor_position((cursor_x, chunks[2].y + 1));
                }

                // Overlay Approval Popup if active
                if let Some((req, _)) = approval_ref {
                    let popup_area = centered_rect(60, 40, f.area());
                    f.render_widget(Clear, popup_area);

                    let popup_text = vec![
                        Line::from(vec![
                            Span::styled("Tool Name: ", Style::default().add_modifier(Modifier::BOLD)),
                            Span::styled(&req.tool_name, Style::default().fg(Color::Magenta)),
                        ]),
                        Line::from(""),
                        Line::from(vec![
                            Span::styled("Description: ", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(&req.description),
                        ]),
                        Line::from(""),
                        Line::from(vec![
                            Span::styled("Input Parameters: ", Style::default().add_modifier(Modifier::BOLD)),
                        ]),
                        Line::from(req.input.to_string()),
                        Line::from(""),
                        Line::from(vec![
                            Span::styled("Action Required: ", Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw("Approve [a], Deny [d], Cancel [c]"),
                        ]),
                    ];

                    let popup_para = Paragraph::new(popup_text)
                        .block(Block::default().borders(Borders::ALL).title("Approval Request"))
                        .wrap(Wrap { trim: true });

                    f.render_widget(popup_para, popup_area);
                }
            })
            .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;

        // 7. Handle Incoming Events
        tokio::select! {
            // Messages from the background agent task
            Some(msg) = bridge_rx.recv() => {
                match msg {
                    TuiBridgeMessage::AgentEvent(ev) => {
                        agent_events.push(ev);
                    }
                    TuiBridgeMessage::ApprovalRequest { request, response_tx } => {
                        active_approval = Some((request, response_tx));
                        status = "Awaiting Approval".to_string();
                    }
                    TuiBridgeMessage::RunCompleted(res) => {
                        is_running = false;
                        active_approval = None; // Dismiss any active approvals
                        match res {
                            Ok(run_dir) => {
                                status = "Completed. Ready for prompt.".to_string();
                                let manifest_path = run_dir.join("run.json");
                                if let Ok(manifest) = RunManifest::load_from(&manifest_path) {
                                    parent_run_id = Some(manifest.run_id);
                                    session_id = manifest.session_id;
                                }
                            }
                            Err(HarnessError::Cancelled) => {
                                status = "Cancelled. Ready for prompt.".to_string();
                                run_error = Some("Cancelled by user".to_string());
                                if let Some(latest) = find_latest_run_id(config, &session_id) {
                                    parent_run_id = Some(latest);
                                }
                            }
                            Err(e) => {
                                status = format!("Failed: {}. Ready.", e);
                                run_error = Some(format!("{}", e));
                                if let Some(latest) = find_latest_run_id(config, &session_id) {
                                    parent_run_id = Some(latest);
                                }
                            }
                        }
                    }
                }
            }
            // Crossterm Keyboard inputs
            Some(ev) = input_rx.recv() => {
                if let Event::Key(key) = ev {
                    if key.kind == event::KeyEventKind::Press {
                        if is_running {
                            if let Some((req, response_tx)) = active_approval.take() {
                                match key.code {
                                    KeyCode::Char('a') => {
                                        let _ = response_tx.send(ApprovalDecision::Approve);
                                        status = "Running".to_string();
                                    }
                                    KeyCode::Char('d') => {
                                        let _ = response_tx.send(ApprovalDecision::Deny);
                                        status = "Running".to_string();
                                    }
                                    KeyCode::Char('c') => {
                                        drop(response_tx);
                                        status = "Running".to_string();
                                    }
                                    _ => {
                                        // Finding 2: Re-wrap active approval on non-approval key
                                        active_approval = Some((req, response_tx));
                                    }
                                }
                            } else {
                                // Escape / Ctrl+C triggers cancellation of active run
                                if key.code == KeyCode::Esc || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)) {
                                    active_cancel_token.cancel();
                                    status = "Cancelling...".to_string();
                                }
                            }
                        } else {
                            // If NOT running, handle text input and general navigation
                            match key.code {
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    break;
                                }
                                KeyCode::Char('q') => {
                                    break;
                                }
                                KeyCode::Enter => {
                                    let trimmed = input_buffer.trim();
                                    if !trimmed.is_empty() {
                                        agent_events.clear();
                                        run_error = None;
                                        is_running = true;
                                        status = "Running".to_string();
                                        active_cancel_token = CancelToken::new();
                                        spawn_run(
                                            Some(trimmed.to_string()),
                                            None,
                                            session_id.clone(),
                                            parent_run_id.clone(),
                                            active_cancel_token.clone(),
                                        );
                                        input_buffer.clear();
                                    }
                                }
                                KeyCode::Backspace => {
                                    input_buffer.pop();
                                }
                                KeyCode::Char(c) => {
                                    input_buffer.push(c);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Helper function to format an `AgentEvent` variant into a summary line.
fn format_agent_event(event: &AgentEvent) -> String {
    match event {
        AgentEvent::UserMessage { content } => format!("User> {}", content),
        AgentEvent::ContextBuilt { token_estimate, .. } => format!("Context built (~{} tokens)", token_estimate),
        AgentEvent::ModelRequest { model, .. } => format!("Model request ({})", model),
        AgentEvent::Text { delta } => format!("Text: {}", delta),
        AgentEvent::Thinking { delta } => format!("Thinking: {}", delta),
        AgentEvent::ToolCallStreamed { name, .. } => format!("Tool call streaming: {}", name),
        AgentEvent::ToolCallProposed { name, .. } => format!("Tool call proposed: {}", name),
        AgentEvent::PolicyDecision { tool_name, risk, .. } => format!("Policy decision for {:?} (Risk: {:?})", tool_name, risk),
        AgentEvent::ApprovalDecision { decision, .. } => format!("Approval decision: {:?}", decision),
        AgentEvent::ToolResult { tool_name, is_error, .. } => format!(
            "Tool {} result (Error: {})",
            tool_name.as_deref().unwrap_or("unknown"),
            is_error
        ),
        AgentEvent::Checkpoint { .. } => "Checkpoint saved".to_string(),
        AgentEvent::Interrupted { reason } => format!("Interrupted: {}", reason),
        AgentEvent::Stop { reason } => format!("Stop: {:?}", reason),
        AgentEvent::Error { message, .. } => format!("Error: {}", message),
        _ => format!("{:?}", event),
    }
}

/// Helper function to create a centered rectangle for popup overlays.
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
