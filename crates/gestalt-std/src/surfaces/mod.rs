#![forbid(unsafe_code)]
//! User-facing and protocol-facing surfaces (`print`, `replay`, `rpc`, `acp`, `tui`).
//!
//! All surfaces consume [`gestalt_runtime::RuntimeHost`], [`gestalt_kernel::RunEvent`],
//! [`gestalt_runtime::PauseHandler`], and [`gestalt_kernel::UISurface`] generically,
//! remaining independent of `gestalt-std`'s `loops`, `tools`, and `providers` features.

pub mod acp;
pub mod print;
pub mod replay;
pub mod rpc;
#[cfg(feature = "tui")]
pub mod tui;

use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use gestalt_kernel::{
    ExtensionRegistry, KernelError, PauseReason, RunEvent, SteeringKind, SteeringSource,
};
use gestalt_runtime::{
    CancellationToken, CredentialRecord, CredentialStore, EventSink, LiveTurnControl, PauseHandler,
    RuntimeError, RuntimeHost, TriggerInstanceConfig, TurnOutcome, current_epoch_ms,
};
use serde_json::{Value, json};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

pub use acp::{run_acp_stdio, run_acp_surface};
pub use print::{PrintOptions, PrintResult, run_print_stdout, run_print_surface};
pub use replay::{
    ReplaySurfaceOptions, ReplaySurfaceResult, run_replay_stdout, run_replay_surface,
};
pub use rpc::{
    RPC_INTERNAL_ERROR, RPC_INVALID_PARAMS, RPC_INVALID_REQUEST, RPC_INVALID_STATE,
    RPC_METHOD_NOT_FOUND, RPC_PARSE_ERROR, RPC_POLICY_DENIED, RPC_SESSION_LOCKED, run_rpc_stdio,
    run_rpc_surface,
};

/// Canonical CLI / surface exit codes (`docs/TECHNICAL_ROADMAP.md` Section 3.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SurfaceExitCode {
    /// Success or 100% deterministic replay parity.
    Success = 0,
    /// Runtime/provider error or replay divergence.
    RuntimeError = 1,
    /// CLI/config parse error or corrupt replay JSONL.
    ConfigOrParseError = 2,
    /// Policy denial, untrusted workspace, or headless pause denial.
    PolicyOrPauseDenied = 3,
    /// SIGINT / aborted Turn.
    Aborted = 130,
}

impl SurfaceExitCode {
    /// Convert to process exit integer code.
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Map a [`RuntimeError`] to the canonical [`SurfaceExitCode`].
    #[must_use]
    pub fn from_runtime_error(err: &RuntimeError) -> Self {
        match err {
            RuntimeError::UntrustedWorkspace { .. }
            | RuntimeError::HeadlessPauseDenied(_)
            | RuntimeError::SandboxViolation(_)
            | RuntimeError::Kernel(KernelError::PolicyRelaxationForbidden(_)) => {
                Self::PolicyOrPauseDenied
            }
            RuntimeError::TomlDe(_) | RuntimeError::Json(_) => Self::ConfigOrParseError,
            _ => Self::RuntimeError,
        }
    }
}

/// [`EventSink`] forwarding redacted [`RunEvent`]s onto a Tokio unbounded channel.
#[derive(Debug, Clone)]
pub struct ChannelEventSink {
    tx: mpsc::UnboundedSender<RunEvent>,
}

impl ChannelEventSink {
    /// Create a [`ChannelEventSink`] backed by `tx`.
    #[must_use]
    pub fn new(tx: mpsc::UnboundedSender<RunEvent>) -> Self {
        Self { tx }
    }
}

impl EventSink for ChannelEventSink {
    fn on_event(&self, event: &RunEvent) {
        let _ = self.tx.send(event.clone());
    }
}

/// Pending `Effect::Pause` prompt emitted by [`InteractivePauseBridge`].
#[derive(Debug, Clone, PartialEq)]
pub struct PausePrompt {
    /// Unique pause identifier.
    pub pause_id: String,
    /// Correlated `effect_id`.
    pub effect_id: String,
    /// Semantic pause reason (`Approval`, `Input`, `Checkpoint`).
    pub reason: PauseReason,
    /// Dialog payload JSON.
    pub payload: Value,
}

type PendingPauseMap = Arc<Mutex<BTreeMap<String, oneshot::Sender<Result<Value, RuntimeError>>>>>;

/// Non-blocking [`PauseHandler`] bridge shared by interactive surfaces (`rpc`, `acp`, `tui`).
#[derive(Debug, Clone)]
pub struct InteractivePauseBridge {
    prompt_tx: mpsc::UnboundedSender<PausePrompt>,
    pending: PendingPauseMap,
}

impl InteractivePauseBridge {
    /// Create a new [`InteractivePauseBridge`] emitting prompts onto `prompt_tx`.
    #[must_use]
    pub fn new(prompt_tx: mpsc::UnboundedSender<PausePrompt>) -> Self {
        Self {
            prompt_tx,
            pending: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Resolve a pending `pause_id` with `resolution`. Returns `false` if unknown.
    pub fn resolve(&self, pause_id: &str, resolution: Value) -> bool {
        let tx = self
            .pending
            .lock()
            .ok()
            .and_then(|mut map| map.remove(pause_id));
        match tx {
            Some(sender) => sender.send(Ok(resolution)).is_ok(),
            None => false,
        }
    }

    /// Deny/cancel a pending `pause_id` fail-closed. Returns `false` if unknown.
    pub fn deny(&self, pause_id: &str, reason: impl Into<String>) -> bool {
        let tx = self
            .pending
            .lock()
            .ok()
            .and_then(|mut map| map.remove(pause_id));
        match tx {
            Some(sender) => sender
                .send(Err(RuntimeError::HeadlessPauseDenied(reason.into())))
                .is_ok(),
            None => false,
        }
    }

    /// Fail closed all currently pending pauses (e.g., on `session/cancel` or EOF).
    pub fn deny_all(&self, reason: &str) {
        let waiters = match self.pending.lock() {
            Ok(mut map) => std::mem::take(&mut *map),
            Err(_) => return,
        };
        for (_, sender) in waiters {
            let _ = sender.send(Err(RuntimeError::HeadlessPauseDenied(reason.to_string())));
        }
    }
}

impl PauseHandler for InteractivePauseBridge {
    fn resolve_pause<'a>(
        &'a self,
        pause_id: &'a str,
        effect_id: &'a str,
        reason: PauseReason,
        payload: &'a Value,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut map) = self.pending.lock() {
            map.insert(pause_id.to_string(), tx);
        }
        let _ = self.prompt_tx.send(PausePrompt {
            pause_id: pause_id.to_string(),
            effect_id: effect_id.to_string(),
            reason,
            payload: payload.clone(),
        });

        let pending = self.pending.clone();
        let pid = pause_id.to_string();
        Box::pin(async move {
            tokio::select! {
                () = cancel.cancelled() => {
                    if let Ok(mut map) = pending.lock() {
                        map.remove(&pid);
                    }
                    Err(RuntimeError::HeadlessPauseDenied("pause cancelled".to_string()))
                }
                res = rx => {
                    match res {
                        Ok(inner) => inner,
                        Err(_) => Err(RuntimeError::HeadlessPauseDenied("pause channel closed".to_string())),
                    }
                }
            }
        })
    }
}

/// Built-in kernel/host slash command specs `(command, usage, description)`
/// shared between `/help` and TUI `/` autocomplete.
pub const KERNEL_COMMAND_SPECS: &[(&str, &str, &str)] = &[
    (
        "/mode",
        "/mode <preset>",
        "Switch active preset or loop mid-session",
    ),
    (
        "/steer",
        "/steer [--urgent] <msg>",
        "Redirect an active Turn between or mid-step",
    ),
    ("/stop", "/stop", "Abort the active Turn cleanly"),
    (
        "/fork",
        "/fork [<node_id>] [--new-file]",
        "Branch SessionTree from a node",
    ),
    (
        "/rewind",
        "/rewind <node_id>",
        "Rewind conversation state (disk files unchanged)",
    ),
    (
        "/tree",
        "/tree",
        "Browse SessionTree branches, rewind, or fork",
    ),
    (
        "/reload",
        "/reload [<ext>]",
        "Hot-reload external extension(s) and refresh skills",
    ),
    (
        "/rollback",
        "/rollback <stage_id>",
        "Revert a promoted Effect::Save stage",
    ),
    ("/new", "/new", "Start a fresh SessionTree"),
    (
        "/session",
        "/session",
        "Browse/switch sessions or inspect active session",
    ),
    ("/resume", "/resume", "Switch to an existing saved session"),
    (
        "/trust",
        "/trust",
        "Approve workspace .gestalt/gestalt.toml",
    ),
    ("/help", "/help", "Show available commands and hotkeys"),
    ("/quit", "/quit", "Exit the interactive session"),
];

/// Parsed kernel-backed or extension-registered slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    /// `/mode <preset>`
    Mode {
        /// Target preset or loop ID.
        preset: String,
    },
    /// `/steer [--urgent] <msg>`
    Steer {
        /// Whether `--urgent` preempts in-flight step I/O.
        urgent: bool,
        /// Steering instruction text.
        message: String,
    },
    /// `/stop`
    Stop,
    /// `/fork [<node_id>] [--new-file]`
    Fork {
        /// Optional node ID to fork from (defaults to active head).
        node_id: Option<String>,
        /// Whether to also write the forked branch to a new JSONL file.
        new_file: bool,
    },
    /// `/rewind <node_id>`
    Rewind {
        /// Historical node ID to rewind conversation state to.
        node_id: String,
    },
    /// `/reload [<ext>]`
    Reload {
        /// Optional external extension name to hot-reload (empty reloads all + skills).
        ext_name: String,
    },
    /// `/rollback <stage_id>`
    Rollback {
        /// Promoted `Effect::Save` stage ID to revert.
        stage_id: String,
    },
    /// `/new` — start a fresh SessionTree
    New,
    /// `/tree` — browse SessionTree DAG, rewind, or fork
    Tree,
    /// `/session` (alias `/resume`) — inspect or switch active session
    Session,
    /// `/trust` — approve workspace `.gestalt/gestalt.toml`
    Trust,
    /// `/help` (alias `/hotkeys`)
    Help,
    /// `/quit` (alias `/exit`)
    Quit,
    /// Extension-registered slash command (`/model`, `/thinking`, `/goal`, `/compact`, `/gate`, `/login`, `/logout`, `/skills`, `/skill:<name>`, or custom).
    Extension {
        /// Command name without leading `/`.
        name: String,
        /// Raw trailing argument string.
        args: String,
    },
}

/// Parse a leading `/` slash command line into [`SlashCommand`]. Returns `None`
/// if `input` does not begin with `/` or has an empty command name.
#[must_use]
pub fn parse_slash_command(input: &str) -> Option<SlashCommand> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('/')?;
    if rest.is_empty() {
        return None;
    }

    let (cmd, raw_args) = match rest.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (rest, ""),
    };
    if cmd.is_empty() {
        return None;
    }

    match cmd {
        "mode" => Some(SlashCommand::Mode {
            preset: raw_args.to_string(),
        }),
        "steer" => {
            if let Some(after) = raw_args.strip_prefix("--urgent")
                && (after.is_empty() || after.starts_with(char::is_whitespace))
            {
                return Some(SlashCommand::Steer {
                    urgent: true,
                    message: after.trim().to_string(),
                });
            }
            Some(SlashCommand::Steer {
                urgent: false,
                message: raw_args.to_string(),
            })
        }
        "stop" => Some(SlashCommand::Stop),
        "fork" => {
            let mut node_id = None;
            let mut new_file = false;
            for tok in raw_args.split_whitespace() {
                if tok == "--new-file" {
                    new_file = true;
                } else if node_id.is_none() {
                    node_id = Some(tok.to_string());
                }
            }
            Some(SlashCommand::Fork { node_id, new_file })
        }
        "rewind" => Some(SlashCommand::Rewind {
            node_id: raw_args.to_string(),
        }),
        "reload" => Some(SlashCommand::Reload {
            ext_name: raw_args.to_string(),
        }),
        "rollback" => Some(SlashCommand::Rollback {
            stage_id: raw_args.to_string(),
        }),
        "new" => Some(SlashCommand::New),
        "tree" => Some(SlashCommand::Tree),
        "session" | "resume" => Some(SlashCommand::Session),
        "trust" => Some(SlashCommand::Trust),
        "help" | "hotkeys" => Some(SlashCommand::Help),
        "quit" | "exit" => Some(SlashCommand::Quit),
        _ => {
            if let Some(skill_name) = cmd.strip_prefix("skill:") {
                let args = if raw_args.is_empty() {
                    format!("load:{skill_name}")
                } else {
                    format!("load:{skill_name} {raw_args}")
                };
                return Some(SlashCommand::Extension {
                    name: "skills".to_string(),
                    args,
                });
            }
            Some(SlashCommand::Extension {
                name: cmd.to_string(),
                args: raw_args.to_string(),
            })
        }
    }
}

/// Mutable surface-level session settings shared across Turns.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceSessionState {
    /// Active preset or base loop ID used for new Turns.
    pub active_preset: String,
    /// Runtime decorators appended on Turn start (e.g., `["goal_check"]` via `/goal`).
    pub extra_decorators: Vec<String>,
}

impl SurfaceSessionState {
    /// Create a [`SurfaceSessionState`] initialized with `active_preset`.
    #[must_use]
    pub fn new(active_preset: impl Into<String>) -> Self {
        Self {
            active_preset: active_preset.into(),
            extra_decorators: Vec::new(),
        }
    }
}

/// Non-blocking pause prompt returned by an idle slash command (e.g., `/login`).
#[derive(Debug, Clone, PartialEq)]
pub struct PendingCommandPause {
    /// Unique pause identifier.
    pub pause_id: String,
    /// Semantic pause reason (`Approval`, `Input`, `Checkpoint`).
    pub reason: PauseReason,
    /// Dialog payload JSON.
    pub payload: Value,
}

/// Outcome returned by [`execute_slash_command_idle`].
#[derive(Debug, Clone, PartialEq)]
pub struct CommandDispatchResult {
    /// Concise human-readable summary message.
    pub message: String,
    /// Structured JSON payload for RPC/ACP/TUI callers.
    pub data: Value,
    /// Optional operator notice (e.g., disk-unchanged warning on `/rewind`).
    pub notice: Option<String>,
    /// Optional non-blocking pause prompt to resolve in the surface's event loop.
    pub pending_pause: Option<PendingCommandPause>,
}

/// Check whether `preset_or_loop` is registered in `registry`.
#[must_use]
pub fn is_known_preset_or_loop(registry: &ExtensionRegistry, preset_or_loop: &str) -> bool {
    registry.presets.contains_key(preset_or_loop)
        || registry.loops.contains_key(preset_or_loop)
        || registry.external_loops.contains_key(preset_or_loop)
}

/// Execute a [`SlashCommand`] while [`RuntimeHost`] is idle (between Turns).
///
/// Extension commands that request interactive input (e.g. `/login`) return
/// `pending_pause` instead of blocking the surface event loop; callers resolve
/// it asynchronously and pass the resolution to [`complete_command_pause`].
///
/// # Errors
///
/// Returns [`RuntimeError`] on invalid arguments, missing extension slots, or I/O failure.
pub async fn execute_slash_command_idle(
    host: &mut RuntimeHost,
    state: &mut SurfaceSessionState,
    cmd: SlashCommand,
) -> Result<CommandDispatchResult, RuntimeError> {
    match cmd {
        SlashCommand::Mode { preset } => {
            if preset.is_empty() {
                return Err(RuntimeError::ArtifactError(
                    "usage: /mode <preset>".to_string(),
                ));
            }
            if !is_known_preset_or_loop(host.kernel().registry(), &preset) {
                return Err(RuntimeError::Kernel(KernelError::UnresolvedExtensionSlot {
                    slot: "preset_or_loop".to_string(),
                    name: preset,
                }));
            }
            state.active_preset = preset.clone();
            Ok(CommandDispatchResult {
                message: format!("Switched active preset to '{preset}'."),
                data: json!({ "mode": preset }),
                notice: None,
                pending_pause: None,
            })
        }
        SlashCommand::Steer { urgent, message } => {
            if message.is_empty() {
                return Err(RuntimeError::ArtifactError(
                    "usage: /steer [--urgent] <msg>".to_string(),
                ));
            }
            let kind = if urgent {
                SteeringKind::UrgentSteer
            } else {
                SteeringKind::Steer
            };
            let sig = host.kernel_mut().enqueue_steering(
                kind,
                Value::String(message.clone()),
                SteeringSource::User,
                current_epoch_ms(),
            )?;
            host.flush_pending()?;
            Ok(CommandDispatchResult {
                message: format!("Queued steering signal '{}'.", sig.id),
                data: json!({ "signal_id": sig.id, "kind": kind, "content": message }),
                notice: None,
                pending_pause: None,
            })
        }
        SlashCommand::Stop => Ok(CommandDispatchResult {
            message: "No active Turn is currently running.".to_string(),
            data: json!({ "stopped": false }),
            notice: None,
            pending_pause: None,
        }),
        SlashCommand::Fork { node_id, new_file } => {
            let ts = current_epoch_ms();
            let branch_id = format!("fork_{ts}");
            let new_path: Option<PathBuf> = if new_file {
                let parent = host
                    .session_path()
                    .parent()
                    .unwrap_or_else(|| host.workspace_root());
                let stem = host
                    .session_path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("session");
                Some(parent.join(format!("{stem}.{branch_id}.jsonl")))
            } else {
                None
            };
            let audit_id =
                host.fork_session(branch_id.clone(), node_id.as_deref(), new_path.as_deref())?;
            Ok(CommandDispatchResult {
                message: match &new_path {
                    Some(p) => format!("Forked branch '{branch_id}' to '{}'.", p.display()),
                    None => format!("Forked branch '{branch_id}' on active SessionTree."),
                },
                data: json!({
                    "branch_id": branch_id,
                    "audit_node_id": audit_id,
                    "new_file": new_path.map(|p| p.display().to_string()),
                }),
                notice: None,
                pending_pause: None,
            })
        }
        SlashCommand::Rewind { node_id } => {
            if node_id.is_empty() {
                return Err(RuntimeError::ArtifactError(
                    "usage: /rewind <node_id>".to_string(),
                ));
            }
            host.rewind_session("main".to_string(), &node_id)?;
            let notice = "Conversation state rewound; workspace files on disk were not modified."
                .to_string();
            Ok(CommandDispatchResult {
                message: format!("Rewound conversation head to '{node_id}'."),
                data: json!({
                    "rewound": true,
                    "target_node_id": node_id,
                    "disk_files_unchanged": true,
                }),
                notice: Some(notice),
                pending_pause: None,
            })
        }
        SlashCommand::Reload { ext_name } => {
            host.set_extra_read_roots(crate::extensions::builders::discover_global_skill_roots());
            let paths =
                crate::extensions::builders::discover_tracked_artifact_paths(host.workspace_root());
            host.set_tracked_artifact_paths(paths);
            if ext_name.is_empty() {
                let reloaded = host.reload_all_external_extensions().await?;
                Ok(CommandDispatchResult {
                    message: format!(
                        "Reloaded {} external extension(s) and refreshed skills/artifacts.",
                        reloaded.len()
                    ),
                    data: json!({ "reloaded": reloaded, "skills_refreshed": true }),
                    notice: None,
                    pending_pause: None,
                })
            } else {
                host.reload_external_extension(&ext_name).await?;
                Ok(CommandDispatchResult {
                    message: format!("Reloaded external extension '{ext_name}'."),
                    data: json!({ "reloaded": ext_name }),
                    notice: None,
                    pending_pause: None,
                })
            }
        }
        SlashCommand::Rollback { stage_id } => {
            if stage_id.is_empty() {
                return Err(RuntimeError::ArtifactError(
                    "usage: /rollback <stage_id>".to_string(),
                ));
            }
            let entry = host.rollback_stage(&stage_id)?;
            Ok(CommandDispatchResult {
                message: format!("Rolled back promoted stage '{stage_id}'."),
                data: json!({ "rolled_back": stage_id, "ledger_entry": entry }),
                notice: None,
                pending_pause: None,
            })
        }
        SlashCommand::New => {
            let ts = current_epoch_ms();
            let dir = host
                .session_path()
                .parent()
                .unwrap_or_else(|| host.workspace_root());
            let new_path = dir.join(format!("session_{ts}.jsonl"));
            host.reset_new_session(new_path.clone())?;
            state.extra_decorators.clear();
            Ok(CommandDispatchResult {
                message: format!("Started fresh session '{}'.", new_path.display()),
                data: json!({
                    "new_session": true,
                    "session_path": new_path.display().to_string(),
                }),
                notice: None,
                pending_pause: None,
            })
        }
        SlashCommand::Tree => {
            let tree = host.kernel().session_tree();
            Ok(CommandDispatchResult {
                message: format!(
                    "SessionTree has {} node(s) (head: {}). Use /rewind <node_id> or /fork [<node_id>].",
                    tree.all_nodes().len(),
                    tree.head_id().unwrap_or("none")
                ),
                data: json!({
                    "total_nodes": tree.all_nodes().len(),
                    "head_node_id": tree.head_id(),
                }),
                notice: None,
                pending_pause: None,
            })
        }
        SlashCommand::Session => {
            let tree = host.kernel().session_tree();
            let total_nodes = tree.all_nodes().len();
            let budget = host.kernel().budget_snapshot();
            let head = tree.head_id();
            Ok(CommandDispatchResult {
                message: format!(
                    "Session '{}' | preset={} | nodes={} | tokens={}",
                    host.session_path().display(),
                    state.active_preset,
                    total_nodes,
                    budget.tokens_used,
                ),
                data: json!({
                    "session_path": host.session_path().display().to_string(),
                    "workspace_root": host.workspace_root().display().to_string(),
                    "active_preset": state.active_preset,
                    "head_node_id": head,
                    "total_nodes": total_nodes,
                    "budget": budget,
                }),
                notice: None,
                pending_pause: None,
            })
        }
        SlashCommand::Trust => {
            let digest = host.trust_workspace()?;
            Ok(CommandDispatchResult {
                message: match &digest {
                    Some(d) => format!("Approved workspace .gestalt/gestalt.toml (sha256: {d})."),
                    None => {
                        "Workspace trust enabled (no .gestalt/gestalt.toml present).".to_string()
                    }
                },
                data: json!({ "trusted": true, "config_digest": digest }),
                notice: None,
                pending_pause: None,
            })
        }
        SlashCommand::Help => {
            let mut cmds: Vec<String> = KERNEL_COMMAND_SPECS
                .iter()
                .map(|(_, usage, _)| (*usage).to_string())
                .collect();
            for spec in host.kernel().registry().commands.values() {
                cmds.push(spec.usage.clone());
            }
            Ok(CommandDispatchResult {
                message: format!("Commands: {}", cmds.join(" | ")),
                data: json!({ "commands": cmds }),
                notice: None,
                pending_pause: None,
            })
        }
        SlashCommand::Quit => Ok(CommandDispatchResult {
            message: "Exiting session.".to_string(),
            data: json!({ "quit": true }),
            notice: None,
            pending_pause: None,
        }),
        SlashCommand::Extension { name, args }
            if matches!(
                name.as_str(),
                "loop" | "schedule" | "watch" | "webhook" | "triggers"
            ) =>
        {
            execute_trigger_slash_command(host, &name, &args).await
        }
        SlashCommand::Extension { name, args } => {
            let mut out = host.execute_command(&name, &args).await?;
            if out.get("command").and_then(Value::as_str) == Some("logout")
                && let Some(provider) = out
                    .get("provider")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            {
                let host_removed = host.credentials_mut().remove_credential(&provider)?;
                if host_removed && let Some(obj) = out.as_object_mut() {
                    obj.insert("removed".to_string(), Value::Bool(true));
                    obj.insert(
                        "message".to_string(),
                        Value::String(format!("Removed stored credentials for '{provider}'.")),
                    );
                }
            }
            if let Some(steer) = out.get("steering").and_then(Value::as_str) {
                host.kernel_mut().enqueue_steering(
                    SteeringKind::Steer,
                    Value::String(steer.to_string()),
                    SteeringSource::User,
                    current_epoch_ms(),
                )?;
                host.flush_pending()?;
            }
            if let Some(decs) = out.get("extra_decorators").and_then(Value::as_array) {
                for d in decs {
                    if let Some(s) = d.as_str()
                        && !state.extra_decorators.iter().any(|existing| existing == s)
                    {
                        state.extra_decorators.push(s.to_string());
                    }
                }
            }
            if let Some(patch) = out.get("params_patch").and_then(Value::as_object)
                && let Some(preset) = host
                    .kernel_mut()
                    .registry_mut()
                    .presets
                    .get_mut(&state.active_preset)
            {
                if !preset.config.params.is_object() {
                    preset.config.params = json!({});
                }
                if let Some(target_obj) = preset.config.params.as_object_mut() {
                    for (k, v) in patch {
                        target_obj.insert(k.clone(), v.clone());
                    }
                }
            }

            let pending_pause = out.get("pause_request").map(|pause_req| {
                let reason: PauseReason = pause_req
                    .get("reason")
                    .cloned()
                    .and_then(|v| serde_json::from_value(v).ok())
                    .unwrap_or(PauseReason::Input);
                let payload = pause_req.get("payload").cloned().unwrap_or(Value::Null);
                PendingCommandPause {
                    pause_id: format!("cmd_pause_{}", current_epoch_ms()),
                    reason,
                    payload,
                }
            });

            let message = out
                .get("message")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("Executed '/{name}'."));

            Ok(CommandDispatchResult {
                message,
                data: out,
                notice: None,
                pending_pause,
            })
        }
    }
}

/// Complete an idle command pause (e.g., `/login` `provider_login` dialog) after
/// the surface receives the user/client response.
///
/// # Errors
///
/// Returns [`RuntimeError`] if credential persistence fails.
pub fn complete_command_pause(
    host: &mut RuntimeHost,
    pause: &PendingCommandPause,
    resolution: &Value,
) -> Result<Value, RuntimeError> {
    if pause.payload.get("dialog").and_then(Value::as_str) == Some("provider_login")
        && let Some(provider) = pause.payload.get("provider").and_then(Value::as_str)
    {
        return persist_login_secret(host.credentials_mut(), provider, resolution);
    }
    Ok(json!({ "resolved": true, "pause_id": pause.pause_id }))
}

pub(crate) type SurfaceTurnSpawn = (
    mpsc::UnboundedSender<LiveTurnControl>,
    CancellationToken,
    oneshot::Receiver<(RuntimeHost, Result<TurnOutcome, RuntimeError>)>,
);

/// Shared Turn launcher used by `rpc`, `acp`, and `tui` surfaces.
pub(crate) fn spawn_surface_turn(
    mut host: RuntimeHost,
    prompt: Value,
    preset: String,
    extra_decorators: Vec<String>,
) -> SurfaceTurnSpawn {
    let (control_tx, control_rx) = mpsc::unbounded_channel();
    let (done_tx, done_rx) = oneshot::channel();
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        let res = host
            .run_turn_with_controls(
                prompt,
                &preset,
                &extra_decorators,
                cancel_clone,
                Some(control_rx),
            )
            .await;
        let _ = done_tx.send((host, res));
    });

    (control_tx, cancel, done_rx)
}

/// Write a JSON-RPC 2.0 success response line.
pub(crate) async fn write_jsonrpc_ok<W: AsyncWrite + Unpin>(
    writer: &mut W,
    id: Value,
    result: Value,
) -> Result<(), RuntimeError> {
    write_json_line(
        writer,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
    )
    .await
}

/// Write a JSON-RPC 2.0 error response line.
pub(crate) async fn write_jsonrpc_err<W: AsyncWrite + Unpin>(
    writer: &mut W,
    id: Value,
    code: i64,
    message: &str,
) -> Result<(), RuntimeError> {
    write_json_line(
        writer,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        }),
    )
    .await
}

/// Serialize `val` as a compact JSON line and flush `writer`.
pub(crate) async fn write_json_line<W: AsyncWrite + Unpin>(
    writer: &mut W,
    val: &Value,
) -> Result<(), RuntimeError> {
    let line = serde_json::to_string(val)?;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

/// Persist a resolved `provider_login` secret into [`CredentialStore`] without
/// returning any secret material in the confirmation JSON.
///
/// # Errors
///
/// Returns [`RuntimeError`] if the resolution lacks a non-empty secret or file I/O fails.
pub fn persist_login_secret(
    store: &mut CredentialStore,
    provider: &str,
    resolution: &Value,
) -> Result<Value, RuntimeError> {
    if let Ok(record) = serde_json::from_value::<CredentialRecord>(resolution.clone()) {
        store.set_record(provider, record)?;
        return Ok(json!({
            "provider": provider,
            "stored": true,
            "kind": "structured",
        }));
    }

    let secret = resolution
        .get("secret")
        .and_then(Value::as_str)
        .or_else(|| resolution.get("api_key").and_then(Value::as_str))
        .or_else(|| resolution.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            RuntimeError::ArtifactError("login resolution missing non-empty 'secret'".to_string())
        })?;

    store.set_credential(provider, secret.to_string())?;
    Ok(json!({
        "provider": provider,
        "stored": true,
        "kind": "api_key",
    }))
}

/// Extract displayable text from a `RunEvent::StreamDelta` payload.
#[must_use]
pub fn extract_stream_text(payload: &Value) -> Option<&str> {
    payload
        .get("text")
        .and_then(Value::as_str)
        .or_else(|| payload.as_str())
}

async fn execute_trigger_slash_command(
    host: &mut RuntimeHost,
    name: &str,
    args: &str,
) -> Result<CommandDispatchResult, RuntimeError> {
    let trimmed = args.trim();
    match name {
        "triggers" => {
            let (sub, rest) = match trimmed.split_once(char::is_whitespace) {
                Some((s, r)) => (s, r.trim()),
                None => (trimmed, ""),
            };
            match sub {
                "" | "list" => {
                    let list = host.list_triggers().await;
                    let msg = if list.is_empty() {
                        "No triggers configured.".to_string()
                    } else {
                        let summary: Vec<String> = list
                            .iter()
                            .map(|t| {
                                format!(
                                    "{} ({}, {}, fires={})",
                                    t.id,
                                    t.kind,
                                    if t.enabled { "active" } else { "paused" },
                                    t.fire_count
                                )
                            })
                            .collect();
                        format!("Triggers ({}): {}", list.len(), summary.join(" | "))
                    };
                    Ok(CommandDispatchResult {
                        message: msg,
                        data: json!({ "triggers": list }),
                        notice: None,
                        pending_pause: None,
                    })
                }
                "pause" if !rest.is_empty() => {
                    let paused = host.pause_trigger(rest).await;
                    Ok(CommandDispatchResult {
                        message: if paused {
                            format!("Paused trigger '{rest}'.")
                        } else {
                            format!("Unknown trigger '{rest}'.")
                        },
                        data: json!({ "paused": paused, "id": rest }),
                        notice: None,
                        pending_pause: None,
                    })
                }
                "resume" if !rest.is_empty() => {
                    host.resume_trigger(rest).await?;
                    Ok(CommandDispatchResult {
                        message: format!("Resumed trigger '{rest}'."),
                        data: json!({ "resumed": true, "id": rest }),
                        notice: None,
                        pending_pause: None,
                    })
                }
                "stop" | "remove" | "rm" if !rest.is_empty() => {
                    let removed = host.remove_trigger(rest).await;
                    Ok(CommandDispatchResult {
                        message: if removed {
                            format!("Removed trigger '{rest}'.")
                        } else {
                            format!("Unknown trigger '{rest}'.")
                        },
                        data: json!({ "removed": removed, "id": rest }),
                        notice: None,
                        pending_pause: None,
                    })
                }
                "fire" if !rest.is_empty() => {
                    let ev = host.fire_trigger_now(rest).await?;
                    Ok(CommandDispatchResult {
                        message: format!("Fired trigger '{rest}'."),
                        data: json!({ "fired": true, "event": ev }),
                        notice: None,
                        pending_pause: None,
                    })
                }
                _ => Err(RuntimeError::ArtifactError(
                    "usage: /triggers [list|pause <id>|resume <id>|stop <id>|fire <id>]"
                        .to_string(),
                )),
            }
        }
        "loop" => {
            if trimmed == "list" {
                return Box::pin(execute_trigger_slash_command(host, "triggers", "list")).await;
            }
            if let Some(id) = trimmed.strip_prefix("stop ") {
                let removed = host.remove_trigger(id.trim()).await;
                return Ok(CommandDispatchResult {
                    message: format!("Removed trigger '{}': {removed}", id.trim()),
                    data: json!({ "removed": removed, "id": id.trim() }),
                    notice: None,
                    pending_pause: None,
                });
            }
            let Some((interval, prompt)) = trimmed.split_once(char::is_whitespace) else {
                return Err(RuntimeError::ArtifactError(
                    "usage: /loop <interval> <prompt>".to_string(),
                ));
            };
            let id = format!("loop_{}", current_epoch_ms());
            host.start_trigger(TriggerInstanceConfig {
                id: id.clone(),
                kind: "interval".to_string(),
                enabled: true,
                prompt: Value::String(prompt.trim().to_string()),
                preset: None,
                extra_decorators: Vec::new(),
                config: json!({ "interval": interval.trim() }),
            })
            .await?;
            Ok(CommandDispatchResult {
                message: format!("Started interval trigger '{id}' every {}.", interval.trim()),
                data: json!({ "started": true, "id": id, "kind": "interval" }),
                notice: None,
                pending_pause: None,
            })
        }
        "schedule" => {
            if trimmed == "list" {
                return Box::pin(execute_trigger_slash_command(host, "triggers", "list")).await;
            }
            if let Some(id) = trimmed.strip_prefix("stop ") {
                let removed = host.remove_trigger(id.trim()).await;
                return Ok(CommandDispatchResult {
                    message: format!("Removed trigger '{}': {removed}", id.trim()),
                    data: json!({ "removed": removed, "id": id.trim() }),
                    notice: None,
                    pending_pause: None,
                });
            }
            let (expr, prompt) = if let Some(rest) = trimmed.strip_prefix('"') {
                rest.split_once('"').ok_or_else(|| {
                    RuntimeError::ArtifactError("usage: /schedule \"<cron>\" <prompt>".to_string())
                })?
            } else {
                let parts: Vec<&str> = trimmed.splitn(6, char::is_whitespace).collect();
                if parts.len() < 6 {
                    return Err(RuntimeError::ArtifactError(
                        "usage: /schedule \"<cron>\" <prompt>".to_string(),
                    ));
                }
                let expr_str = format!(
                    "{} {} {} {} {}",
                    parts[0], parts[1], parts[2], parts[3], parts[4]
                );
                let id = format!("sched_{}", current_epoch_ms());
                host.start_trigger(TriggerInstanceConfig {
                    id: id.clone(),
                    kind: "cron_schedule".to_string(),
                    enabled: true,
                    prompt: Value::String(parts[5].trim().to_string()),
                    preset: None,
                    extra_decorators: Vec::new(),
                    config: json!({ "expression": expr_str }),
                })
                .await?;
                return Ok(CommandDispatchResult {
                    message: format!("Started cron schedule trigger '{id}' ({expr_str})."),
                    data: json!({ "started": true, "id": id, "kind": "cron_schedule" }),
                    notice: None,
                    pending_pause: None,
                });
            };
            let prompt_trimmed = prompt.trim();
            if prompt_trimmed.is_empty() {
                return Err(RuntimeError::ArtifactError(
                    "usage: /schedule \"<cron>\" <prompt>".to_string(),
                ));
            }
            let id = format!("sched_{}", current_epoch_ms());
            host.start_trigger(TriggerInstanceConfig {
                id: id.clone(),
                kind: "cron_schedule".to_string(),
                enabled: true,
                prompt: Value::String(prompt_trimmed.to_string()),
                preset: None,
                extra_decorators: Vec::new(),
                config: json!({ "expression": expr.trim() }),
            })
            .await?;
            Ok(CommandDispatchResult {
                message: format!("Started cron schedule trigger '{id}' ({}).", expr.trim()),
                data: json!({ "started": true, "id": id, "kind": "cron_schedule" }),
                notice: None,
                pending_pause: None,
            })
        }
        "watch" => {
            if let Some(id) = trimmed.strip_prefix("stop ") {
                let removed = host.remove_trigger(id.trim()).await;
                return Ok(CommandDispatchResult {
                    message: format!("Removed trigger '{}': {removed}", id.trim()),
                    data: json!({ "removed": removed, "id": id.trim() }),
                    notice: None,
                    pending_pause: None,
                });
            }
            let Some((path, prompt)) = trimmed.split_once(char::is_whitespace) else {
                return Err(RuntimeError::ArtifactError(
                    "usage: /watch <path> <prompt>".to_string(),
                ));
            };
            let id = format!("watch_{}", current_epoch_ms());
            host.start_trigger(TriggerInstanceConfig {
                id: id.clone(),
                kind: "fs_watch".to_string(),
                enabled: true,
                prompt: Value::String(prompt.trim().to_string()),
                preset: None,
                extra_decorators: Vec::new(),
                config: json!({ "paths": [path.trim()] }),
            })
            .await?;
            Ok(CommandDispatchResult {
                message: format!("Started fs_watch trigger '{id}' on '{}'.", path.trim()),
                data: json!({ "started": true, "id": id, "kind": "fs_watch" }),
                notice: None,
                pending_pause: None,
            })
        }
        "webhook" => {
            if let Some(id) = trimmed.strip_prefix("stop ") {
                let removed = host.remove_trigger(id.trim()).await;
                return Ok(CommandDispatchResult {
                    message: format!("Removed trigger '{}': {removed}", id.trim()),
                    data: json!({ "removed": removed, "id": id.trim() }),
                    notice: None,
                    pending_pause: None,
                });
            }
            let Some((bind_addr, prompt)) = trimmed.split_once(char::is_whitespace) else {
                return Err(RuntimeError::ArtifactError(
                    "usage: /webhook <addr> <prompt>".to_string(),
                ));
            };
            let id = format!("hook_{}", current_epoch_ms());
            host.start_trigger(TriggerInstanceConfig {
                id: id.clone(),
                kind: "webhook".to_string(),
                enabled: true,
                prompt: Value::String(prompt.trim().to_string()),
                preset: None,
                extra_decorators: Vec::new(),
                config: json!({ "bind_addr": bind_addr.trim() }),
            })
            .await?;
            Ok(CommandDispatchResult {
                message: format!("Started webhook trigger '{id}' on '{}'.", bind_addr.trim()),
                data: json!({ "started": true, "id": id, "kind": "webhook" }),
                notice: None,
                pending_pause: None,
            })
        }
        _ => Err(RuntimeError::ArtifactError(format!(
            "unknown trigger command '/{name}'"
        ))),
    }
}

pub(crate) type TriggerRpcFuture =
    Pin<Box<dyn Future<Output = Result<Value, (i64, String)>> + Send + 'static>>;

/// Shared JSON-RPC / ACP trigger management dispatcher (`list`, `add`, `remove`,
/// `pause`, `resume`, `fire`).
///
/// Returns a `'static` future so callers inside `tokio::spawn` do not hold
/// `&RuntimeHost` (`!Sync`) across `.await` points.
pub(crate) fn dispatch_trigger_rpc(
    host: &RuntimeHost,
    op: &str,
    params: Value,
) -> TriggerRpcFuture {
    match op {
        "list" => {
            let fut = host.list_triggers();
            Box::pin(async move { Ok(json!({ "triggers": fut.await })) })
        }
        "add" => match serde_json::from_value::<TriggerInstanceConfig>(params) {
            Ok(inst) => {
                let tid = inst.id.clone();
                let fut = host.start_trigger(inst);
                Box::pin(async move {
                    fut.await
                        .map(|()| json!({ "added": true, "id": tid }))
                        .map_err(|e| (RPC_INTERNAL_ERROR, e.to_string()))
                })
            }
            Err(e) => Box::pin(async move {
                Err((RPC_INVALID_PARAMS, format!("invalid trigger config: {e}")))
            }),
        },
        "remove" | "pause" | "resume" | "fire" => {
            let Some(tid) = params.get("id").and_then(Value::as_str) else {
                return Box::pin(async { Err((RPC_INVALID_PARAMS, "missing 'id'".to_string())) });
            };
            match op {
                "remove" => {
                    let fut = host.remove_trigger(tid);
                    Box::pin(async move { Ok(json!({ "removed": fut.await })) })
                }
                "pause" => {
                    let fut = host.pause_trigger(tid);
                    Box::pin(async move { Ok(json!({ "paused": fut.await })) })
                }
                "resume" => {
                    let fut = host.resume_trigger(tid);
                    Box::pin(async move {
                        fut.await
                            .map(|()| json!({ "resumed": true }))
                            .map_err(|e| (RPC_INTERNAL_ERROR, e.to_string()))
                    })
                }
                _ => {
                    let fut = host.fire_trigger_now(tid);
                    Box::pin(async move {
                        fut.await
                            .map(|ev| json!({ "fired": true, "event": ev }))
                            .map_err(|e| (RPC_INTERNAL_ERROR, e.to_string()))
                    })
                }
            }
        }
        _ => {
            let msg = format!("unknown trigger op: {op}");
            Box::pin(async move { Err((RPC_METHOD_NOT_FOUND, msg)) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, BudgetCap, ContextBuilder, KernelRuntime, LoopAction,
        LoopConfig, LoopContext, ModelCapabilities, PolicyGate, SandboxPolicy, SessionSlice,
        StepResult,
    };
    use gestalt_runtime::sandbox::UnsandboxedFallbackDriver;
    use gestalt_runtime::{CommandHandler, RuntimeRegistry};
    use std::fs;
    use std::time::Duration;

    struct EchoCmd;
    impl CommandHandler for EchoCmd {
        fn execute<'a>(
            &'a self,
            args: &'a str,
            _kernel: &'a mut KernelRuntime,
        ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
            let provider = args.trim().to_string();
            Box::pin(async move {
                Ok(json!({
                    "pause_request": {
                        "reason": PauseReason::Input,
                        "payload": {
                            "dialog": "provider_login",
                            "provider": provider,
                            "secret": true,
                        }
                    }
                }))
            })
        }
    }

    struct DoneLoop;
    impl AgentLoop for DoneLoop {
        fn id(&self) -> &str {
            "done"
        }
        fn digest(&self) -> &str {
            "d1"
        }
        fn step(
            &mut self,
            _ctx: &mut dyn LoopContext,
            _results: Option<Vec<StepResult>>,
        ) -> LoopAction {
            LoopAction::Done(json!("ok"))
        }
    }
    struct DoneFactory;
    impl AgentLoopFactory for DoneFactory {
        fn id(&self) -> &str {
            "done"
        }
        fn digest(&self) -> &str {
            "d1"
        }
        fn create(&self, _c: &LoopConfig, _r: &ExtensionRegistry) -> Box<dyn AgentLoop> {
            Box::new(DoneLoop)
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

    #[test]
    fn slash_command_parser_covers_all_kernel_and_extension_commands() {
        assert_eq!(parse_slash_command("hello world"), None);
        assert_eq!(parse_slash_command("/"), None);
        assert_eq!(
            parse_slash_command("/mode general"),
            Some(SlashCommand::Mode {
                preset: "general".into()
            })
        );
        assert_eq!(parse_slash_command("/new"), Some(SlashCommand::New));
        assert_eq!(parse_slash_command("/tree"), Some(SlashCommand::Tree));
        assert_eq!(parse_slash_command("/session"), Some(SlashCommand::Session));
        assert_eq!(parse_slash_command("/resume"), Some(SlashCommand::Session));
        assert_eq!(parse_slash_command("/trust"), Some(SlashCommand::Trust));
        assert_eq!(parse_slash_command("/help"), Some(SlashCommand::Help));
        assert_eq!(parse_slash_command("/hotkeys"), Some(SlashCommand::Help));
        assert_eq!(parse_slash_command("/quit"), Some(SlashCommand::Quit));
        assert_eq!(parse_slash_command("/exit"), Some(SlashCommand::Quit));
        assert_eq!(
            parse_slash_command("/skill:deploy prod"),
            Some(SlashCommand::Extension {
                name: "skills".into(),
                args: "load:deploy prod".into(),
            })
        );
        assert_eq!(
            parse_slash_command("/steer use cargo check"),
            Some(SlashCommand::Steer {
                urgent: false,
                message: "use cargo check".into()
            })
        );
        assert_eq!(
            parse_slash_command("/steer --urgent stop now"),
            Some(SlashCommand::Steer {
                urgent: true,
                message: "stop now".into()
            })
        );
        assert_eq!(parse_slash_command("/stop"), Some(SlashCommand::Stop));
        assert_eq!(
            parse_slash_command("/fork"),
            Some(SlashCommand::Fork {
                node_id: None,
                new_file: false
            })
        );
        assert_eq!(
            parse_slash_command("/fork n_4 --new-file"),
            Some(SlashCommand::Fork {
                node_id: Some("n_4".into()),
                new_file: true
            })
        );
        assert_eq!(
            parse_slash_command("/rewind n_2"),
            Some(SlashCommand::Rewind {
                node_id: "n_2".into()
            })
        );
        assert_eq!(
            parse_slash_command("/reload my_router"),
            Some(SlashCommand::Reload {
                ext_name: "my_router".into()
            })
        );
        assert_eq!(
            parse_slash_command("/rollback stg_1"),
            Some(SlashCommand::Rollback {
                stage_id: "stg_1".into()
            })
        );
        assert_eq!(
            parse_slash_command("/goal cargo test passes"),
            Some(SlashCommand::Extension {
                name: "goal".into(),
                args: "cargo test passes".into()
            })
        );
    }

    #[test]
    fn exit_codes_map_runtime_errors_deterministically() {
        assert_eq!(
            SurfaceExitCode::from_runtime_error(&RuntimeError::HeadlessPauseDenied("no".into())),
            SurfaceExitCode::PolicyOrPauseDenied
        );
        assert_eq!(
            SurfaceExitCode::from_runtime_error(&RuntimeError::UntrustedWorkspace {
                workspace_root: "/repo".into(),
                config_digest: "abc".into()
            }),
            SurfaceExitCode::PolicyOrPauseDenied
        );
        let json_err = serde_json::from_str::<Value>("{").unwrap_err();
        assert_eq!(
            SurfaceExitCode::from_runtime_error(&RuntimeError::Json(json_err)),
            SurfaceExitCode::ConfigOrParseError
        );
        assert_eq!(
            SurfaceExitCode::from_runtime_error(&RuntimeError::ArtifactError("boom".into())),
            SurfaceExitCode::RuntimeError
        );
    }

    #[tokio::test]
    async fn interactive_pause_bridge_fails_closed_on_cancellation() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let bridge = InteractivePauseBridge::new(tx);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        tokio::spawn(async move {
            let _ = rx.recv().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel_clone.cancel();
        });

        let payload = json!({ "action": "promote" });
        let res = bridge
            .resolve_pause("p1", "e1", PauseReason::Approval, &payload, cancel)
            .await;
        assert!(matches!(res, Err(RuntimeError::HeadlessPauseDenied(_))));
        assert!(!bridge.resolve("p1", json!({ "approved": true })));
    }

    #[tokio::test]
    async fn idle_slash_command_rewind_notice_and_login_secret_persistence() {
        let dir = std::env::temp_dir().join(format!("gestalt_surf_mod_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");

        let mut reg = ExtensionRegistry::new();
        reg.loops.insert("done".into(), Box::new(DoneFactory));
        reg.builders.insert("default".into(), Box::new(NullBuilder));
        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let mut rt = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
        rt.commands.insert("login".into(), Arc::new(EchoCmd));

        let cred_path = dir.join("credentials.toml");
        let creds = CredentialStore::load_from_path(cred_path.clone()).expect("creds");
        let mut host = RuntimeHost::new(
            "r_surf".into(),
            &dir,
            dir.join(".gestalt/sessions/s.jsonl"),
            reg,
            gate,
            rt,
            creds,
            None,
            Arc::new(gestalt_runtime::HeadlessPauseHandler::default()),
        )
        .expect("host");

        let root_id = host
            .kernel()
            .session_tree()
            .head_id()
            .expect("head")
            .to_string();
        let mut state = SurfaceSessionState::new("done");

        let rewind_res = execute_slash_command_idle(
            &mut host,
            &mut state,
            SlashCommand::Rewind { node_id: root_id },
        )
        .await
        .expect("rewind");
        assert!(
            rewind_res
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("not modified"))
        );

        let login_res = execute_slash_command_idle(
            &mut host,
            &mut state,
            SlashCommand::Extension {
                name: "login".into(),
                args: "anthropic".into(),
            },
        )
        .await
        .expect("login");
        let pending = login_res.pending_pause.expect("non-blocking pause");
        let stored = complete_command_pause(
            &mut host,
            &pending,
            &json!({ "secret": "sk-top-secret-123" }),
        )
        .expect("complete login");
        assert_eq!(stored["stored"], true);
        assert!(!stored.to_string().contains("sk-top-secret-123"));
        assert_eq!(
            host.credentials().get("anthropic", None).as_deref(),
            Some("sk-top-secret-123")
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
