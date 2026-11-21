#![forbid(unsafe_code)]
//! Interactive `ratatui` + `crossterm` TUI surface (`#[cfg(feature = "tui")]`).

pub mod components;
pub mod render;
pub mod state;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use gestalt_kernel::{RunEvent, SteeringKind, SteeringSource};
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
pub use state::{ActiveOverlay, TranscriptItem, TuiAppState, TuiEffect, TuiLayoutRects};

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
    sync_app_from_host(&mut app, &host);

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
                app.apply_run_event(&ev);
            }
            Some(prompt) = pause_rx.recv() => {
                app.open_pause_dialog(prompt);
            }
            turn_done = async {
                match active_turn.as_mut() {
                    Some(h) => (&mut h.done_rx).await,
                    None => std::future::pending().await,
                }
            } => {
                active_turn = None;
                app.is_running = false;
                if let Ok((mut returned_host, res)) = turn_done {
                    while let Ok(ev) = event_rx.try_recv() {
                        app.apply_run_event(&ev);
                    }
                    sync_app_from_host(&mut app, &returned_host);
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
                            app.items.push(TranscriptItem::Notice {
                                text: format!("Error: {err}"),
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
                            } else if let Some(host) = host_slot.as_mut() {
                                match execute_slash_command_idle(
                                    host,
                                    &mut session_state,
                                    parsed,
                                )
                                .await
                                {
                                    Ok(res) => {
                                        app.active_preset = session_state.active_preset.clone();
                                        sync_app_from_host(&mut app, host);
                                        if let Some(pending) = res.pending_pause {
                                            app.open_pause_dialog(PausePrompt {
                                                pause_id: pending.pause_id.clone(),
                                                effect_id: pending.pause_id.clone(),
                                                reason: pending.reason,
                                                payload: pending.payload.clone(),
                                            });
                                            pending_cmd_pause = Some(pending);
                                        } else {
                                            app.items.push(TranscriptItem::Notice { text: res.message });
                                        }
                                        if let Some(notice) = res.notice {
                                            app.items.push(TranscriptItem::Notice { text: notice });
                                        }
                                    }
                                    Err(err) => {
                                        app.items.push(TranscriptItem::Notice {
                                            text: format!("Command error: {err}"),
                                        });
                                    }
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
                        } else if let Some(host) = host_slot.take() {
                            app.items.push(TranscriptItem::UserPrompt { text: cmd.clone() });
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

fn sync_app_from_host(app: &mut TuiAppState, host: &RuntimeHost) {
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
}
