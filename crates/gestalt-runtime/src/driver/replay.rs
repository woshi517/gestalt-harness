#![forbid(unsafe_code)]
//! Offline replay driver supporting `--reexecute-ext` inside network-disabled
//! OS sandboxes.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use gestalt_kernel::{
    DivergenceKind, ExtensionRegistry, LoopAction, NodePayload, PolicyGate, ReplayDivergence,
    ReplayEngine, ReplayMode, ReplayReport, SessionNode, SessionTree,
};
use serde_json::Value;

use crate::blobs::BlobStore;
use crate::config::{ExtensionProtocol, ExternalExtensionConfig};
use crate::credentials::StreamRedactor;
use crate::ext_ipc::{ExtIpcWorker, ExternalLoopStepRequest};
use crate::sandbox::SandboxDriver;
use crate::{CancellationToken, RuntimeError};

/// Options required when re-executing out-of-process extensions in
/// [`replay_session_tree`] (`--reexecute-ext`).
pub struct ExternalReplayOptions<'a> {
    /// Configured external RPC extensions keyed by name.
    pub ext_configs: &'a BTreeMap<String, ExternalExtensionConfig>,
    /// Canonical workspace root path.
    pub workspace_root: &'a Path,
    /// OS sandbox driver used to run external workers network-disabled.
    pub sandbox_driver: &'a Arc<dyn SandboxDriver>,
}

/// Options for [`replay_session_tree_with_overlay`].
pub struct ReplayOverlayOptions<'a> {
    /// Target `ExtensionRegistry` to verify against.
    pub registry: &'a ExtensionRegistry,
    /// `PolicyGate` invariants for replay.
    pub policy_gate: &'a PolicyGate,
    /// Content-addressed blob store for artifact snapshots.
    pub blob_store: &'a BlobStore,
    /// Replay execution mode (`Default` or `ReexecuteExternal`).
    pub mode: ReplayMode,
    /// External extension options when `mode == ReplayMode::ReexecuteExternal`.
    pub ext_opts: Option<ExternalReplayOptions<'a>>,
    /// Optional staged [`gestalt_kernel::ArtifactStoreView`] overlay.
    pub overlay: Option<&'a gestalt_kernel::ArtifactStoreView>,
}

/// Replay a recorded [`SessionTree`] offline, optionally re-executing out-of-process
/// extension workers inside a network-disabled OS sandbox (`--reexecute-ext`).
///
/// # Arguments
///
/// * `tree` - Validated `SessionTree` to replay
/// * `registry` - Extension registry providing loop, builder, and layer implementations
/// * `policy_gate` - Policy gate invariants for replay verification
/// * `blob_store` - Content-addressed blob store for artifact snapshots
/// * `mode` - Replay mode (`Default` or `ReexecuteExternal`)
/// * `ext_opts` - Optional external worker config for `ReexecuteExternal`
///
/// # Returns
///
/// Structured [`ReplayReport`] summarizing replayed turns, phases, and any divergences.
///
/// # Errors
///
/// Returns [`RuntimeError`] if structural decode, sandbox spawn, or IPC fails.
pub async fn replay_session_tree(
    tree: &SessionTree,
    registry: &ExtensionRegistry,
    policy_gate: &PolicyGate,
    blob_store: &BlobStore,
    mode: ReplayMode,
    ext_opts: Option<ExternalReplayOptions<'_>>,
) -> Result<ReplayReport, RuntimeError> {
    replay_session_tree_with_overlay(
        tree,
        ReplayOverlayOptions {
            registry,
            policy_gate,
            blob_store,
            mode,
            ext_opts,
            overlay: None,
        },
    )
    .await
}

/// Replay a recorded [`SessionTree`] offline with an optional staged
/// [`gestalt_kernel::ArtifactStoreView`] overlay (used by `replay_verify`).
///
/// # Arguments
///
/// * `tree` - Validated `SessionTree` to replay
/// * `opts` - Bundled [`ReplayOverlayOptions`] including registry, gate, blob store, and overlay
///
/// # Returns
///
/// Structured [`ReplayReport`] summarizing replayed turns, phases, and any divergences.
///
/// # Errors
///
/// Returns [`RuntimeError`] if structural decode, sandbox spawn, or IPC fails.
pub async fn replay_session_tree_with_overlay(
    tree: &SessionTree,
    opts: ReplayOverlayOptions<'_>,
) -> Result<ReplayReport, RuntimeError> {
    let ReplayOverlayOptions {
        registry,
        policy_gate,
        blob_store,
        mode,
        ext_opts,
        overlay,
    } = opts;
    let mut overlay_by_digest: BTreeMap<String, String> = BTreeMap::new();
    if let Some(ov) = overlay {
        for node in tree.all_nodes() {
            if let Ok(NodePayload::TurnStart {
                artifact_snapshots, ..
            }) = node.decode_payload()
            {
                for (path, recorded_digest) in artifact_snapshots {
                    if let Some(entry) = ov.get(&path) {
                        overlay_by_digest.insert(recorded_digest, entry.content.clone());
                    }
                }
            }
        }
    }

    let mut report =
        ReplayEngine::new(registry, policy_gate).replay(tree, ReplayMode::Default, |digest| {
            if let Some(ov_text) = overlay_by_digest.get(digest) {
                return Some(ov_text.clone());
            }
            blob_store.get_utf8(digest).ok()
        })?;

    if mode != ReplayMode::ReexecuteExternal {
        return Ok(report);
    }

    let Some(opts) = ext_opts else {
        return Err(RuntimeError::IpcProtocol {
            ext_name: "replay".to_string(),
            message: "ExternalReplayOptions required for ReplayMode::ReexecuteExternal".to_string(),
        });
    };

    let mut offline_sandbox = policy_gate.sandbox().clone();
    offline_sandbox.network = false;

    let mut workers: BTreeMap<String, ExtIpcWorker> = BTreeMap::new();
    for (ext_name, cfg) in opts.ext_configs {
        if cfg.protocol == ExtensionProtocol::Rpc {
            let mut w = ExtIpcWorker::spawn(
                ext_name.clone(),
                cfg,
                opts.workspace_root,
                &offline_sandbox,
                opts.sandbox_driver,
                StreamRedactor::default(),
            )?;
            let _ = w.describe().await?;
            workers.insert(ext_name.clone(), w);
        }
    }

    let nodes = tree.all_nodes();
    let mut active_ext_loop: Option<(String, gestalt_kernel::LoopConfig)> = None;
    let mut active_handoff = Value::Null;
    let mut idx = 0;

    while idx < nodes.len() {
        let node = &nodes[idx];
        match node.decode_payload()? {
            NodePayload::PhaseOpen {
                base_loop_id,
                config,
                ..
            } => {
                if let Some(binding) = registry.external_loops.get(&base_loop_id) {
                    active_ext_loop = Some((binding.ext_name.clone(), config.clone()));
                    let is_switch = nodes.get(idx + 1).is_some_and(|n| {
                        n.kind == "step_result"
                            && n.payload.get("status").and_then(Value::as_str) == Some("switched")
                    });
                    if !is_switch && let Some(worker) = workers.get_mut(&binding.ext_name) {
                        let req = ExternalLoopStepRequest {
                            phase_id: node.phase_id.clone().unwrap_or_default(),
                            turn_id: node.turn_id.clone().unwrap_or_default(),
                            handoff: active_handoff.clone(),
                            config,
                            state_token: None,
                            results: None,
                            session_slice: nodes[..=idx].to_vec(),
                            artifacts: BTreeMap::new(),
                            budget_remaining: gestalt_kernel::BudgetSnapshot::default(),
                            steering: Vec::new(),
                            now_ms: node.timestamp_ms,
                            context: Value::Null,
                        };
                        let out = worker.loop_step(&req, CancellationToken::new()).await?;
                        verify_ext_action_against_trace(nodes, idx + 1, &out.action, &mut report);
                    }
                } else {
                    active_ext_loop = None;
                }
            }
            NodePayload::PhaseSeal { handoff, .. } => {
                active_handoff = handoff;
                active_ext_loop = None;
            }
            NodePayload::StepResult(sr) => {
                if let Some((ref ext_name, ref config)) = active_ext_loop {
                    let mut batch = vec![sr];
                    while idx + 1 < nodes.len()
                        && nodes[idx + 1].kind == "step_result"
                        && let NodePayload::StepResult(next_sr) = nodes[idx + 1].decode_payload()?
                    {
                        idx += 1;
                        batch.push(next_sr);
                    }
                    let state_token = nodes[..idx]
                        .iter()
                        .rev()
                        .find(|n| n.kind == "effect")
                        .and_then(|n| n.payload.get("state_token").cloned());

                    if let Some(worker) = workers.get_mut(ext_name) {
                        let req = ExternalLoopStepRequest {
                            phase_id: nodes[idx].phase_id.clone().unwrap_or_default(),
                            turn_id: nodes[idx].turn_id.clone().unwrap_or_default(),
                            handoff: active_handoff.clone(),
                            config: config.clone(),
                            state_token,
                            results: Some(batch),
                            session_slice: nodes[..=idx].to_vec(),
                            artifacts: BTreeMap::new(),
                            budget_remaining: gestalt_kernel::BudgetSnapshot::default(),
                            steering: Vec::new(),
                            now_ms: nodes[idx].timestamp_ms,
                            context: Value::Null,
                        };
                        let out = worker.loop_step(&req, CancellationToken::new()).await?;
                        verify_ext_action_against_trace(nodes, idx + 1, &out.action, &mut report);
                    }
                }
            }
            _ => {}
        }
        idx += 1;
    }

    for (_, mut w) in workers {
        let _ = w.shutdown().await;
    }

    Ok(report)
}

fn verify_ext_action_against_trace(
    nodes: &[SessionNode],
    start_idx: usize,
    action: &LoopAction,
    report: &mut ReplayReport,
) {
    let mut recorded_effects = Vec::new();
    let mut cursor = start_idx;
    while cursor < nodes.len() {
        let n = &nodes[cursor];
        if n.kind == "effect" {
            if let Ok(NodePayload::Effect { envelope, .. }) = n.decode_payload() {
                recorded_effects.push((
                    n.id.clone(),
                    n.turn_id.clone(),
                    n.phase_id.clone(),
                    envelope,
                ));
            }
            cursor += 1;
        } else if n.kind == "steering" || n.kind == "pause" || n.kind == "pause_resolved" {
            cursor += 1;
        } else {
            break;
        }
    }

    match action {
        LoopAction::Yield(replayed) => {
            if replayed.len() != recorded_effects.len() {
                let node_id = recorded_effects
                    .first()
                    .map_or_else(|| "eof".to_string(), |(id, _, _, _)| id.clone());
                report.divergences.push(ReplayDivergence {
                    node_id,
                    turn_id: None,
                    phase_id: None,
                    kind: DivergenceKind::EffectSequenceMismatch,
                    expected: Value::from(recorded_effects.len()),
                    actual: Value::from(replayed.len()),
                    message: "external loop yielded different effect batch length".to_string(),
                });
                return;
            }
            for (rep, (nid, tid, pid, rec)) in replayed.iter().zip(recorded_effects.iter()) {
                if rep != &rec.effect {
                    report.divergences.push(ReplayDivergence {
                        node_id: nid.clone(),
                        turn_id: tid.clone(),
                        phase_id: pid.clone(),
                        kind: DivergenceKind::EffectPayloadMismatch,
                        expected: serde_json::to_value(&rec.effect).unwrap_or(Value::Null),
                        actual: serde_json::to_value(rep).unwrap_or(Value::Null),
                        message: "external loop effect payload diverged during --reexecute-ext"
                            .to_string(),
                    });
                }
            }
        }
        LoopAction::Done(_) => {
            if !recorded_effects.is_empty() {
                let (nid, tid, pid, rec) = &recorded_effects[0];
                report.divergences.push(ReplayDivergence {
                    node_id: nid.clone(),
                    turn_id: tid.clone(),
                    phase_id: pid.clone(),
                    kind: DivergenceKind::UnexpectedDone,
                    expected: serde_json::to_value(&rec.effect).unwrap_or(Value::Null),
                    actual: Value::String("done".to_string()),
                    message: "external loop returned Done where trace recorded an Effect"
                        .to_string(),
                });
            }
        }
    }
}
