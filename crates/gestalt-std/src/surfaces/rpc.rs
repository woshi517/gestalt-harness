#![forbid(unsafe_code)]
//! Headless newline-delimited stdio JSON-RPC 2.0 server (`--mode rpc`).

use std::collections::BTreeMap;
use std::sync::Arc;

use gestalt_kernel::{
    BudgetSnapshot, KernelError, LoopConfig, RunEvent, SteeringKind, SteeringSource, UISurface,
};
use gestalt_runtime::{
    CancellationToken, LiveSteeringMessage, LiveTurnControl, RuntimeError, RuntimeHost,
    TurnOutcome, current_epoch_ms,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::surfaces::{
    ChannelEventSink, InteractivePauseBridge, PausePrompt, PendingCommandPause, SlashCommand,
    SurfaceSessionState, complete_command_pause, execute_slash_command_idle,
    is_known_preset_or_loop, parse_slash_command, spawn_surface_turn, write_json_line,
    write_jsonrpc_err, write_jsonrpc_ok,
};

/// Standard JSON-RPC 2.0 parse error code.
pub const RPC_PARSE_ERROR: i64 = -32700;
/// Standard JSON-RPC 2.0 invalid request code.
pub const RPC_INVALID_REQUEST: i64 = -32600;
/// Standard JSON-RPC 2.0 method not found code.
pub const RPC_METHOD_NOT_FOUND: i64 = -32601;
/// Standard JSON-RPC 2.0 invalid params code.
pub const RPC_INVALID_PARAMS: i64 = -32602;
/// Standard JSON-RPC 2.0 internal error code.
pub const RPC_INTERNAL_ERROR: i64 = -32603;
/// Gestalt policy/sandbox/pause denied error code.
pub const RPC_POLICY_DENIED: i64 = -32001;
/// Gestalt session locked error code.
pub const RPC_SESSION_LOCKED: i64 = -32002;
/// Gestalt invalid state error code (e.g. mutating session while Turn is active).
pub const RPC_INVALID_STATE: i64 = -32003;

struct ActiveTurnHandle {
    req_id: Option<Value>,
    control_tx: mpsc::UnboundedSender<LiveTurnControl>,
    cancel: CancellationToken,
    done_rx: oneshot::Receiver<(RuntimeHost, Result<TurnOutcome, RuntimeError>)>,
}

struct CachedSurfaceSnapshot {
    presets: Vec<String>,
    commands: Value,
    ui: UISurface,
    budget: BudgetSnapshot,
}

impl CachedSurfaceSnapshot {
    fn from_host(host: &RuntimeHost) -> Self {
        let reg = host.kernel().registry();
        Self {
            presets: reg.presets.keys().cloned().collect(),
            commands: json!(reg.commands),
            ui: reg.ui.clone(),
            budget: *host.kernel().budget_snapshot(),
        }
    }
}

/// Run the headless JSON-RPC 2.0 server over `reader` and `writer` until EOF.
///
/// # Errors
///
/// Returns [`RuntimeError`] on fatal stream write failure.
pub async fn run_rpc_surface<R, W>(
    mut host: RuntimeHost,
    initial_preset: String,
    reader: R,
    mut writer: W,
) -> Result<(), RuntimeError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<RunEvent>();
    let (pause_tx, _pause_rx) = mpsc::unbounded_channel::<PausePrompt>();
    let pause_bridge = Arc::new(InteractivePauseBridge::new(pause_tx));

    host.set_event_sink(Some(Arc::new(ChannelEventSink::new(event_tx))));
    host.set_pause_handler(pause_bridge.clone());

    let mut state = SurfaceSessionState::new(initial_preset);
    let mut snapshot = CachedSurfaceSnapshot::from_host(&host);
    let mut trigger_rx = host.take_trigger_receiver().await;
    let mut host_slot: Option<RuntimeHost> = Some(host);
    let mut active_turn: Option<ActiveTurnHandle> = None;
    let mut pending_cmd_pauses: BTreeMap<String, PendingCommandPause> = BTreeMap::new();
    let mut lines = reader.lines();

    loop {
        tokio::select! {
            biased;
            Some(ev) = event_rx.recv() => {
                write_json_line(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "event/notify",
                        "params": ev,
                    }),
                ).await?;
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
                        .unwrap_or_else(|| state.active_preset.clone());
                    let (control_tx, cancel, done_rx) = spawn_surface_turn(
                        idle_host,
                        trig_ev.prompt,
                        target_preset,
                        state.extra_decorators.clone(),
                    );
                    active_turn = Some(ActiveTurnHandle {
                        req_id: None,
                        control_tx,
                        cancel,
                        done_rx,
                    });
                }
            }
            turn_done = async {
                match active_turn.as_mut() {
                    Some(handle) => (&mut handle.done_rx).await,
                    None => std::future::pending().await,
                }
            } => {
                let handle = active_turn.take();
                let req_id = handle.and_then(|h| h.req_id);
                if let Ok((mut returned_host, res)) = turn_done {
                    while let Ok(ev) = event_rx.try_recv() {
                        write_json_line(
                            &mut writer,
                            &json!({
                                "jsonrpc": "2.0",
                                "method": "event/notify",
                                "params": ev,
                            }),
                        ).await?;
                    }
                    snapshot = CachedSurfaceSnapshot::from_host(&returned_host);
                    match res {
                        Ok(outcome) => {
                            if let Some(id) = req_id {
                                let payload = match &outcome {
                                    TurnOutcome::Completed { turn_id, output, usage } => json!({
                                        "status": "completed",
                                        "turn_id": turn_id,
                                        "output": output,
                                        "usage": usage,
                                    }),
                                    TurnOutcome::Aborted { turn_id, reason } => json!({
                                        "status": "aborted",
                                        "turn_id": turn_id,
                                        "reason": reason,
                                    }),
                                };
                                write_jsonrpc_ok(&mut writer, id, payload).await?;
                            }
                            if let Some(follow_up) = returned_host.kernel_mut().pop_next_follow_up() {
                                let (control_tx, cancel, done_rx) = spawn_surface_turn(
                                    returned_host,
                                    follow_up.content,
                                    state.active_preset.clone(),
                                    state.extra_decorators.clone(),
                                );
                                active_turn = Some(ActiveTurnHandle {
                                    req_id: None,
                                    control_tx,
                                    cancel,
                                    done_rx,
                                });
                            } else {
                                host_slot = Some(returned_host);
                            }
                        }
                        Err(err) => {
                            if let Some(id) = req_id {
                                let (code, msg) = map_runtime_error(&err);
                                write_jsonrpc_err(&mut writer, id, code, &msg).await?;
                            }
                            host_slot = Some(returned_host);
                        }
                    }
                }
            }
            line_res = lines.next_line() => {
                let line = match line_res {
                    Ok(Some(l)) => l,
                    Ok(None) => break,
                    Err(err) => return Err(err.into()),
                };
                if line.trim().is_empty() {
                    continue;
                }
                let msg: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(err) => {
                        write_jsonrpc_err(
                            &mut writer,
                            Value::Null,
                            RPC_PARSE_ERROR,
                            &format!("invalid JSON: {err}"),
                        ).await?;
                        continue;
                    }
                };
                let id = msg.get("id").cloned();
                let Some(method) = msg.get("method").and_then(Value::as_str) else {
                    if let Some(req_id) = id {
                        write_jsonrpc_err(&mut writer, req_id, RPC_INVALID_REQUEST, "missing method").await?;
                    }
                    continue;
                };
                let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

                handle_rpc_request(
                    method,
                    params,
                    id,
                    &mut writer,
                    &mut host_slot,
                    &mut active_turn,
                    &mut state,
                    &mut snapshot,
                    &pause_bridge,
                    &mut pending_cmd_pauses,
                ).await?;
            }
        }
    }

    if let Some(handle) = active_turn.take() {
        pause_bridge.deny_all("rpc stdin closed");
        handle.cancel.cancel();
        if let Ok((returned_host, _)) = handle.done_rx.await {
            returned_host.shutdown().await;
        }
    } else if let Some(h) = host_slot.take() {
        h.shutdown().await;
    }
    Ok(())
}

/// Convenience wrapper running [`run_rpc_surface`] against `stdin` and `stdout`.
///
/// # Errors
///
/// Returns [`RuntimeError`] on fatal I/O failure.
pub async fn run_rpc_stdio(host: RuntimeHost, initial_preset: String) -> Result<(), RuntimeError> {
    let reader = BufReader::new(tokio::io::stdin());
    let writer = tokio::io::stdout();
    run_rpc_surface(host, initial_preset, reader, writer).await
}

#[allow(clippy::too_many_arguments)]
async fn handle_rpc_request<W: AsyncWrite + Unpin>(
    method: &str,
    params: Value,
    id: Option<Value>,
    writer: &mut W,
    host_slot: &mut Option<RuntimeHost>,
    active_turn: &mut Option<ActiveTurnHandle>,
    state: &mut SurfaceSessionState,
    snapshot: &mut CachedSurfaceSnapshot,
    pause_bridge: &Arc<InteractivePauseBridge>,
    pending_cmd_pauses: &mut BTreeMap<String, PendingCommandPause>,
) -> Result<(), RuntimeError> {
    let reply_id = id.clone().unwrap_or(Value::Null);

    match method {
        "turn/start" => {
            let Some(prompt) = params.get("prompt").cloned() else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, RPC_INVALID_PARAMS, "missing 'prompt'")
                        .await?;
                }
                return Ok(());
            };
            let preset = params
                .get("preset")
                .and_then(Value::as_str)
                .map_or_else(|| state.active_preset.clone(), str::to_string);

            if let Some(host) = host_slot.take() {
                if !is_known_preset_or_loop(host.kernel().registry(), &preset) {
                    *host_slot = Some(host);
                    if id.is_some() {
                        write_jsonrpc_err(
                            writer,
                            reply_id,
                            RPC_INVALID_PARAMS,
                            &format!("unknown preset or loop '{preset}'"),
                        )
                        .await?;
                    }
                    return Ok(());
                }
                state.active_preset = preset.clone();
                let (control_tx, cancel, done_rx) =
                    spawn_surface_turn(host, prompt, preset, state.extra_decorators.clone());
                *active_turn = Some(ActiveTurnHandle {
                    req_id: id,
                    control_tx,
                    cancel,
                    done_rx,
                });
            } else if params.get("follow_up").and_then(Value::as_bool) == Some(true)
                && let Some(handle) = active_turn.as_ref()
            {
                let _ = handle
                    .control_tx
                    .send(LiveTurnControl::Steering(LiveSteeringMessage {
                        kind: SteeringKind::FollowUp,
                        content: prompt,
                        source: SteeringSource::User,
                    }));
                if id.is_some() {
                    write_jsonrpc_ok(writer, reply_id, json!({ "queued": "follow_up" })).await?;
                }
            } else if id.is_some() {
                write_jsonrpc_err(
                    writer,
                    reply_id,
                    RPC_INVALID_STATE,
                    "a Turn is already active",
                )
                .await?;
            }
        }
        "turn/steer" => {
            let Some(content) = params.get("content").cloned() else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, RPC_INVALID_PARAMS, "missing 'content'")
                        .await?;
                }
                return Ok(());
            };
            let kind = match params
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("steer")
            {
                "urgent" | "urgent_steer" => SteeringKind::UrgentSteer,
                "follow_up" => SteeringKind::FollowUp,
                "abort" => SteeringKind::Abort,
                _ => SteeringKind::Steer,
            };
            if let Some(handle) = active_turn.as_ref() {
                let _ = handle
                    .control_tx
                    .send(LiveTurnControl::Steering(LiveSteeringMessage {
                        kind,
                        content,
                        source: SteeringSource::User,
                    }));
                if id.is_some() {
                    write_jsonrpc_ok(writer, reply_id, json!({ "queued": true, "kind": kind }))
                        .await?;
                }
            } else if let Some(host) = host_slot.as_mut() {
                match host.kernel_mut().enqueue_steering(
                    kind,
                    content,
                    SteeringSource::User,
                    current_epoch_ms(),
                ) {
                    Ok(sig) => {
                        let _ = host.flush_pending();
                        if id.is_some() {
                            write_jsonrpc_ok(
                                writer,
                                reply_id,
                                json!({ "queued": true, "signal_id": sig.id }),
                            )
                            .await?;
                        }
                    }
                    Err(err) => {
                        if id.is_some() {
                            write_jsonrpc_err(
                                writer,
                                reply_id,
                                RPC_INTERNAL_ERROR,
                                &err.to_string(),
                            )
                            .await?;
                        }
                    }
                }
            }
        }
        "turn/cancel" => {
            if let Some(handle) = active_turn.as_ref() {
                pause_bridge.deny_all("turn cancelled");
                let _ = handle
                    .control_tx
                    .send(LiveTurnControl::Steering(LiveSteeringMessage {
                        kind: SteeringKind::Abort,
                        content: Value::String("cancelled via rpc".to_string()),
                        source: SteeringSource::User,
                    }));
                handle.cancel.cancel();
                if id.is_some() {
                    write_jsonrpc_ok(writer, reply_id, json!({ "cancelled": true })).await?;
                }
            } else if id.is_some() {
                write_jsonrpc_ok(writer, reply_id, json!({ "cancelled": false })).await?;
            }
        }
        "phase/switch" => {
            let Some(preset) = params.get("preset").and_then(Value::as_str) else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, RPC_INVALID_PARAMS, "missing 'preset'")
                        .await?;
                }
                return Ok(());
            };
            let handoff = params.get("handoff").cloned().unwrap_or(Value::Null);
            let config_override: Option<LoopConfig> = params
                .get("config")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok());
            if let Some(handle) = active_turn.as_ref() {
                let _ = handle.control_tx.send(LiveTurnControl::SwitchMode {
                    preset_or_loop: preset.to_string(),
                    handoff,
                    config_override,
                });
                state.active_preset = preset.to_string();
                if id.is_some() {
                    write_jsonrpc_ok(
                        writer,
                        reply_id,
                        json!({ "switched": true, "preset": preset }),
                    )
                    .await?;
                }
            } else if let Some(host) = host_slot.as_mut() {
                if !is_known_preset_or_loop(host.kernel().registry(), preset) {
                    if id.is_some() {
                        write_jsonrpc_err(
                            writer,
                            reply_id,
                            RPC_INVALID_PARAMS,
                            &format!("unknown preset or loop '{preset}'"),
                        )
                        .await?;
                    }
                    return Ok(());
                }
                state.active_preset = preset.to_string();
                if id.is_some() {
                    write_jsonrpc_ok(
                        writer,
                        reply_id,
                        json!({ "switched": true, "preset": preset }),
                    )
                    .await?;
                }
            }
        }
        "pause/resolve" => {
            let Some(pause_id) = params.get("pause_id").and_then(Value::as_str) else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, RPC_INVALID_PARAMS, "missing 'pause_id'")
                        .await?;
                }
                return Ok(());
            };
            let resolution = params.get("resolution").cloned().unwrap_or(Value::Null);
            if let Some(cmd_pause) = pending_cmd_pauses.remove(pause_id) {
                if let Some(host) = host_slot.as_mut() {
                    match complete_command_pause(host, &cmd_pause, &resolution) {
                        Ok(data) => {
                            if id.is_some() {
                                write_jsonrpc_ok(
                                    writer,
                                    reply_id,
                                    json!({ "resolved": true, "data": data }),
                                )
                                .await?;
                            }
                        }
                        Err(err) => {
                            if id.is_some() {
                                let (code, msg) = map_runtime_error(&err);
                                write_jsonrpc_err(writer, reply_id, code, &msg).await?;
                            }
                        }
                    }
                }
                return Ok(());
            }
            let ok = pause_bridge.resolve(pause_id, resolution);
            if id.is_some() {
                if ok {
                    write_jsonrpc_ok(writer, reply_id, json!({ "resolved": true })).await?;
                } else {
                    write_jsonrpc_err(
                        writer,
                        reply_id,
                        RPC_INVALID_PARAMS,
                        &format!("unknown or expired pause_id '{pause_id}'"),
                    )
                    .await?;
                }
            }
        }
        "session/fork" => {
            let Some(host) = host_slot.as_mut() else {
                if id.is_some() {
                    write_jsonrpc_err(
                        writer,
                        reply_id,
                        RPC_INVALID_STATE,
                        "cannot fork while a Turn is active",
                    )
                    .await?;
                }
                return Ok(());
            };
            let from_node = params
                .get("from_node_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            let new_file = params
                .get("new_file")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            match execute_slash_command_idle(
                host,
                state,
                SlashCommand::Fork {
                    node_id: from_node,
                    new_file,
                },
            )
            .await
            {
                Ok(res) => {
                    if id.is_some() {
                        write_jsonrpc_ok(writer, reply_id, res.data).await?;
                    }
                }
                Err(err) => {
                    if id.is_some() {
                        let (code, msg) = map_runtime_error(&err);
                        write_jsonrpc_err(writer, reply_id, code, &msg).await?;
                    }
                }
            }
        }
        "session/rewind" => {
            let Some(host) = host_slot.as_mut() else {
                if id.is_some() {
                    write_jsonrpc_err(
                        writer,
                        reply_id,
                        RPC_INVALID_STATE,
                        "cannot rewind while a Turn is active",
                    )
                    .await?;
                }
                return Ok(());
            };
            let Some(target) = params.get("target_node_id").and_then(Value::as_str) else {
                if id.is_some() {
                    write_jsonrpc_err(
                        writer,
                        reply_id,
                        RPC_INVALID_PARAMS,
                        "missing 'target_node_id'",
                    )
                    .await?;
                }
                return Ok(());
            };
            match execute_slash_command_idle(
                host,
                state,
                SlashCommand::Rewind {
                    node_id: target.to_string(),
                },
            )
            .await
            {
                Ok(res) => {
                    if id.is_some() {
                        write_jsonrpc_ok(writer, reply_id, res.data).await?;
                    }
                }
                Err(err) => {
                    if id.is_some() {
                        let (code, msg) = map_runtime_error(&err);
                        write_jsonrpc_err(writer, reply_id, code, &msg).await?;
                    }
                }
            }
        }
        "extension/reload" => {
            let Some(host) = host_slot.as_mut() else {
                if id.is_some() {
                    write_jsonrpc_err(
                        writer,
                        reply_id,
                        RPC_INVALID_STATE,
                        "cannot reload extensions while a Turn is active",
                    )
                    .await?;
                }
                return Ok(());
            };
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, RPC_INVALID_PARAMS, "missing 'name'")
                        .await?;
                }
                return Ok(());
            };
            match host.reload_external_extension(name).await {
                Ok(()) => {
                    *snapshot = CachedSurfaceSnapshot::from_host(host);
                    if id.is_some() {
                        write_jsonrpc_ok(writer, reply_id, json!({ "reloaded": name })).await?;
                    }
                }
                Err(err) => {
                    if id.is_some() {
                        let (code, msg) = map_runtime_error(&err);
                        write_jsonrpc_err(writer, reply_id, code, &msg).await?;
                    }
                }
            }
        }
        "command/run" => {
            let Some(host) = host_slot.as_mut() else {
                if id.is_some() {
                    write_jsonrpc_err(
                        writer,
                        reply_id,
                        RPC_INVALID_STATE,
                        "cannot run commands while a Turn is active",
                    )
                    .await?;
                }
                return Ok(());
            };
            let Some(cmd_name) = params.get("command").and_then(Value::as_str) else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, RPC_INVALID_PARAMS, "missing 'command'")
                        .await?;
                }
                return Ok(());
            };
            let args = params.get("args").and_then(Value::as_str).unwrap_or("");
            let raw_cmd = if cmd_name.starts_with('/') {
                format!("{cmd_name} {args}")
            } else {
                format!("/{cmd_name} {args}")
            };
            let Some(parsed) = parse_slash_command(&raw_cmd) else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, RPC_INVALID_PARAMS, "invalid command")
                        .await?;
                }
                return Ok(());
            };
            match execute_slash_command_idle(host, state, parsed).await {
                Ok(res) => {
                    if let Some(pending) = res.pending_pause {
                        let ev = RunEvent::Paused {
                            pause_id: pending.pause_id.clone(),
                            effect_id: pending.pause_id.clone(),
                            reason: pending.reason,
                            payload: pending.payload.clone(),
                        };
                        pending_cmd_pauses.insert(pending.pause_id.clone(), pending);
                        write_json_line(
                            writer,
                            &json!({
                                "jsonrpc": "2.0",
                                "method": "event/notify",
                                "params": ev,
                            }),
                        )
                        .await?;
                    }
                    if id.is_some() {
                        write_jsonrpc_ok(writer, reply_id, res.data).await?;
                    }
                }
                Err(err) => {
                    if id.is_some() {
                        let (code, msg) = map_runtime_error(&err);
                        write_jsonrpc_err(writer, reply_id, code, &msg).await?;
                    }
                }
            }
        }
        "surface/describe" => {
            if let Some(host) = host_slot.as_ref() {
                *snapshot = CachedSurfaceSnapshot::from_host(host);
            }
            if id.is_some() {
                write_jsonrpc_ok(
                    writer,
                    reply_id,
                    json!({
                        "active_preset": state.active_preset,
                        "presets": snapshot.presets,
                        "commands": snapshot.commands,
                        "ui": snapshot.ui,
                        "budget": snapshot.budget,
                    }),
                )
                .await?;
            }
        }
        "trigger/list" | "trigger/add" | "trigger/remove" | "trigger/pause" | "trigger/resume"
        | "trigger/fire" => {
            let Some(host) = host_slot.as_ref() else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, RPC_INVALID_STATE, "a Turn is active")
                        .await?;
                }
                return Ok(());
            };
            let op = method.strip_prefix("trigger/").unwrap_or(method);
            match super::dispatch_trigger_rpc(host, op, params).await {
                Ok(val) if id.is_some() => write_jsonrpc_ok(writer, reply_id, val).await?,
                Err((code, msg)) if id.is_some() => {
                    write_jsonrpc_err(writer, reply_id, code, &msg).await?;
                }
                _ => {}
            }
        }
        _ => {
            if id.is_some() {
                write_jsonrpc_err(
                    writer,
                    reply_id,
                    RPC_METHOD_NOT_FOUND,
                    &format!("method not found: {method}"),
                )
                .await?;
            }
        }
    }

    Ok(())
}

fn map_runtime_error(err: &RuntimeError) -> (i64, String) {
    let code = match err {
        RuntimeError::SessionLocked { .. } => RPC_SESSION_LOCKED,
        RuntimeError::UntrustedWorkspace { .. }
        | RuntimeError::HeadlessPauseDenied(_)
        | RuntimeError::SandboxViolation(_)
        | RuntimeError::Kernel(KernelError::PolicyRelaxationForbidden(_)) => RPC_POLICY_DENIED,
        RuntimeError::Kernel(KernelError::InvalidState(_)) => RPC_INVALID_STATE,
        RuntimeError::Kernel(KernelError::UnresolvedExtensionSlot { .. }) => RPC_INVALID_PARAMS,
        _ => RPC_INTERNAL_ERROR,
    };
    (code, err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, BudgetCap, ContextBuilder, Effect, ExtensionRegistry,
        LoopAction, LoopConfig, LoopContext, ModelCapabilities, PauseReason, PolicyGate,
        SandboxPolicy, SessionSlice, StepResult,
    };
    use gestalt_runtime::sandbox::UnsandboxedFallbackDriver;
    use gestalt_runtime::{CredentialStore, HeadlessPauseHandler, RuntimeRegistry};
    use std::fs;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    struct PauseAndDoneLoop {
        step: usize,
    }
    impl AgentLoop for PauseAndDoneLoop {
        fn id(&self) -> &str {
            "p_loop"
        }
        fn digest(&self) -> &str {
            "p1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            _results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            if self.step == 0 {
                self.step += 1;
                return LoopAction::Yield(vec![Effect::Pause {
                    reason: PauseReason::Approval,
                    payload: json!({ "action": "promote" }),
                }]);
            }
            LoopAction::Done(json!("approved_done"))
        }
    }
    struct PauseFactory;
    impl AgentLoopFactory for PauseFactory {
        fn id(&self) -> &str {
            "p_loop"
        }
        fn digest(&self) -> &str {
            "p1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(PauseAndDoneLoop { step: 0 })
        }
    }
    struct NullBuilder;
    impl ContextBuilder for NullBuilder {
        fn id(&self) -> &str {
            "default"
        }
        fn digest(&self) -> &str {
            "b1"
        }
        fn build(
            &self,
            _s: &SessionSlice,
            _a: &gestalt_kernel::ArtifactStoreView,
            _c: &ModelCapabilities,
        ) -> Value {
            Value::Null
        }
    }

    #[tokio::test]
    async fn rpc_surface_handles_turn_pause_resolve_fork_rewind_and_errors() {
        let dir = std::env::temp_dir().join(format!("gestalt_rpc_surf_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");

        let mut reg = ExtensionRegistry::new();
        reg.loops
            .insert("p_loop".into(), std::sync::Arc::new(PauseFactory));
        reg.builders
            .insert("default".into(), std::sync::Arc::new(NullBuilder));
        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let rt = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
        let host = RuntimeHost::new(
            "r_rpc".into(),
            &dir,
            dir.join(".gestalt/sessions/rpc.jsonl"),
            reg,
            gate,
            rt,
            CredentialStore::default(),
            None,
            Arc::new(HeadlessPauseHandler::default()),
        )
        .expect("host");

        let (client_in, server_in) = tokio::io::duplex(8192);
        let (server_out, client_out) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            run_rpc_surface(host, "p_loop".into(), BufReader::new(server_in), server_out).await
        });

        let mut cin = client_in;
        let mut cout = BufReader::new(client_out).lines();

        // 1. surface/describe
        cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"surface/describe\"}\n")
            .await
            .unwrap();
        let desc: Value = serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(desc["result"]["active_preset"], "p_loop");

        // 2. Unknown method -> -32601
        cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"no/such\"}\n")
            .await
            .unwrap();
        let unk: Value = serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(unk["error"]["code"], RPC_METHOD_NOT_FOUND);

        // 3. turn/start -> wait for paused event -> pause/resolve -> turn completes
        cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"turn/start\",\"params\":{\"prompt\":\"go\"}}\n")
            .await
            .unwrap();

        let pause_id = loop {
            let line = tokio::time::timeout(Duration::from_secs(2), cout.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["method"] == "event/notify" && v["params"]["event"] == "paused" {
                break v["params"]["pause_id"].as_str().unwrap().to_string();
            }
        };

        let resolve_req = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "pause/resolve",
            "params": { "pause_id": pause_id, "resolution": { "approved": true } }
        });
        cin.write_all(format!("{resolve_req}\n").as_bytes())
            .await
            .unwrap();

        let mut saw_resolve_ok = false;
        let mut saw_turn_done = false;
        while !(saw_resolve_ok && saw_turn_done) {
            let line = tokio::time::timeout(Duration::from_secs(2), cout.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == 4 {
                assert_eq!(v["result"]["resolved"], true);
                saw_resolve_ok = true;
            } else if v["id"] == 3 {
                assert_eq!(v["result"]["status"], "completed");
                assert_eq!(v["result"]["output"], "approved_done");
                saw_turn_done = true;
            }
        }

        // 4. session/fork
        cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"session/fork\",\"params\":{\"new_file\":true}}\n")
            .await
            .unwrap();
        loop {
            let line = cout.next_line().await.unwrap().unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == 5 {
                assert!(v["result"]["branch_id"].as_str().is_some());
                break;
            }
        }

        drop(cin);
        let _ = server.await.unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
