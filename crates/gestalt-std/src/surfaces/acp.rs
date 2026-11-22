#![forbid(unsafe_code)]
//! Agent Client Protocol v1 (`--mode acp`) stdio JSON-RPC 2.0 server.

use std::collections::BTreeMap;
use std::sync::Arc;

use gestalt_kernel::{ExtensionRegistry, PauseReason, RunEvent, SteeringKind, SteeringSource};
use gestalt_runtime::{
    CancellationToken, LiveSteeringMessage, LiveTurnControl, RuntimeError, RuntimeHost,
    TurnOutcome, current_epoch_ms,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::surfaces::{
    ChannelEventSink, InteractivePauseBridge, PausePrompt, PendingCommandPause, SlashCommand,
    SurfaceSessionState, complete_command_pause, execute_slash_command_idle, extract_stream_text,
    is_known_preset_or_loop, parse_slash_command, spawn_surface_turn, write_json_line,
    write_jsonrpc_err, write_jsonrpc_ok,
};

struct ActiveAcpTurn {
    prompt_req_id: Value,
    control_tx: mpsc::UnboundedSender<LiveTurnControl>,
    cancel: CancellationToken,
    done_rx: oneshot::Receiver<(RuntimeHost, Result<TurnOutcome, RuntimeError>)>,
}

/// Run the ACP v1 server over `reader` and `writer` until EOF.
///
/// # Errors
///
/// Returns [`RuntimeError`] on fatal stream write failure.
pub async fn run_acp_surface<R, W>(
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
    let (pause_tx, mut pause_rx) = mpsc::unbounded_channel::<PausePrompt>();
    let pause_bridge = Arc::new(InteractivePauseBridge::new(pause_tx));

    host.set_event_sink(Some(Arc::new(ChannelEventSink::new(event_tx))));
    host.set_pause_handler(pause_bridge.clone());

    let session_id = host
        .session_path()
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("gestalt-session")
        .to_string();

    let mut state = SurfaceSessionState::new(initial_preset);
    let mut available_modes = collect_modes(host.kernel().registry());
    let mut trigger_rx = host.take_trigger_receiver().await;
    let mut host_slot: Option<RuntimeHost> = Some(host);
    let mut active_turn: Option<ActiveAcpTurn> = None;
    let mut perm_seq: u64 = 0;
    let mut pending_perm_requests: BTreeMap<String, String> = BTreeMap::new();
    let mut pending_cmd_pauses: BTreeMap<String, PendingCommandPause> = BTreeMap::new();
    let mut lines = reader.lines();

    loop {
        tokio::select! {
            biased;
            Some(ev) = event_rx.recv() => {
                if let Some(update) = run_event_to_acp_update(&ev) {
                    write_json_line(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": session_id,
                                "update": update,
                            }
                        }),
                    ).await?;
                }
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
                    active_turn = Some(ActiveAcpTurn {
                        prompt_req_id: Value::Null,
                        control_tx,
                        cancel,
                        done_rx,
                    });
                }
            }
            Some(prompt) = pause_rx.recv() => {
                perm_seq += 1;
                let req_id = format!("acp_perm_{perm_seq}");
                pending_perm_requests.insert(req_id.clone(), prompt.pause_id.clone());
                let title = prompt
                    .payload
                    .get("prompt")
                    .or_else(|| prompt.payload.get("title"))
                    .and_then(Value::as_str)
                    .unwrap_or("Approve Gestalt action")
                    .to_string();
                write_json_line(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "method": "session/request_permission",
                        "params": {
                            "sessionId": session_id,
                            "toolCall": {
                                "toolCallId": prompt.effect_id,
                                "title": title,
                                "kind": match prompt.reason {
                                    PauseReason::Approval => "edit",
                                    PauseReason::Input => "other",
                                    PauseReason::Checkpoint => "think",
                                },
                            },
                            "options": [
                                { "optionId": "allow_once", "name": "Allow once", "kind": "allow_once" },
                                { "optionId": "reject_once", "name": "Reject", "kind": "reject_once" }
                            ],
                            "_meta": {
                                "pause_id": prompt.pause_id,
                                "reason": prompt.reason,
                                "payload": prompt.payload,
                            }
                        }
                    }),
                ).await?;
            }
            turn_done = async {
                match active_turn.as_mut() {
                    Some(handle) => (&mut handle.done_rx).await,
                    None => std::future::pending().await,
                }
            } => {
                let handle = active_turn.take();
                if let Ok((mut returned_host, res)) = turn_done {
                    while let Ok(ev) = event_rx.try_recv() {
                        if let Some(update) = run_event_to_acp_update(&ev) {
                            write_json_line(
                                &mut writer,
                                &json!({
                                    "jsonrpc": "2.0",
                                    "method": "session/update",
                                    "params": {
                                        "sessionId": session_id,
                                        "update": update,
                                    }
                                }),
                            ).await?;
                        }
                    }
                    available_modes = collect_modes(returned_host.kernel().registry());
                    if let Some(h) = handle {
                        match res {
                            Ok(TurnOutcome::Completed { .. }) => {
                                write_jsonrpc_ok(&mut writer, h.prompt_req_id, json!({ "stopReason": "end_turn" })).await?;
                            }
                            Ok(TurnOutcome::Aborted { .. }) => {
                                write_jsonrpc_ok(&mut writer, h.prompt_req_id, json!({ "stopReason": "cancelled" })).await?;
                            }
                            Err(RuntimeError::HeadlessPauseDenied(_)) => {
                                write_jsonrpc_ok(&mut writer, h.prompt_req_id, json!({ "stopReason": "refusal" })).await?;
                            }
                            Err(err) => {
                                write_jsonrpc_err(&mut writer, h.prompt_req_id, -32603, &err.to_string()).await?;
                            }
                        }
                    }
                    if let Some(follow_up) = returned_host.kernel_mut().pop_next_follow_up() {
                        let (control_tx, cancel, done_rx) = spawn_surface_turn(
                            returned_host,
                            follow_up.content,
                            state.active_preset.clone(),
                            state.extra_decorators.clone(),
                        );
                        active_turn = Some(ActiveAcpTurn {
                            prompt_req_id: Value::Null,
                            control_tx,
                            cancel,
                            done_rx,
                        });
                    } else {
                        host_slot = Some(returned_host);
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
                        write_jsonrpc_err(&mut writer, Value::Null, -32700, &format!("invalid JSON: {err}")).await?;
                        continue;
                    }
                };

                // Handle client responses to server-initiated `session/request_permission`
                if msg.get("method").is_none()
                    && let Some(resp_id) = msg.get("id").and_then(Value::as_str)
                    && let Some(pause_id) = pending_perm_requests.remove(resp_id)
                {
                    if let Some(cmd_pause) = pending_cmd_pauses.remove(&pause_id)
                        && let Some(host) = host_slot.as_mut()
                        && let Some(res) = msg.get("result")
                    {
                        let _ = complete_command_pause(host, &cmd_pause, res);
                    } else {
                        resolve_permission_response(&pause_bridge, &pause_id, &msg);
                    }
                    continue;
                }

                let id = msg.get("id").cloned();
                let Some(method) = msg.get("method").and_then(Value::as_str) else {
                    continue;
                };
                let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

                handle_acp_method(
                    method,
                    params,
                    id,
                    &session_id,
                    &mut writer,
                    &mut host_slot,
                    &mut active_turn,
                    &mut state,
                    &mut available_modes,
                    &pause_bridge,
                    &mut perm_seq,
                    &mut pending_perm_requests,
                    &mut pending_cmd_pauses,
                ).await?;
            }
        }
    }

    if let Some(handle) = active_turn.take() {
        pause_bridge.deny_all("acp stdin closed");
        handle.cancel.cancel();
        if let Ok((returned_host, _)) = handle.done_rx.await {
            returned_host.shutdown().await;
        }
    } else if let Some(h) = host_slot.take() {
        h.shutdown().await;
    }
    Ok(())
}

/// Convenience wrapper running [`run_acp_surface`] against `stdin` and `stdout`.
///
/// # Errors
///
/// Returns [`RuntimeError`] on fatal I/O failure.
pub async fn run_acp_stdio(host: RuntimeHost, initial_preset: String) -> Result<(), RuntimeError> {
    let reader = BufReader::new(tokio::io::stdin());
    let writer = tokio::io::stdout();
    run_acp_surface(host, initial_preset, reader, writer).await
}

#[allow(clippy::too_many_arguments)]
async fn handle_acp_method<W: AsyncWrite + Unpin>(
    method: &str,
    params: Value,
    id: Option<Value>,
    session_id: &str,
    writer: &mut W,
    host_slot: &mut Option<RuntimeHost>,
    active_turn: &mut Option<ActiveAcpTurn>,
    state: &mut SurfaceSessionState,
    available_modes: &mut Vec<Value>,
    pause_bridge: &Arc<InteractivePauseBridge>,
    perm_seq: &mut u64,
    pending_perm_requests: &mut BTreeMap<String, String>,
    pending_cmd_pauses: &mut BTreeMap<String, PendingCommandPause>,
) -> Result<(), RuntimeError> {
    let reply_id = id.clone().unwrap_or(Value::Null);

    match method {
        "initialize" => {
            if id.is_some() {
                write_jsonrpc_ok(
                    writer,
                    reply_id,
                    json!({
                        "protocolVersion": 1,
                        "agentInfo": {
                            "name": "gestalt",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "agentCapabilities": {
                            "loadSession": true,
                            "promptCapabilities": {
                                "image": false,
                                "audio": false,
                                "embeddedContext": true,
                            }
                        }
                    }),
                )
                .await?;
            }
        }
        "session/new" | "session/load" => {
            if let Some(host) = host_slot.as_ref() {
                *available_modes = collect_modes(host.kernel().registry());
            }
            if id.is_some() {
                write_jsonrpc_ok(
                    writer,
                    reply_id,
                    json!({
                        "sessionId": session_id,
                        "modes": {
                            "currentModeId": state.active_preset,
                            "availableModes": available_modes,
                        }
                    }),
                )
                .await?;
            }
        }
        "session/set_mode" => {
            let Some(mode_id) = params.get("modeId").and_then(Value::as_str) else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, -32602, "missing 'modeId'").await?;
                }
                return Ok(());
            };
            if let Some(handle) = active_turn.as_ref() {
                let _ = handle.control_tx.send(LiveTurnControl::SwitchMode {
                    preset_or_loop: mode_id.to_string(),
                    handoff: Value::Null,
                    config_override: None,
                });
                state.active_preset = mode_id.to_string();
            } else if let Some(host) = host_slot.as_ref() {
                if !is_known_preset_or_loop(host.kernel().registry(), mode_id) {
                    if id.is_some() {
                        write_jsonrpc_err(
                            writer,
                            reply_id,
                            -32602,
                            &format!("unknown mode '{mode_id}'"),
                        )
                        .await?;
                    }
                    return Ok(());
                }
                state.active_preset = mode_id.to_string();
            }
            write_json_line(
                writer,
                &json!({
                    "jsonrpc": "2.0",
                    "method": "session/update",
                    "params": {
                        "sessionId": session_id,
                        "update": {
                            "sessionUpdate": "current_mode_update",
                            "currentModeId": state.active_preset,
                        }
                    }
                }),
            )
            .await?;
            if id.is_some() {
                write_jsonrpc_ok(writer, reply_id, json!({})).await?;
            }
        }
        "session/prompt" => {
            let prompt_text = extract_acp_prompt_text(&params);
            if let Some(cmd) = parse_slash_command(&prompt_text) {
                if let Some(handle) = active_turn.as_ref() {
                    match cmd {
                        SlashCommand::Steer { urgent, message } => {
                            let kind = if urgent {
                                SteeringKind::UrgentSteer
                            } else {
                                SteeringKind::Steer
                            };
                            let _ = handle.control_tx.send(LiveTurnControl::Steering(
                                LiveSteeringMessage {
                                    kind,
                                    content: Value::String(message),
                                    source: SteeringSource::User,
                                },
                            ));
                            if id.is_some() {
                                write_jsonrpc_ok(
                                    writer,
                                    reply_id,
                                    json!({ "stopReason": "end_turn" }),
                                )
                                .await?;
                            }
                        }
                        SlashCommand::Stop => {
                            pause_bridge.deny_all("stopped via /stop");
                            handle.cancel.cancel();
                            if id.is_some() {
                                write_jsonrpc_ok(
                                    writer,
                                    reply_id,
                                    json!({ "stopReason": "cancelled" }),
                                )
                                .await?;
                            }
                        }
                        SlashCommand::Mode { preset } => {
                            let _ = handle.control_tx.send(LiveTurnControl::SwitchMode {
                                preset_or_loop: preset.clone(),
                                handoff: Value::Null,
                                config_override: None,
                            });
                            state.active_preset = preset;
                            if id.is_some() {
                                write_jsonrpc_ok(
                                    writer,
                                    reply_id,
                                    json!({ "stopReason": "end_turn" }),
                                )
                                .await?;
                            }
                        }
                        _ => {
                            if id.is_some() {
                                write_jsonrpc_err(
                                    writer,
                                    reply_id,
                                    -32003,
                                    "command requires idle session",
                                )
                                .await?;
                            }
                        }
                    }
                    return Ok(());
                }

                if let Some(host) = host_slot.as_mut() {
                    match execute_slash_command_idle(host, state, cmd).await {
                        Ok(res) => {
                            if let Some(pending) = res.pending_pause {
                                *perm_seq += 1;
                                let req_id = format!("acp_perm_{}", *perm_seq);
                                pending_perm_requests
                                    .insert(req_id.clone(), pending.pause_id.clone());
                                let pid = pending.pause_id.clone();
                                let reason = pending.reason;
                                let payload = pending.payload.clone();
                                pending_cmd_pauses.insert(pid.clone(), pending);
                                write_json_line(
                                    writer,
                                    &json!({
                                        "jsonrpc": "2.0",
                                        "id": req_id,
                                        "method": "session/request_permission",
                                        "params": {
                                            "sessionId": session_id,
                                            "toolCall": {
                                                "toolCallId": pid,
                                                "title": payload.get("prompt").and_then(Value::as_str).unwrap_or("Input required"),
                                                "kind": "other",
                                            },
                                            "options": [
                                                { "optionId": "allow_once", "name": "Submit", "kind": "allow_once" },
                                                { "optionId": "reject_once", "name": "Cancel", "kind": "reject_once" }
                                            ],
                                            "_meta": {
                                                "pause_id": pid,
                                                "reason": reason,
                                                "payload": payload,
                                            }
                                        }
                                    }),
                                )
                                .await?;
                            }
                            let mut msg = res.message;
                            if let Some(notice) = res.notice {
                                msg.push('\n');
                                msg.push_str(&notice);
                            }
                            write_json_line(
                                writer,
                                &json!({
                                    "jsonrpc": "2.0",
                                    "method": "session/update",
                                    "params": {
                                        "sessionId": session_id,
                                        "update": {
                                            "sessionUpdate": "agent_message_chunk",
                                            "content": { "type": "text", "text": msg },
                                        }
                                    }
                                }),
                            )
                            .await?;
                            if params.get("prompt").is_some() && !state.active_preset.is_empty() {
                                *available_modes = collect_modes(host.kernel().registry());
                            }
                            if id.is_some() {
                                write_jsonrpc_ok(
                                    writer,
                                    reply_id,
                                    json!({ "stopReason": "end_turn" }),
                                )
                                .await?;
                            }
                        }
                        Err(err) => {
                            if id.is_some() {
                                write_jsonrpc_err(writer, reply_id, -32602, &err.to_string())
                                    .await?;
                            }
                        }
                    }
                    return Ok(());
                }
            }

            if let Some(host) = host_slot.take() {
                let (control_tx, cancel, done_rx) = spawn_surface_turn(
                    host,
                    Value::String(prompt_text),
                    state.active_preset.clone(),
                    state.extra_decorators.clone(),
                );
                *active_turn = Some(ActiveAcpTurn {
                    prompt_req_id: reply_id,
                    control_tx,
                    cancel,
                    done_rx,
                });
            } else if id.is_some() {
                write_jsonrpc_err(writer, reply_id, -32003, "a prompt Turn is already active")
                    .await?;
            }
        }
        "session/cancel" => {
            if let Some(handle) = active_turn.as_ref() {
                pause_bridge.deny_all("session/cancel");
                let _ = handle
                    .control_tx
                    .send(LiveTurnControl::Steering(LiveSteeringMessage {
                        kind: SteeringKind::Abort,
                        content: Value::String("cancelled via acp session/cancel".to_string()),
                        source: SteeringSource::User,
                    }));
                handle.cancel.cancel();
            }
        }
        "_gestalt/steer" => {
            let content = params.get("content").cloned().unwrap_or(Value::Null);
            let urgent = params
                .get("urgent")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let kind = if urgent {
                SteeringKind::UrgentSteer
            } else {
                SteeringKind::Steer
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
                    write_jsonrpc_ok(writer, reply_id, json!({ "queued": true })).await?;
                }
            } else if let Some(host) = host_slot.as_mut() {
                let sig = host.kernel_mut().enqueue_steering(
                    kind,
                    content,
                    SteeringSource::User,
                    current_epoch_ms(),
                )?;
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
        }
        "_gestalt/fork" | "_gestalt/rewind" | "_gestalt/reload" => {
            let Some(host) = host_slot.as_mut() else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, -32003, "session busy with active Turn")
                        .await?;
                }
                return Ok(());
            };
            let cmd = match method {
                "_gestalt/fork" => SlashCommand::Fork {
                    node_id: params
                        .get("from_node_id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    new_file: params
                        .get("new_file")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
                "_gestalt/rewind" => SlashCommand::Rewind {
                    node_id: params
                        .get("target_node_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                },
                _ => SlashCommand::Reload {
                    ext_name: params
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                },
            };
            match execute_slash_command_idle(host, state, cmd).await {
                Ok(res) => {
                    if id.is_some() {
                        write_jsonrpc_ok(writer, reply_id, res.data).await?;
                    }
                }
                Err(err) => {
                    if id.is_some() {
                        write_jsonrpc_err(writer, reply_id, -32602, &err.to_string()).await?;
                    }
                }
            }
        }
        "_gestalt/trigger/list"
        | "_gestalt/trigger/add"
        | "_gestalt/trigger/remove"
        | "_gestalt/trigger/pause"
        | "_gestalt/trigger/resume"
        | "_gestalt/trigger/fire" => {
            let Some(host) = host_slot.as_ref() else {
                if id.is_some() {
                    write_jsonrpc_err(writer, reply_id, -32003, "session busy with active Turn")
                        .await?;
                }
                return Ok(());
            };
            let op = method.strip_prefix("_gestalt/trigger/").unwrap_or(method);
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
                    -32601,
                    &format!("method not found: {method}"),
                )
                .await?;
            }
        }
    }

    Ok(())
}

fn run_event_to_acp_update(event: &RunEvent) -> Option<Value> {
    match event {
        RunEvent::StreamDelta {
            channel,
            effect_id,
            payload,
            ..
        } => match channel.as_str() {
            "text" => {
                let text = extract_stream_text(payload)?;
                Some(json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": text },
                }))
            }
            "thought" => {
                let text = extract_stream_text(payload)?;
                Some(json!({
                    "sessionUpdate": "agent_thought_chunk",
                    "content": { "type": "text", "text": text },
                }))
            }
            "tool_output" => Some(json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": effect_id,
                "status": "in_progress",
                "rawOutput": payload,
            })),
            _ => None,
        },
        RunEvent::ToolUi {
            effect_id,
            renderer,
            data,
            ..
        } => Some(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": effect_id,
            "status": "completed",
            "_meta": {
                "gestalt_tool_ui": {
                    "renderer": renderer,
                    "data": data,
                }
            }
        })),
        RunEvent::PhaseSwitched {
            preset_id,
            base_loop_id,
            ..
        } => Some(json!({
            "sessionUpdate": "current_mode_update",
            "currentModeId": preset_id.as_deref().unwrap_or(base_loop_id.as_str()),
        })),
        RunEvent::PolicyDenied { effect_id, reason } => Some(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": effect_id,
            "status": "failed",
            "rawOutput": { "denied": true, "reason": reason },
        })),
        RunEvent::TriggerFired {
            trigger_id,
            kind,
            turn_or_signal_id,
            queued_as_follow_up,
            payload,
        } => Some(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {
                "type": "text",
                "text": format!("[trigger:{trigger_id} ({kind})] {}", if *queued_as_follow_up { "queued as follow-up" } else { "started turn" }),
            },
            "_meta": {
                "gestalt_trigger_fired": {
                    "trigger_id": trigger_id,
                    "kind": kind,
                    "turn_or_signal_id": turn_or_signal_id,
                    "queued_as_follow_up": queued_as_follow_up,
                    "payload": payload,
                }
            }
        })),
        RunEvent::NodeAppended { .. }
        | RunEvent::Paused { .. }
        | RunEvent::TurnCompleted { .. }
        | RunEvent::TurnAborted { .. } => None,
    }
}

fn resolve_permission_response(bridge: &InteractivePauseBridge, pause_id: &str, response: &Value) {
    if response.get("error").is_some() {
        bridge.deny(pause_id, "acp client returned permission error");
        return;
    }
    let Some(result) = response.get("result") else {
        bridge.deny(pause_id, "malformed acp permission response");
        return;
    };
    let outcome = result.get("outcome").unwrap_or(result);
    match outcome.get("outcome").and_then(Value::as_str) {
        Some("selected") => {
            let option_id = outcome
                .get("optionId")
                .and_then(Value::as_str)
                .unwrap_or("");
            if option_id.starts_with("allow") {
                let mut res = json!({
                    "approved": true,
                    "option_id": option_id,
                });
                if let Some(secret) = result.get("secret").or_else(|| outcome.get("secret")) {
                    res["secret"] = secret.clone();
                }
                bridge.resolve(pause_id, res);
            } else {
                bridge.resolve(
                    pause_id,
                    json!({
                        "approved": false,
                        "option_id": option_id,
                    }),
                );
            }
        }
        Some("cancelled") => {
            bridge.deny(pause_id, "permission cancelled by client");
        }
        _ => {
            bridge.deny(
                pause_id,
                "unrecognized acp permission outcome (fail-closed)",
            );
        }
    }
}

fn collect_modes(registry: &ExtensionRegistry) -> Vec<Value> {
    if !registry.presets.is_empty() {
        return registry
            .presets
            .keys()
            .map(|id| json!({ "id": id, "name": id }))
            .collect();
    }
    registry
        .loops
        .keys()
        .map(|id| json!({ "id": id, "name": id }))
        .collect()
}

fn extract_acp_prompt_text(params: &Value) -> String {
    if let Some(arr) = params.get("prompt").and_then(Value::as_array) {
        let parts: Vec<&str> = arr
            .iter()
            .filter_map(|blk| blk.get("text").and_then(Value::as_str))
            .collect();
        return parts.join("\n");
    }
    params
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, BudgetCap, ContextBuilder, Effect, LoopAction, LoopConfig,
        LoopContext, ModelCapabilities, ModelKind, PolicyGate, SandboxPolicy, SessionSlice,
        StepResult,
    };
    use gestalt_runtime::sandbox::UnsandboxedFallbackDriver;
    use gestalt_runtime::{
        CredentialStore, HeadlessPauseHandler, ModelProvider, ModelRequest, ModelResponse,
        ProviderContext, RuntimeRegistry, StreamDeltaFn,
    };
    use std::fs;
    use std::future::Future;
    use std::pin::Pin;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    struct AcpMockProvider;
    impl ModelProvider for AcpMockProvider {
        fn id(&self) -> &str {
            "mock"
        }
        fn capabilities(&self, _model: &str) -> ModelCapabilities {
            ModelCapabilities::default()
        }
        fn invoke<'a>(
            &'a self,
            _req: &'a ModelRequest,
            _ctx: &'a ProviderContext<'a>,
            on_delta: StreamDeltaFn<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>>
        {
            Box::pin(async move {
                on_delta("thought", json!({ "text": "planning" }));
                on_delta("text", json!({ "text": "done" }));
                Ok(ModelResponse {
                    kind: ModelKind::Llm,
                    output: json!({ "text": "done" }),
                    usage: Default::default(),
                })
            })
        }
    }

    struct AcpLoop {
        step: usize,
    }
    impl AgentLoop for AcpLoop {
        fn id(&self) -> &str {
            "acp_loop"
        }
        fn digest(&self) -> &str {
            "a1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            _results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            match self.step {
                0 => {
                    self.step += 1;
                    LoopAction::Yield(vec![Effect::Pause {
                        reason: PauseReason::Approval,
                        payload: json!({ "title": "Allow edit" }),
                    }])
                }
                1 => {
                    self.step += 1;
                    LoopAction::Yield(vec![Effect::Model {
                        target: "mock/m1".into(),
                        kind: ModelKind::Llm,
                        input: json!({}),
                        schema: None,
                    }])
                }
                _ => LoopAction::Done(json!("ok")),
            }
        }
    }
    struct AcpFactory;
    impl AgentLoopFactory for AcpFactory {
        fn id(&self) -> &str {
            "acp_loop"
        }
        fn digest(&self) -> &str {
            "a1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(AcpLoop { step: 0 })
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
    async fn acp_surface_supports_init_prompt_thought_chunks_and_request_permission() {
        let dir = std::env::temp_dir().join(format!("gestalt_acp_surf_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");

        let mut reg = ExtensionRegistry::new();
        reg.loops
            .insert("acp_loop".into(), std::sync::Arc::new(AcpFactory));
        reg.builders
            .insert("default".into(), std::sync::Arc::new(NullBuilder));
        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let mut rt = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
        rt.providers.register_provider(Arc::new(AcpMockProvider));
        let host = RuntimeHost::new(
            "r_acp".into(),
            &dir,
            dir.join(".gestalt/sessions/acp.jsonl"),
            reg,
            gate,
            rt,
            CredentialStore::default(),
            None,
            Arc::new(HeadlessPauseHandler::default()),
        )
        .expect("host");

        let (cin, sin) = tokio::io::duplex(8192);
        let (sout, cout) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            run_acp_surface(host, "acp_loop".into(), BufReader::new(sin), sout).await
        });

        let mut cin = cin;
        let mut cout = BufReader::new(cout).lines();

        // 1. initialize
        cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n")
            .await
            .unwrap();
        let init: Value = serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(init["result"]["protocolVersion"], 1);

        // 2. session/new
        cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/new\",\"params\":{}}\n")
            .await
            .unwrap();
        let new_sess: Value =
            serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(new_sess["result"]["modes"]["currentModeId"], "acp_loop");

        // 3. session/prompt -> expect session/request_permission -> reply allow_once -> expect thought + message + end_turn
        cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/prompt\",\"params\":{\"prompt\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n")
            .await
            .unwrap();

        let perm_req_id = loop {
            let line = tokio::time::timeout(Duration::from_secs(2), cout.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["method"] == "session/request_permission" {
                break v["id"].as_str().unwrap().to_string();
            }
        };

        let perm_reply = json!({
            "jsonrpc": "2.0",
            "id": perm_req_id,
            "result": { "outcome": { "outcome": "selected", "optionId": "allow_once" } }
        });
        cin.write_all(format!("{perm_reply}\n").as_bytes())
            .await
            .unwrap();

        let mut saw_thought = false;
        let mut saw_text = false;
        loop {
            let line = tokio::time::timeout(Duration::from_secs(2), cout.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["method"] == "session/update" {
                let kind = v["params"]["update"]["sessionUpdate"].as_str();
                if kind == Some("agent_thought_chunk") {
                    saw_thought = true;
                } else if kind == Some("agent_message_chunk") {
                    saw_text = true;
                }
            } else if v["id"] == 3 {
                assert_eq!(v["result"]["stopReason"], "end_turn");
                break;
            }
        }
        assert!(saw_thought);
        assert!(saw_text);

        drop(cin);
        let _ = server.await.unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
