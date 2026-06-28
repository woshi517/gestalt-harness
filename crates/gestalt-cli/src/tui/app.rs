#![allow(clippy::large_futures)]

use std::sync::Arc;
use std::time::Duration;

use gestalt_core::{
    approval::ApprovalDecision, cancel::CancelToken, error::HarnessError, event::AgentEvent,
};
use gestalt_trace::run_manifest::RunManifest;

use crate::config::EffectiveConfig;
use crate::output::CliReport;
use crate::run::run_prompt;
use crate::sessions;
use crate::tui::approval::TuiApprovalProvider;
use crate::tui::bridge::{init_diagnostics_buffer, TuiBridgeMessage, TuiLogLayer};
use crate::tui::screens::chat::draw_chat_screen;
use crate::tui::services;
use crate::tui::state::{push_event, TuiAppState, TuiModal};
use crate::tui::update::{self, TuiUiAction};

use crossterm::event::{self, Event};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::{mpsc, oneshot};

/// RAII Guard to ensure the terminal is restored to its original state
/// on any exit path, including errors, early returns, cancellations, or panics.
struct TerminalGuard;

impl TerminalGuard {
    fn create() -> Result<Self, HarnessError> {
        crossterm::terminal::enable_raw_mode()
            .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)
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
                                        if latest_run.is_none()
                                            || modified > latest_run.as_ref().unwrap().0
                                        {
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

fn check_onboarding_needed(config: &EffectiveConfig) -> bool {
    if let Ok(resolved) = config.resolve_provider() {
        let provider_name = resolved.provider_name().to_string();
        if provider_name == "ollama" {
            return false;
        }
        if let Ok(report) = crate::auth::resolve_auth(config, &provider_name) {
            return report.status == "missing";
        }
    }
    true
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
            crossterm::cursor::Show
        );
        original_hook(panic_info);
    }));

    // 3. Setup Session Lineage Tracking
    let mut session_id = format!("session-{}", uuid::Uuid::new_v4());
    let mut parent_run_id: Option<String> = None;

    if let Some(ref target) = resume {
        let parent_run_path = crate::runs::resolve_run_path(config, target)?;
        let manifest_path = parent_run_path.join("run.json");
        if !manifest_path.exists() {
            return Err(HarnessError::Config(
                gestalt_core::ConfigError::InvalidValue {
                    field: "resume".to_string(),
                    reason: format!("run.json missing from {}", parent_run_path.display()),
                },
            ));
        }

        let manifest = RunManifest::load_from(&manifest_path).map_err(|e| {
            HarnessError::Trace(gestalt_core::TraceError::ReadFailed {
                reason: e.to_string(),
            })
        })?;

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
        let api_key = api_key.clone();
        let approval_provider = approval_provider.clone();
        let event_tx = event_tx.clone();
        let bridge_tx = bridge_tx.clone();
        move |run_config: EffectiveConfig,
              run_prompt_val: Option<String>,
              run_resume_val: Option<String>,
              current_session: String,
              current_parent: Option<String>,
              action_type: &'static str,
              cancel_token: CancelToken| {
            let api_key = api_key.clone();
            let approval_provider = approval_provider.clone();
            let event_tx = event_tx.clone();
            let bridge_tx = bridge_tx.clone();
            tokio::spawn(async move {
                let res = async {
                    if action_type == "branch" {
                        if let (Some(p), Some(ref target)) = (run_prompt_val, current_parent) {
                            sessions::run_session_action(
                                &run_config,
                                "branch",
                                target,
                                Some(p),
                                None,
                                api_key,
                                cancel_token,
                                Some(approval_provider),
                                Some(event_tx),
                                None,
                            )
                            .await
                        } else {
                            Err(HarnessError::Config(
                                gestalt_core::ConfigError::MissingField(
                                    "prompt or branch target".to_string(),
                                ),
                            ))
                        }
                    } else if action_type == "continue" {
                        if let Some(p) = run_prompt_val {
                            if current_parent.is_some() {
                                sessions::run_session_action(
                                    &run_config,
                                    "continue",
                                    &current_session,
                                    Some(p),
                                    None,
                                    api_key,
                                    cancel_token,
                                    Some(approval_provider),
                                    Some(event_tx),
                                    None,
                                )
                                .await
                            } else {
                                run_prompt(
                                    &run_config,
                                    &p,
                                    api_key,
                                    cancel_token,
                                    Some(approval_provider),
                                    Some(event_tx),
                                    Some(current_session),
                                    None,
                                )
                                .await
                            }
                        } else {
                            Err(HarnessError::Config(
                                gestalt_core::ConfigError::MissingField("prompt".to_string()),
                            ))
                        }
                    } else if action_type == "resume" {
                        if let Some(target) = run_resume_val {
                            sessions::run_session_action(
                                &run_config,
                                "resume",
                                &target,
                                None,
                                None,
                                api_key,
                                cancel_token,
                                Some(approval_provider),
                                Some(event_tx),
                                None,
                            )
                            .await
                        } else {
                            Err(HarnessError::Config(
                                gestalt_core::ConfigError::MissingField(
                                    "resume target".to_string(),
                                ),
                            ))
                        }
                    } else {
                        Err(HarnessError::Config(
                            gestalt_core::ConfigError::InvalidValue {
                                field: "action_type".to_string(),
                                reason: format!("Unknown action type: {}", action_type),
                            },
                        ))
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

    // 6. Initialize State
    let mut state = TuiAppState::new(config.clone(), session_id, parent_run_id);
    let mut active_cancel_token = cancel_token.clone();
    let mut active_approval_tx: Option<oneshot::Sender<ApprovalDecision>> = None;
    let (conn_tx, mut conn_rx) = mpsc::unbounded_channel();

    // Check if onboarding connection wizard is needed
    if check_onboarding_needed(&state.config) {
        state.chrome.active_modal = TuiModal::Onboarding;
    }

    // Load initial lineage tree
    if let Ok(tree) = services::load_lineage_tree(&state.config, &state.session_id) {
        state.lineage.model = Some(tree);
    }

    // Load initial transcript (for resumed sessions)
    if let Ok(transcript) = services::load_session_transcript(
        &state.config,
        &state.session_id,
        state.parent_run_id.as_deref(),
    ) {
        state.chat.events = transcript;
    }

    // Trigger initial run if requested (only if onboarding is not active)
    if (prompt.is_some() || resume.is_some()) && state.chrome.active_modal != TuiModal::Onboarding {
        state.is_running = true;
        state.status = "Running".to_string();
        let init_action = if resume.is_some() && prompt.is_some() {
            "branch"
        } else if resume.is_some() {
            "resume"
        } else {
            "continue"
        };
        spawn_run(
            state.config.clone(),
            prompt.clone(),
            resume.clone(),
            state.session_id.clone(),
            state.parent_run_id.clone(),
            init_action,
            active_cancel_token.clone(),
        );
    }

    loop {
        // Draw the terminal, updating size state automatically
        terminal
            .draw(|f| {
                state.chrome.terminal_width = f.area().width;
                state.chrome.terminal_height = f.area().height;
                draw_chat_screen(f, &state);
            })
            .map_err(|e| HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;

        // 7. Handle Incoming Events
        tokio::select! {
            // Messages from the background agent task
            Some(msg) = bridge_rx.recv() => {
                match msg {
                    TuiBridgeMessage::AgentEvent(ev) => {
                        let _ = update::apply_agent_event(&mut state, ev);
                    }
                    TuiBridgeMessage::ApprovalRequest { request, response_tx } => {
                        active_approval_tx = Some(response_tx);
                        state.approval.active_request = Some(request);
                        state.chrome.active_modal = TuiModal::Approval;
                        state.status = "Awaiting Approval".to_string();
                    }
                    TuiBridgeMessage::RunCompleted(res) => {
                        state.is_running = false;
                        state.approval.active_request = None;
                        state.chrome.active_modal = TuiModal::None;
                        active_approval_tx = None;
                        match res {
                            Ok(run_dir) => {
                                state.status = "Completed. Ready for prompt.".to_string();
                                let manifest_path = run_dir.join("run.json");
                                if let Ok(manifest) = RunManifest::load_from(&manifest_path) {
                                    state.parent_run_id = Some(manifest.run_id);
                                    state.session_id = manifest.session_id;
                                }
                            }
                            Err(HarnessError::Cancelled) => {
                                state.status = "Cancelled. Ready for prompt.".to_string();
                                state.run_error = Some("Cancelled by user".to_string());
                                if let Some(latest) = find_latest_run_id(&state.config, &state.session_id) {
                                    state.parent_run_id = Some(latest);
                                }
                            }
                            Err(e) => {
                                let message = e.to_string();
                                state.status = "Failed. Ready for prompt.".to_string();
                                state.run_error = Some(message.clone());
                                state.show_notification("Run failed", message, true);
                                if let Some(latest) = find_latest_run_id(&state.config, &state.session_id) {
                                    state.parent_run_id = Some(latest);
                                }
                            }
                        }
                        // Refresh lineage tree
                        if let Ok(tree) = services::load_lineage_tree(&state.config, &state.session_id) {
                            state.lineage.model = Some(tree);
                        }
                    }
                }
            }
            // Onboarding connection results from background thread
            Some(conn_res) = conn_rx.recv() => {
                match conn_res {
                    Ok(_) => {
                        state.chrome.active_modal = TuiModal::None;
                        state.status = "Provider Connected. Ready.".to_string();
                        // Reload config
                        if let Ok(new_cfg) = crate::config::load_effective_config(&crate::config::CliOverrides::default()) {
                            state.config = new_cfg.clone();
                            state.details.config = Some(new_cfg);
                        }
                        // Refresh sessions list
                        if let Ok(list) = services::load_session_list(&state.config) {
                            state.switcher.model = Some(list);
                        }
                        // Refresh lineage tree
                        if let Ok(tree) = services::load_lineage_tree(&state.config, &state.session_id) {
                            state.lineage.model = Some(tree);
                        }
                    }
                    Err(e) => {
                        state.onboarding.error_message = Some(format!("{e}"));
                        state.status = "Connection Failed".to_string();
                    }
                }
            }
            // Crossterm events
            Some(ev) = input_rx.recv() => {
                match ev {
                    Event::Key(key) => {
                        let actions = update::handle_key_event(&mut state, key);
                        let mut should_quit = false;

                        for action in actions {
                            match action {
                                TuiUiAction::SubmitPrompt(p) => {
                                    state.is_running = true;
                                    state.status = "Running".to_string();
                                    active_cancel_token = CancelToken::new();
                                    spawn_run(
                                        state.config.clone(),
                                        Some(p),
                                        None,
                                        state.session_id.clone(),
                                        state.parent_run_id.clone(),
                                        "continue",
                                        active_cancel_token.clone(),
                                    );
                                }
                                TuiUiAction::BranchSession { parent_run_id, prompt } => {
                                    state.is_running = true;
                                    state.status = "Running".to_string();
                                    active_cancel_token = CancelToken::new();
                                    spawn_run(
                                        state.config.clone(),
                                        Some(prompt),
                                        None,
                                        state.session_id.clone(),
                                        Some(parent_run_id),
                                        "branch",
                                        active_cancel_token.clone(),
                                    );
                                }
                                TuiUiAction::InterruptRun => {
                                    active_cancel_token.cancel();
                                    state.status = "Cancelling...".to_string();
                                }
                                TuiUiAction::ApprovalDecision { decision } => {
                                    if let Some(tx) = active_approval_tx.take() {
                                        let _ = tx.send(decision);
                                    }
                                    state.status = "Running".to_string();
                                }
                                TuiUiAction::SelectSession(sid) => {
                                    state.session_id = sid.clone();
                                    state.parent_run_id = find_latest_run_id(&state.config, &sid);
                                    if let Ok(transcript) = services::load_session_transcript(&state.config, &state.session_id, state.parent_run_id.as_deref()) {
                                        state.chat.events = transcript;
                                    } else {
                                        state.chat.events.clear();
                                    }
                                    state.lineage.selected_index = 0;
                                    if let Ok(tree) = services::load_lineage_tree(&state.config, &state.session_id) {
                                        state.lineage.model = Some(tree);
                                    }
                                }
                                TuiUiAction::LoadSessions => {
                                    if let Ok(list) = services::load_session_list(&state.config) {
                                        state.switcher.model = Some(list);
                                    }
                                    state.switcher.selected_index = 0;
                                }
                                TuiUiAction::LoadLineage => {
                                    if let Ok(tree) = services::load_lineage_tree(&state.config, &state.session_id) {
                                        state.lineage.model = Some(tree);
                                    }
                                    state.lineage.selected_index = 0;
                                }
                                TuiUiAction::StartNewSession => {
                                    state.start_new_session();
                                }
                                TuiUiAction::SaveOnboarding { provider, api_key } => {
                                    state.onboarding.error_message = None;
                                    state.status = "Connecting LLM Provider...".to_string();
                                    let config = state.config.clone();
                                    let prov = provider.clone();
                                    let key_opt = if api_key.is_empty() { None } else { Some(api_key) };
                                    let tx = conn_tx.clone();
                                    tokio::task::spawn_blocking(move || {
                                        let res = crate::connect::connect_provider(
                                            &config,
                                            &prov,
                                            key_opt,
                                            false, // no_keychain
                                            true,  // set_default
                                            None, None, None, None,
                                            None,
                                        );
                                        let _ = tx.send(res);
                                    });
                                }
                                TuiUiAction::ChangeMode(mode) => {
                                    if let Ok(parsed_mode) = crate::config::mode_from_str(&mode) {
                                        state.config.defaults.mode = Some(parsed_mode);
                                        state.details.config = Some(state.config.clone());
                                        push_event(&mut state.chat.events, AgentEvent::ContextBuilt {
                                            packet_id: String::new(),
                                            token_estimate: 0,
                                            packet_hash: None,
                                            sources: None,
                                            omissions: None,
                                            prompt_source: Some(format!("Switched execution mode to '{mode}'")),
                                        });
                                    } else {
                                        push_event(&mut state.chat.events, AgentEvent::Error {
                                            message: format!("Invalid mode: '{mode}'. Supported: confirm, yolo, human, dry-run, replay"),
                                            recoverable: true,
                                        });
                                    }
                                }
                                TuiUiAction::CalculateCost => {
                                    let cost = crate::slash::calculate_session_cost(&state.config, &state.session_id);
                                    push_event(&mut state.chat.events, AgentEvent::ContextBuilt {
                                        packet_id: String::new(),
                                        token_estimate: 0,
                                        packet_hash: None,
                                        sources: None,
                                        omissions: None,
                                        prompt_source: Some(format!("Aggregated session cost: ${cost:.6}")),
                                    });
                                }
                                TuiUiAction::ExplainContext(parent) => {
                                    let overrides = crate::config::CliOverrides {
                                        workspace: Some(state.config.workspace_root.clone()),
                                        provider: state.config.provider_override.clone(),
                                        model: state.config.model_override.clone(),
                                        mode: state.config.defaults.mode.map(|mode| mode.to_string()),
                                        max_turns: state.config.defaults.max_turns,
                                        profile: state.config.defaults.profile.clone(),
                                        ..crate::config::CliOverrides::default()
                                    };
                                    let tx = bridge_tx.clone();
                                    tokio::spawn(async move {
                                        let res = crate::context::explain_context(&overrides, None, Some(&parent)).await;
                                        let msg = match res {
                                            Ok(report) => report.render_text(),
                                            Err(e) => format!("Error explaining context: {e}"),
                                        };
                                        let _ = tx.send(TuiBridgeMessage::AgentEvent(AgentEvent::ContextBuilt {
                                            packet_id: String::new(),
                                            token_estimate: 0,
                                            packet_hash: None,
                                            sources: None,
                                            omissions: None,
                                            prompt_source: Some(msg),
                                        }));
                                    });
                                }
                                TuiUiAction::ExportRun { parent_run_id, format } => {
                                    let config = state.config.clone();
                                    let tx = bridge_tx.clone();
                                    tokio::spawn(async move {
                                        let export_format = match format.as_str() {
                                            "jsonl" => crate::output::ExportFormat::Jsonl,
                                            "sharegpt" => crate::output::ExportFormat::Sharegpt,
                                            _ => crate::output::ExportFormat::Markdown,
                                        };
                                        let res = crate::export::export_run(&config, &parent_run_id, export_format);
                                        let msg = match res {
                                            Ok(report) => report.render_text(),
                                            Err(e) => format!("Error exporting run: {e}"),
                                        };
                                        let _ = tx.send(TuiBridgeMessage::AgentEvent(AgentEvent::ContextBuilt {
                                            packet_id: String::new(),
                                            token_estimate: 0,
                                            packet_hash: None,
                                            sources: None,
                                            omissions: None,
                                            prompt_source: Some(msg),
                                        }));
                                    });
                                }
                                TuiUiAction::VerifyRun(parent) => {
                                    let config = state.config.clone();
                                    let tx = bridge_tx.clone();
                                    tokio::spawn(async move {
                                        let res = crate::verify::verify_run(&config, &parent).await;
                                        let msg = match res {
                                            Ok(report) => report.render_text(),
                                            Err(e) => format!("Error verifying run: {e}"),
                                        };
                                        let _ = tx.send(TuiBridgeMessage::AgentEvent(AgentEvent::ContextBuilt {
                                            packet_id: String::new(),
                                            token_estimate: 0,
                                            packet_hash: None,
                                            sources: None,
                                            omissions: None,
                                            prompt_source: Some(msg),
                                        }));
                                    });
                                }
                                TuiUiAction::Quit => {
                                    should_quit = true;
                                }
                            }
                        }

                        if should_quit {
                            break;
                        }
                    }
                    Event::Resize(w, h) => {
                        state.chrome.terminal_width = w;
                        state.chrome.terminal_height = h;
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}
