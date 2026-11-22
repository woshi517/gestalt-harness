#![forbid(unsafe_code)]
//! Interactive `ratatui` + `crossterm` TUI surface (`#[cfg(feature = "tui")]`).

pub mod components;
pub mod render;
pub mod state;

use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use gestalt_kernel::{
    NodePayload, RunEvent, SessionNode, SessionTree, SteeringKind, SteeringSource,
};
use gestalt_runtime::{
    CancellationToken, LiveSteeringMessage, LiveTurnControl, RuntimeError, RuntimeHost,
    TurnOutcome, current_epoch_ms,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

pub use components::{
    DockSlot, SelectListItem, SelectListState, TuiComponent, TuiComponentRegistry, TuiDockWidget,
};
pub use render::render_tui;
pub use state::{
    ActiveOverlay, ModelConfigStatus, SelectorKind, SessionFileItem, TranscriptItem, TuiAppState,
    TuiEffect, TuiLayoutRects,
};

use crate::surfaces::{
    ChannelEventSink, InteractivePauseBridge, PausePrompt, PendingCommandPause, SlashCommand,
    SurfaceExitCode, SurfaceSessionState, complete_command_pause, execute_slash_command_idle,
    parse_slash_command, spawn_surface_turn,
};

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture);
    }
}

struct ActiveTuiTurn {
    control_tx: mpsc::UnboundedSender<LiveTurnControl>,
    cancel: CancellationToken,
    done_rx: oneshot::Receiver<(RuntimeHost, Result<TurnOutcome, RuntimeError>)>,
}

/// Launch the interactive TUI surface with optional custom [`TuiComponentRegistry`]
/// overrides/widgets.
///
/// # Errors
///
/// Returns [`RuntimeError`] if terminal initialization or rendering fails.
pub async fn run_tui_surface(
    mut host: RuntimeHost,
    initial_preset: String,
    custom_components: TuiComponentRegistry,
) -> Result<SurfaceExitCode, RuntimeError> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<RunEvent>();
    let (pause_tx, mut pause_rx) = mpsc::unbounded_channel::<PausePrompt>();
    let pause_bridge = Arc::new(InteractivePauseBridge::new(pause_tx));

    host.set_event_sink(Some(Arc::new(ChannelEventSink::new(event_tx))));
    host.set_pause_handler(pause_bridge.clone());

    let ws_label = host
        .workspace_root()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();

    let mut session_state = SurfaceSessionState::new(initial_preset.clone());
    let mut app = TuiAppState::new(ws_label, initial_preset);
    app.hydrate_from_tree(host.kernel().session_tree());
    sync_app_from_host(&mut app, &host).await;

    let mut active_session_path = host.session_path().to_path_buf();
    let mut session_read_offset: u64 =
        std::fs::metadata(&active_session_path).map_or(0, |m| m.len());
    let mut trigger_rx = host.take_trigger_receiver().await;
    let mut host_slot: Option<RuntimeHost> = Some(host);
    let mut active_turn: Option<ActiveTuiTurn> = None;
    let mut pending_cmd_pause: Option<PendingCommandPause> = None;

    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Event>();
    let input_stop = CancellationToken::new();
    let input_stop_clone = input_stop.clone();
    tokio::task::spawn_blocking(move || {
        while !input_stop_clone.is_cancelled() {
            if event::poll(Duration::from_millis(33)).unwrap_or(false)
                && let Ok(ev) = event::read()
                && input_tx.send(ev).is_err()
            {
                break;
            }
        }
    });

    let mut tick = tokio::time::interval(Duration::from_millis(80));
    let mut exit_code = SurfaceExitCode::Success;

    loop {
        terminal.draw(|f| render_tui(f, &mut app, &custom_components))?;

        tokio::select! {
            biased;
            Some(ev) = event_rx.recv() => {
                if matches!(ev, RunEvent::NodeAppended { .. }) {
                    ingest_new_session_lines(&active_session_path, &mut session_read_offset, &mut app);
                }
                app.apply_run_event(&ev);
            }
            Some(prompt) = pause_rx.recv() => {
                app.open_pause_dialog(prompt);
            }
            Some(trig_ev) = async {
                match trigger_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(handle) = active_turn.as_ref() {
                    let _ = handle.control_tx.send(LiveTurnControl::Trigger(trig_ev));
                } else if let Some(mut idle_host) = host_slot.take() {
                    let _ = idle_host.ingest_idle_trigger_event(&trig_ev).await;
                    let target_preset = trig_ev
                        .preset
                        .unwrap_or_else(|| session_state.active_preset.clone());
                    app.is_running = true;
                    let (control_tx, cancel, done_rx) = spawn_surface_turn(
                        idle_host,
                        trig_ev.prompt,
                        target_preset,
                        session_state.extra_decorators.clone(),
                    );
                    active_turn = Some(ActiveTuiTurn {
                        control_tx,
                        cancel,
                        done_rx,
                    });
                }
            }
            turn_done = async {
                match active_turn.as_mut() {
                    Some(h) => (&mut h.done_rx).await,
                    None => std::future::pending().await,
                }
            } => {
                active_turn = None;
                app.is_running = false;
                app.active_status_message = None;
                if let Ok((mut returned_host, res)) = turn_done {
                    while let Ok(ev) = event_rx.try_recv() {
                        if matches!(ev, RunEvent::NodeAppended { .. }) {
                            ingest_new_session_lines(&active_session_path, &mut session_read_offset, &mut app);
                        }
                        app.apply_run_event(&ev);
                    }
                    if let Ok(slice) = returned_host.kernel().session_tree().slice_to(None) {
                        for node in slice.nodes() {
                            app.ingest_node(node);
                        }
                    }
                    session_read_offset = std::fs::metadata(&active_session_path).map_or(session_read_offset, |m| m.len());
                    sync_app_from_host(&mut app, &returned_host).await;
                    match res {
                        Ok(TurnOutcome::Completed { .. }) => {
                            if let Some(follow_up) = returned_host.kernel_mut().pop_next_follow_up() {
                                app.is_running = true;
                                let (control_tx, cancel, done_rx) = spawn_surface_turn(
                                    returned_host,
                                    follow_up.content,
                                    session_state.active_preset.clone(),
                                    session_state.extra_decorators.clone(),
                                );
                                active_turn = Some(ActiveTuiTurn {
                                    control_tx,
                                    cancel,
                                    done_rx,
                                });
                            } else {
                                host_slot = Some(returned_host);
                            }
                        }
                        Ok(TurnOutcome::Aborted { .. }) => {
                            host_slot = Some(returned_host);
                        }
                        Err(err) => {
                            app.items.push(TranscriptItem::ErrorNotice {
                                title: "Turn Error".to_string(),
                                message: err.to_string(),
                                hint: state::classify_error_hint(&err.to_string()),
                            });
                            exit_code = SurfaceExitCode::from_runtime_error(&err);
                            host_slot = Some(returned_host);
                        }
                    }
                }
            }
            Some(ev) = input_rx.recv() => {
                let eff = match ev {
                    Event::Key(key) => app.handle_key_event(key),
                    Event::Mouse(mouse) => app.handle_mouse_event(mouse),
                    _ => TuiEffect::None,
                };
                match eff {
                    TuiEffect::None => {}
                    TuiEffect::Quit => break,
                    TuiEffect::AbortTurn => {
                        if let Some(handle) = active_turn.as_ref() {
                            pause_bridge.deny_all("aborted by user");
                            let _ = handle.control_tx.send(LiveTurnControl::Steering(
                                LiveSteeringMessage {
                                    kind: SteeringKind::Abort,
                                    content: Value::String("Ctrl+C".to_string()),
                                    source: SteeringSource::User,
                                },
                            ));
                            handle.cancel.cancel();
                        }
                    }
                    TuiEffect::ResolvePause { pause_id, resolution } => {
                        if let Some(cmd_pause) = pending_cmd_pause
                            .take()
                            .filter(|p| p.pause_id == pause_id)
                        {
                            if let Some(host) = host_slot.as_mut() {
                                match complete_command_pause(host, &cmd_pause, &resolution) {
                                    Ok(_) => {
                                        sync_app_from_host(&mut app, host).await;
                                        app.items.push(TranscriptItem::Notice {
                                            text: "Stored credentials securely.".to_string(),
                                        });
                                    }
                                    Err(err) => {
                                        app.items.push(TranscriptItem::Notice {
                                            text: format!("Command pause error: {err}"),
                                        });
                                    }
                                }
                            }
                        } else {
                            pause_bridge.resolve(&pause_id, resolution);
                        }
                    }
                    TuiEffect::DenyPause { pause_id, reason } => {
                        if pending_cmd_pause
                            .as_ref()
                            .is_some_and(|p| p.pause_id == pause_id)
                        {
                            pending_cmd_pause = None;
                            app.items.push(TranscriptItem::Notice {
                                text: format!("Cancelled: {reason}"),
                            });
                        } else {
                            pause_bridge.deny(&pause_id, reason);
                        }
                    }
                    TuiEffect::OpenTreeSelector => {
                        if let Some(host) = host_slot.as_ref() {
                            app.open_tree_selector(host.kernel().session_tree());
                        }
                    }
                    TuiEffect::RewindToNode(node_id) => {
                        if let Some(host) = host_slot.as_mut() {
                            match host.rewind_session("main".to_string(), &node_id) {
                                Ok(()) => {
                                    app.hydrate_from_tree(host.kernel().session_tree());
                                    session_read_offset =
                                        std::fs::metadata(&active_session_path).map_or(0, |m| m.len());
                                    sync_app_from_host(&mut app, host).await;
                                    app.items.push(TranscriptItem::Notice {
                                        text: format!(
                                            "Rewound to '{node_id}'. Workspace files on disk were not modified."
                                        ),
                                    });
                                }
                                Err(err) => {
                                    app.items.push(TranscriptItem::ErrorNotice {
                                        title: "Rewind Failed".to_string(),
                                        message: err.to_string(),
                                        hint: None,
                                    });
                                }
                            }
                        }
                    }
                    TuiEffect::ForkFromNode(node_id) => {
                        if let Some(host) = host_slot.as_mut() {
                            let branch = format!("fork_{}", current_epoch_ms());
                            match host.fork_session(branch.clone(), Some(&node_id), None) {
                                Ok(_) => {
                                    app.hydrate_from_tree(host.kernel().session_tree());
                                    session_read_offset =
                                        std::fs::metadata(&active_session_path).map_or(0, |m| m.len());
                                    sync_app_from_host(&mut app, host).await;
                                    app.items.push(TranscriptItem::Notice {
                                        text: format!("Forked branch '{branch}' from '{node_id}'."),
                                    });
                                }
                                Err(err) => {
                                    app.items.push(TranscriptItem::ErrorNotice {
                                        title: "Fork Failed".to_string(),
                                        message: err.to_string(),
                                        hint: None,
                                    });
                                }
                            }
                        }
                    }
                    TuiEffect::SwitchSession(path) => {
                        if let Some(host) = host_slot.as_mut() {
                            match host.switch_session(path.clone()) {
                                Ok(()) => {
                                    active_session_path = host.session_path().to_path_buf();
                                    session_read_offset =
                                        std::fs::metadata(&active_session_path).map_or(0, |m| m.len());
                                    app.hydrate_from_tree(host.kernel().session_tree());
                                    sync_app_from_host(&mut app, host).await;
                                    app.items.push(TranscriptItem::Notice {
                                        text: format!(
                                            "Switched session to '{}'.",
                                            active_session_path.display()
                                        ),
                                    });
                                }
                                Err(err) => {
                                    app.items.push(TranscriptItem::ErrorNotice {
                                        title: "Session Switch Failed".to_string(),
                                        message: err.to_string(),
                                        hint: None,
                                    });
                                }
                            }
                        }
                    }
                    TuiEffect::TriggerAction(cmd) | TuiEffect::SubmitInput(cmd) => {
                        if let Some(parsed) = parse_slash_command(&cmd) {
                            if let Some(handle) = active_turn.as_ref() {
                                match parsed {
                                    SlashCommand::Steer { urgent, message } => {
                                        app.queued_steering.push(message.clone());
                                        let _ = handle.control_tx.send(LiveTurnControl::Steering(
                                            LiveSteeringMessage {
                                                kind: if urgent {
                                                    SteeringKind::UrgentSteer
                                                } else {
                                                    SteeringKind::Steer
                                                },
                                                content: Value::String(message),
                                                source: SteeringSource::User,
                                            },
                                        ));
                                    }
                                    SlashCommand::Stop => {
                                        pause_bridge.deny_all("stopped via /stop");
                                        handle.cancel.cancel();
                                    }
                                    SlashCommand::Mode { preset } => {
                                        let _ = handle.control_tx.send(LiveTurnControl::SwitchMode {
                                            preset_or_loop: preset.clone(),
                                            handoff: Value::Null,
                                            config_override: None,
                                        });
                                        session_state.active_preset = preset;
                                    }
                                    _ => {
                                        app.items.push(TranscriptItem::Notice {
                                            text: "Wait for active Turn to finish before running that command.".to_string(),
                                        });
                                    }
                                }
                            } else {
                                let mut skill_turn_prompt: Option<String> = None;
                                if let Some(host) = host_slot.as_mut() {
                                    match &parsed {
                                        SlashCommand::Tree => {
                                            app.open_tree_selector(host.kernel().session_tree());
                                            continue;
                                        }
                                        SlashCommand::Session => {
                                            let sessions = discover_session_files(host);
                                            app.open_session_selector(
                                                sessions,
                                                host.session_path().to_path_buf(),
                                            );
                                            continue;
                                        }
                                        SlashCommand::Extension { name, args }
                                            if name == "model" && args.trim().is_empty() =>
                                        {
                                            let items = build_model_selector_items(host);
                                            app.open_selector_modal(
                                                "Select Model",
                                                SelectorKind::Model,
                                                items,
                                            );
                                            continue;
                                        }
                                        SlashCommand::Extension { name, args }
                                            if name == "thinking" && args.trim().is_empty() =>
                                        {
                                            app.open_selector_modal(
                                                "Select Thinking Budget",
                                                SelectorKind::Thinking,
                                                build_thinking_selector_items(),
                                            );
                                            continue;
                                        }
                                        _ => {}
                                    }

                                    let was_rewind = matches!(parsed, SlashCommand::Rewind { .. });
                                    match execute_slash_command_idle(
                                        host,
                                        &mut session_state,
                                        parsed,
                                    )
                                    .await
                                    {
                                        Ok(res) => {
                                            if res.data.get("quit").and_then(Value::as_bool)
                                                == Some(true)
                                            {
                                                break;
                                            }
                                            if res.data.get("new_session").and_then(Value::as_bool)
                                                == Some(true)
                                            {
                                                active_session_path =
                                                    host.session_path().to_path_buf();
                                                session_read_offset =
                                                    std::fs::metadata(&active_session_path)
                                                        .map_or(0, |m| m.len());
                                                app.hydrate_from_tree(host.kernel().session_tree());
                                            } else if was_rewind {
                                                app.hydrate_from_tree(host.kernel().session_tree());
                                                session_read_offset =
                                                    std::fs::metadata(&active_session_path)
                                                        .map_or(0, |m| m.len());
                                            }
                                            app.active_preset = session_state.active_preset.clone();
                                            sync_app_from_host(&mut app, host).await;
                                            if let Some(pending) = res.pending_pause {
                                                app.open_pause_dialog(PausePrompt {
                                                    pause_id: pending.pause_id.clone(),
                                                    effect_id: pending.pause_id.clone(),
                                                    reason: pending.reason,
                                                    payload: pending.payload.clone(),
                                                });
                                                pending_cmd_pause = Some(pending);
                                            } else {
                                                app.items.push(TranscriptItem::Notice {
                                                    text: res.message,
                                                });
                                            }
                                            if let Some(notice) = res.notice {
                                                app.items
                                                    .push(TranscriptItem::Notice { text: notice });
                                            }
                                            if res.data.get("loaded_skill").is_some() {
                                                skill_turn_prompt = res
                                                    .data
                                                    .get("steering")
                                                    .and_then(Value::as_str)
                                                    .map(ToString::to_string);
                                            }
                                        }
                                        Err(err) => {
                                            app.items.push(TranscriptItem::ErrorNotice {
                                                title: "Command Error".to_string(),
                                                message: err.to_string(),
                                                hint: state::classify_error_hint(&err.to_string()),
                                            });
                                        }
                                    }
                                }
                                if let Some(prompt_text) = skill_turn_prompt
                                    && check_preflight_model(&mut app)
                                    && let Some(turn_host) = host_slot.take()
                                {
                                    app.is_running = true;
                                    let (control_tx, cancel, done_rx) = spawn_surface_turn(
                                        turn_host,
                                        Value::String(prompt_text),
                                        session_state.active_preset.clone(),
                                        session_state.extra_decorators.clone(),
                                    );
                                    active_turn = Some(ActiveTuiTurn {
                                        control_tx,
                                        cancel,
                                        done_rx,
                                    });
                                }
                            }
                        } else if let Some(handle) = active_turn.as_ref() {
                            app.queued_steering.push(cmd.clone());
                            let _ = handle.control_tx.send(LiveTurnControl::Steering(
                                LiveSteeringMessage {
                                    kind: SteeringKind::Steer,
                                    content: Value::String(cmd),
                                    source: SteeringSource::User,
                                },
                            ));
                        } else if host_slot.is_some() {
                            if !check_preflight_model(&mut app) {
                                app.items.push(TranscriptItem::UserPrompt { text: cmd });
                                continue;
                            }
                            if let Some(host) = host_slot.take() {
                                app.is_running = true;
                                let (control_tx, cancel, done_rx) = spawn_surface_turn(
                                    host,
                                    Value::String(cmd),
                                    session_state.active_preset.clone(),
                                    session_state.extra_decorators.clone(),
                                );
                                active_turn = Some(ActiveTuiTurn {
                                    control_tx,
                                    cancel,
                                    done_rx,
                                });
                            }
                        }
                    }
                }
            }
            _ = tick.tick() => {
                if app.is_running {
                    app.spinner_tick = app.spinner_tick.wrapping_add(1);
                }
            }
        }
    }

    input_stop.cancel();
    if let Some(handle) = active_turn.take() {
        pause_bridge.deny_all("tui exited");
        handle.cancel.cancel();
        if let Ok((returned_host, _)) = handle.done_rx.await {
            returned_host.shutdown().await;
        }
    } else if let Some(h) = host_slot.take() {
        h.shutdown().await;
    }

    Ok(exit_code)
}

async fn sync_app_from_host(app: &mut TuiAppState, host: &RuntimeHost) {
    let gate = host.kernel().policy_gate();
    let policy_summary = json!({
        "rule_ids": gate.rule_ids(),
        "baseline_allowed_tools": gate.baseline_allowed_tools(),
        "sandbox": gate.sandbox(),
        "updated_at_ms": current_epoch_ms(),
    });
    app.sync_from_registry(
        host.kernel().registry(),
        *host.kernel().budget_snapshot(),
        policy_summary,
    );
    app.active_triggers = host.list_triggers().await;
    app.model_status = compute_model_status(host, app.active_model.as_deref());
}

fn compute_model_status(host: &RuntimeHost, active_model: Option<&str>) -> ModelConfigStatus {
    let target = active_model.unwrap_or("default_llm");
    let Ok(resolved) = host.runtime().providers.resolve_target(target) else {
        return match active_model {
            None => ModelConfigStatus::Unconfigured,
            Some(t) => ModelConfigStatus::UnknownTarget {
                target: t.to_string(),
            },
        };
    };

    let provider_id = resolved.provider.id();
    let env_key = resolved
        .config
        .as_ref()
        .and_then(|c| c.env_key.as_deref())
        .or(match provider_id {
            "anthropic" => Some("ANTHROPIC_API_KEY"),
            "openai" => Some("OPENAI_API_KEY"),
            _ => None,
        });
    let requires_auth = matches!(provider_id, "anthropic" | "openai") || env_key.is_some();
    if requires_auth
        && host
            .credentials()
            .get_record(provider_id, env_key)
            .is_none()
    {
        return ModelConfigStatus::MissingCredential {
            provider: provider_id.to_string(),
            target: format!("{provider_id}/{}", resolved.model),
        };
    }

    ModelConfigStatus::Ready {
        target: active_model.map_or_else(
            || format!("{provider_id}/{}", resolved.model),
            ToString::to_string,
        ),
    }
}

fn check_preflight_model(app: &mut TuiAppState) -> bool {
    match &app.model_status {
        ModelConfigStatus::Ready { .. } => true,
        ModelConfigStatus::Unconfigured => {
            app.items.push(TranscriptItem::ErrorNotice {
                title: "No Model Configured".to_string(),
                message: "Cannot start a Turn because no LLM model is configured.".to_string(),
                hint: Some(
                    "Run /model <provider/model> (e.g. /model anthropic/claude-sonnet-4-6) and /login <provider>."
                        .to_string(),
                ),
            });
            false
        }
        ModelConfigStatus::UnknownTarget { target } => {
            app.items.push(TranscriptItem::ErrorNotice {
                title: "Unknown Model Target".to_string(),
                message: format!("Model target '{target}' does not match any registered provider."),
                hint: Some("Run /model to select a valid provider/model.".to_string()),
            });
            false
        }
        ModelConfigStatus::MissingCredential { provider, target } => {
            app.items.push(TranscriptItem::ErrorNotice {
                title: "Missing Credentials".to_string(),
                message: format!(
                    "Model '{target}' requires credentials for provider '{provider}'."
                ),
                hint: Some(format!(
                    "Run /login {provider} or set the provider API key environment variable."
                )),
            });
            false
        }
    }
}

fn ingest_new_session_lines(session_path: &Path, offset: &mut u64, app: &mut TuiAppState) {
    let Ok(mut file) = File::open(session_path) else {
        return;
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return;
    }
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    while let Ok(bytes) = reader.read_line(&mut line) {
        if bytes == 0 {
            break;
        }
        *offset = offset.saturating_add(bytes as u64);
        let trimmed = line.trim();
        if !trimmed.is_empty()
            && let Ok(node) = serde_json::from_str::<SessionNode>(trimmed)
        {
            app.ingest_node(&node);
        }
        line.clear();
    }
}

fn discover_session_files(host: &RuntimeHost) -> Vec<SessionFileItem> {
    let dir = host
        .session_path()
        .parent()
        .unwrap_or_else(|| host.workspace_root());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("session.jsonl")
            .to_string();
        let modified_secs = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let (node_count, turn_count, preview) = match SessionTree::from_jsonl_str(&raw) {
            Ok((tree, _)) => {
                let mut turns = 0usize;
                let mut first_prompt = None;
                for n in tree.all_nodes() {
                    if let Ok(NodePayload::TurnStart { prompt, .. }) = n.decode_payload() {
                        turns += 1;
                        if first_prompt.is_none() {
                            let s = match &prompt {
                                Value::String(t) => t.clone(),
                                other => other.to_string(),
                            };
                            first_prompt = Some(s.chars().take(48).collect::<String>());
                        }
                    }
                }
                (
                    tree.all_nodes().len(),
                    turns,
                    first_prompt.unwrap_or_else(|| "(empty session)".to_string()),
                )
            }
            Err(_) => (0, 0, "(unreadable)".to_string()),
        };
        out.push(SessionFileItem {
            path,
            filename,
            modified_secs,
            turn_count,
            node_count,
            preview,
        });
    }
    out.sort_by_key(|a| std::cmp::Reverse(a.modified_secs));
    out
}

fn build_model_selector_items(host: &RuntimeHost) -> Vec<SelectListItem> {
    let mut items = Vec::new();
    let creds = host.credentials();
    let reg = &host.runtime().providers;

    let known_defaults: &[(&str, &str, &str, &str)] = &[
        (
            "anthropic",
            "anthropic/claude-sonnet-4-6",
            "Anthropic Claude Sonnet 4.6",
            "ANTHROPIC_API_KEY",
        ),
        (
            "anthropic",
            "anthropic/claude-opus-4-6",
            "Anthropic Claude Opus 4.6",
            "ANTHROPIC_API_KEY",
        ),
        (
            "anthropic",
            "anthropic/claude-haiku-4-5",
            "Anthropic Claude Haiku 4.5",
            "ANTHROPIC_API_KEY",
        ),
        ("openai", "openai/gpt-5", "OpenAI GPT-5", "OPENAI_API_KEY"),
        (
            "openai",
            "openai/gpt-5-mini",
            "OpenAI GPT-5 Mini",
            "OPENAI_API_KEY",
        ),
        ("openai", "openai/o3", "OpenAI o3", "OPENAI_API_KEY"),
    ];

    for &(prov, target, desc, env_key) in known_defaults {
        if reg.get_provider(prov).is_some() {
            let status = if creds.get_record(prov, Some(env_key)).is_some() {
                "[ready]"
            } else {
                "[needs /login]"
            };
            items.push(SelectListItem {
                value: format!("/model {target}"),
                label: target.to_string(),
                description: format!("{desc} {status}"),
            });
        }
    }

    for (alias, cfg) in reg.configured_aliases() {
        let target = if cfg.model.is_empty() {
            alias.clone()
        } else {
            format!("{alias}/{}", cfg.model)
        };
        if !items.iter().any(|i| i.label == target) {
            items.push(SelectListItem {
                value: format!("/model {target}"),
                label: target,
                description: "Configured in gestalt.toml".to_string(),
            });
        }
    }
    items
}

fn build_thinking_selector_items() -> Vec<SelectListItem> {
    vec![
        SelectListItem {
            value: "/thinking off".to_string(),
            label: "off".to_string(),
            description: "Disable extended reasoning".to_string(),
        },
        SelectListItem {
            value: "/thinking low".to_string(),
            label: "low".to_string(),
            description: "1,024 thinking tokens".to_string(),
        },
        SelectListItem {
            value: "/thinking medium".to_string(),
            label: "medium".to_string(),
            description: "4,096 thinking tokens".to_string(),
        },
        SelectListItem {
            value: "/thinking high".to_string(),
            label: "high".to_string(),
            description: "16,384 thinking tokens".to_string(),
        },
    ]
}
