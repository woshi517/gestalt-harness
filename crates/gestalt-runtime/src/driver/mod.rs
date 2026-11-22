#![forbid(unsafe_code)]
//! Tokio `Effect` execution driver, `RuntimeHost`, and `RuntimeRegistry`.
//!
//! Submodules separate focused concerns:
//! - [`dispatch`]: parallel `Model`/`Tool`/`Save`/`Spawn` execution & schema validation
//! - [`session_io`]: advisory `.lock` management & crash-safe JSONL flushing
//! - [`worktree`]: detached git worktree lifecycle for `WorkspaceMode::Worktree`
//! - [`replay`]: `--reexecute-ext` sandboxed replay runner

pub mod dispatch;
pub mod replay;
pub mod session_io;
pub mod worktree;

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use gestalt_kernel::{
    DriverDirective, DriverResponse, Effect, EffectEnvelope, ExtensionRegistry, KernelError,
    KernelRuntime, LoopConfig, ModelKind, PauseReason, PolicyDecision, PolicyGate, PolicyRule,
    RunEvent, SandboxPolicy, SessionSlice, SteeringKind, SteeringSource, StepResult, Usage,
    sha256_hex,
};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;

use crate::artifacts::{ArtifactManager, LedgerEntry};
use crate::blobs::BlobStore;
use crate::config::{ExtensionProtocol, GestaltConfig, ResolvedRuntimeConfig, WorkspaceTrustStore};
use crate::credentials::{CredentialStore, StreamRedactor};
use crate::ext_ipc::{
    ExtIpcHost, ExternalBuilderRequest, ExternalDecoratorStepRequest, ExternalLoopStepRequest,
};
use crate::mcp::McpClient;
use crate::provider::{ProviderRegistry, SharedHttpClient, StreamDeltaFn};
use crate::sandbox::{SandboxDriver, WorkspacePathGuard};
use crate::trigger::{
    TriggerContext, TriggerEvent, TriggerHandler, TriggerInstanceConfig, TriggerManager,
    TriggerStatus,
};
use crate::{CancellationToken, RuntimeError};

pub use dispatch::validate_tool_args;
pub use replay::{ExternalReplayOptions, replay_session_tree};
pub use session_io::{SessionLockGuard, current_epoch_ms};
pub use worktree::{capture_worktree_diff_blob, create_worktree, prune_clean_worktrees};

/// Outcome returned by a [`ToolHandler`] execution.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    /// Structured JSON or text output from the tool.
    pub output: Value,
    /// `true` if the tool encountered a recoverable execution/validation error.
    pub is_error: bool,
}

/// Execution context supplied to in-process [`ToolHandler`] implementations.
#[derive(Clone)]
pub struct ToolExecutionContext {
    /// Canonical workspace root path.
    pub workspace_root: PathBuf,
    /// Working directory for tool execution.
    pub cwd: PathBuf,
    /// Active Turn ID.
    pub turn_id: String,
    /// Active Phase ID.
    pub phase_id: String,
    /// Correlated `effect_id`.
    pub effect_id: String,
    /// Active `SandboxPolicy` invariants.
    pub sandbox_policy: SandboxPolicy,
    /// In-process path confinement and `protected_paths` guard.
    pub path_guard: WorkspacePathGuard,
    /// OS-level subprocess sandbox driver.
    pub sandbox_driver: Arc<dyn SandboxDriver>,
    /// Content-addressed blob store for oversized outputs.
    pub blob_store: BlobStore,
    /// Cooperative cancellation token for `<= 250 ms` abort.
    pub cancel: CancellationToken,
    /// Optional event sink for transient `RunEvent::ToolUi` emissions.
    pub event_sink: Option<Arc<dyn EventSink>>,
    /// Host stream redactor applied to `ToolUi` payloads.
    pub redactor: StreamRedactor,
}

impl ToolExecutionContext {
    /// Emit a transient, secret-redacted [`RunEvent::ToolUi`] payload targeting
    /// a registered `UISurface` renderer (e.g., `"diff_preview"`).
    pub fn emit_tool_ui(&self, renderer: &str, mut data: Value) {
        let Some(ref sink) = self.event_sink else {
            return;
        };
        self.redactor.redact_json(&mut data);
        sink.on_event(&RunEvent::ToolUi {
            turn_id: self.turn_id.clone(),
            effect_id: self.effect_id.clone(),
            renderer: renderer.to_string(),
            data,
        });
    }
}

/// Async handler contract for registered tools in [`RuntimeRegistry`].
pub trait ToolHandler: Send + Sync {
    /// Execute the tool with validated `args`, streaming optional deltas via `on_delta`.
    fn execute<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolExecutionContext,
        on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ToolOutcome, RuntimeError>> + Send + 'a>>;
}

/// Async handler contract for extension-registered slash commands.
pub trait CommandHandler: Send + Sync {
    /// Execute the slash command with raw argument text against the active [`KernelRuntime`].
    fn execute<'a>(
        &'a self,
        args: &'a str,
        kernel: &'a mut KernelRuntime,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>>;
}

/// Async handler resolving `Effect::Pause` dialogs (TUI, RPC, ACP, or headless policy).
pub trait PauseHandler: Send + Sync {
    /// Resolve a pause dialog or return [`RuntimeError::HeadlessPauseDenied`] if denied.
    fn resolve_pause<'a>(
        &'a self,
        pause_id: &'a str,
        effect_id: &'a str,
        reason: PauseReason,
        payload: &'a Value,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>>;
}

/// Sink receiving canonical and transient [`RunEvent`]s emitted during execution.
pub trait EventSink: Send + Sync {
    /// Receive a redacted [`RunEvent`].
    fn on_event(&self, event: &RunEvent);
}

/// Fail-closed headless [`PauseHandler`] permitting only reasons/actions listed
/// in `[policy].headless_auto_approve`.
#[derive(Debug, Clone, Default)]
pub struct HeadlessPauseHandler {
    /// Allowed pause reasons or `payload.action` strings.
    pub auto_approve: Vec<String>,
}

impl PauseHandler for HeadlessPauseHandler {
    fn resolve_pause<'a>(
        &'a self,
        _pause_id: &'a str,
        _effect_id: &'a str,
        reason: PauseReason,
        payload: &'a Value,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        let reason_str = match reason {
            PauseReason::Approval => "approval",
            PauseReason::Input => "input",
            PauseReason::Checkpoint => "checkpoint",
        };
        let action_str = payload.get("action").and_then(Value::as_str);
        let allowed = self
            .auto_approve
            .iter()
            .any(|entry| entry == reason_str || action_str.is_some_and(|act| act == entry));
        Box::pin(async move {
            if allowed {
                Ok(json!({ "approved": true, "auto_approved": true }))
            } else {
                Err(RuntimeError::HeadlessPauseDenied(format!(
                    "pause reason '{reason_str}' is not listed in [policy].headless_auto_approve"
                )))
            }
        })
    }
}

/// Bridges out-of-process `policy/evaluate` rules from `ExtensionRegistry` into
/// `PolicyGate` via `PolicyDecision::RequiresVerification` (failing closed on
/// timeout, crash, or non-allow response).
#[derive(Debug, Clone)]
pub struct ExternalPolicyBridgeRule {
    rule_id: String,
}

impl ExternalPolicyBridgeRule {
    /// Create a new fail-closed bridge rule for external policy `rule_id`.
    #[must_use]
    pub fn new(rule_id: impl Into<String>) -> Self {
        Self {
            rule_id: rule_id.into(),
        }
    }
}

impl PolicyRule for ExternalPolicyBridgeRule {
    fn id(&self) -> &str {
        &self.rule_id
    }

    fn evaluate(&self, env: &EffectEnvelope, _session: &SessionSlice) -> PolicyDecision {
        PolicyDecision::RequiresVerification {
            rule_id: self.rule_id.clone(),
            verifier_effect: Box::new(Effect::Model {
                target: format!("ext_policy:{}", self.rule_id),
                kind: ModelKind::Decision,
                input: serde_json::to_value(env).unwrap_or(Value::Null),
                schema: None,
            }),
            pending_effect: Box::new(env.clone()),
        }
    }

    fn evaluate_verification(
        &self,
        _pending: &EffectEnvelope,
        verifier_result: &StepResult,
    ) -> PolicyDecision {
        if let StepResult::Model { output, .. } = verifier_result
            && output.get("decision").and_then(Value::as_str) == Some("allow")
        {
            return PolicyDecision::Allow;
        }
        PolicyDecision::Deny {
            reason: format!(
                "external policy rule '{}' did not approve effect (fail-closed)",
                self.rule_id
            ),
        }
    }
}

/// Asynchronous steering injection message delivered to [`RuntimeHost::run_turn_with_steering`].
#[derive(Debug, Clone)]
pub struct LiveSteeringMessage {
    /// Steering kind (`Steer`, `UrgentSteer`, `FollowUp`, `Abort`).
    pub kind: SteeringKind,
    /// Instruction or cancellation reason payload.
    pub content: Value,
    /// Entity that issued the signal.
    pub source: SteeringSource,
}

/// Asynchronous mid-turn control message delivered to [`RuntimeHost::run_turn_with_controls`].
#[derive(Debug, Clone)]
pub enum LiveTurnControl {
    /// Inject a live steering or abort signal.
    Steering(LiveSteeringMessage),
    /// Switch the active Phase mid-turn at the next safe step boundary.
    SwitchMode {
        /// Target preset or loop ID.
        preset_or_loop: String,
        /// Typed handoff JSON passed to the next Phase.
        handoff: Value,
        /// Optional `LoopConfig` overrides.
        config_override: Option<LoopConfig>,
    },
    /// Deliver a background [`TriggerEvent`] while a Turn is active, queueing
    /// it as a durable `FollowUp` without interrupting the active Step.
    Trigger(TriggerEvent),
}

/// Host registry binding async runtime handlers (`ToolHandler`, `CommandHandler`,
/// `ProviderRegistry`, `SandboxDriver`, `SharedHttpClient`, `ExtIpcHost`, `McpClient`).
pub struct RuntimeRegistry {
    /// Registered in-process tool execution handlers.
    pub tools: BTreeMap<String, Arc<dyn ToolHandler>>,
    /// Registered in-process slash command handlers.
    pub commands: BTreeMap<String, Arc<dyn CommandHandler>>,
    /// Model provider registry and config tables.
    pub providers: ProviderRegistry,
    /// Shared HTTP/SSE client passed to `ModelProvider`s via `ProviderContext`.
    pub http: SharedHttpClient,
    /// Active OS-level subprocess sandbox driver.
    pub sandbox_driver: Arc<dyn SandboxDriver>,
    /// Active out-of-process stdio JSON-RPC extension workers.
    pub ext_ipc: Arc<Mutex<ExtIpcHost>>,
    /// Active stdio MCP clients keyed by extension name.
    pub mcp_clients: BTreeMap<String, Arc<Mutex<McpClient>>>,
    /// Mapping from MCP tool name to owning MCP client key.
    pub mcp_tool_owners: BTreeMap<String, String>,
    /// Registered user-space [`TriggerHandler`] implementations keyed by `kind`.
    pub trigger_handlers: BTreeMap<String, Arc<dyn TriggerHandler>>,
    /// Host supervisor managing active background trigger tasks.
    pub trigger_manager: Arc<Mutex<TriggerManager>>,
}

impl RuntimeRegistry {
    /// Create a [`RuntimeRegistry`] with `sandbox_driver` and a default 120s HTTP client.
    #[must_use]
    pub fn new(sandbox_driver: Arc<dyn SandboxDriver>) -> Self {
        let http = SharedHttpClient::new(120)
            .unwrap_or_else(|_| SharedHttpClient::new(0).expect("reqwest client init"));
        Self {
            tools: BTreeMap::new(),
            commands: BTreeMap::new(),
            providers: ProviderRegistry::new(),
            http,
            sandbox_driver,
            ext_ipc: Arc::new(Mutex::new(ExtIpcHost::new())),
            mcp_clients: BTreeMap::new(),
            mcp_tool_owners: BTreeMap::new(),
            trigger_handlers: BTreeMap::new(),
            trigger_manager: Arc::new(Mutex::new(TriggerManager::new())),
        }
    }

    /// Connect all configured external RPC and MCP extensions from `resolved`
    /// prior to constructing `KernelRuntime`, registering their descriptors into
    /// `registry` and their additive fail-closed policy rules into `policy_gate`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if spawning, `extension/describe`, or MCP
    /// initialization fails.
    pub async fn connect_configured_extensions(
        &mut self,
        workspace_root: &Path,
        resolved: &ResolvedRuntimeConfig,
        redactor: &StreamRedactor,
        policy_gate: &mut PolicyGate,
        registry: &mut ExtensionRegistry,
    ) -> Result<(), RuntimeError> {
        self.providers.merge_configs(resolved.providers.clone());
        let sandbox_policy = policy_gate.sandbox().clone();

        for (name, ext_cfg) in &resolved.extensions {
            match ext_cfg.protocol {
                ExtensionProtocol::Rpc => {
                    let spec = {
                        let mut ipc = self.ext_ipc.lock().await;
                        ipc.start_and_register(
                            name,
                            ext_cfg.clone(),
                            workspace_root.to_path_buf(),
                            sandbox_policy.clone(),
                            self.sandbox_driver.clone(),
                            redactor.clone(),
                            registry,
                        )
                        .await?
                    };
                    for pol in &spec.policies {
                        policy_gate.add_rule(Box::new(ExternalPolicyBridgeRule::new(&pol.id)));
                    }
                }
                ExtensionProtocol::Mcp => {
                    self.connect_or_reload_mcp(
                        name,
                        ext_cfg,
                        workspace_root,
                        &sandbox_policy,
                        redactor.clone(),
                        registry,
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn connect_or_reload_mcp(
        &mut self,
        name: &str,
        ext_cfg: &crate::config::ExternalExtensionConfig,
        workspace_root: &Path,
        sandbox_policy: &SandboxPolicy,
        redactor: StreamRedactor,
        registry: &mut ExtensionRegistry,
    ) -> Result<(), RuntimeError> {
        if let Some(old_client) = self.mcp_clients.remove(name) {
            let mut guard = old_client.lock().await;
            let _ = guard.shutdown().await;
        }
        self.mcp_tool_owners.retain(|_, owner| owner != name);
        let mut client = McpClient::connect(
            name.to_string(),
            ext_cfg,
            workspace_root,
            sandbox_policy,
            &self.sandbox_driver,
            redactor,
        )
        .await?;
        let tool_names = client.register_tools(registry).await?;
        for t in tool_names {
            self.mcp_tool_owners.insert(t, name.to_string());
        }
        self.mcp_clients
            .insert(name.to_string(), Arc::new(Mutex::new(client)));
        Ok(())
    }

    fn clone_for_spawn(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            commands: self.commands.clone(),
            providers: ProviderRegistry::new(),
            http: self.http.clone(),
            sandbox_driver: self.sandbox_driver.clone(),
            ext_ipc: self.ext_ipc.clone(),
            mcp_clients: self.mcp_clients.clone(),
            mcp_tool_owners: self.mcp_tool_owners.clone(),
            trigger_handlers: BTreeMap::new(),
            trigger_manager: Arc::new(Mutex::new(TriggerManager::new())),
        }
    }
}

/// Terminal outcome of a single Turn executed by [`RuntimeHost::run_turn`].
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    /// Turn completed cleanly via `LoopAction::Done`.
    Completed {
        /// Completed Turn ID.
        turn_id: String,
        /// Final Turn output JSON.
        output: Value,
        /// Accumulated Turn usage.
        usage: Usage,
    },
    /// Turn was aborted cleanly (e.g., `/stop`, `Ctrl+C`, or `session/cancel`).
    Aborted {
        /// Aborted Turn ID.
        turn_id: String,
        /// Cancellation reason.
        reason: String,
    },
}

/// Complete Tokio I/O host coordinating `KernelRuntime`, atomic session JSONL
/// persistence, OS sandboxes, credentials, artifacts, providers, and external IPC.
pub struct RuntimeHost {
    run_id: String,
    workspace_root: PathBuf,
    kernel: KernelRuntime,
    runtime: RuntimeRegistry,
    credentials: CredentialStore,
    artifacts: ArtifactManager,
    blob_store: BlobStore,
    session_path: PathBuf,
    session_file: File,
    _lock: SessionLockGuard,
    event_sink: Option<Arc<dyn EventSink>>,
    pause_handler: Arc<dyn PauseHandler>,
    tracked_artifact_paths: Vec<String>,
    auto_trust_workspace: bool,
}

impl RuntimeHost {
    /// Initialize a new [`RuntimeHost`] with a fresh `SessionTree` and exclusive
    /// advisory `.lock` file.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on lock contention, sandbox path errors, or I/O failure.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: String,
        workspace_root: &Path,
        session_path: PathBuf,
        registry: ExtensionRegistry,
        policy_gate: PolicyGate,
        runtime: RuntimeRegistry,
        credentials: CredentialStore,
        event_sink: Option<Arc<dyn EventSink>>,
        pause_handler: Arc<dyn PauseHandler>,
    ) -> Result<Self, RuntimeError> {
        let canon_ws = fs::canonicalize(workspace_root)?;
        let lock = SessionLockGuard::acquire(&session_path)?;
        let kernel = KernelRuntime::new(
            run_id.clone(),
            canon_ws.to_string_lossy().to_string(),
            current_epoch_ms(),
            registry,
            policy_gate,
        )?;
        let session_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&session_path)?;
        let redactor = credentials.redactor();
        let artifacts = ArtifactManager::new(&canon_ws)?;
        let blob_store = BlobStore::new(&canon_ws);

        let mut host = Self {
            run_id,
            workspace_root: canon_ws,
            kernel,
            runtime,
            credentials,
            artifacts,
            blob_store,
            session_path,
            session_file,
            _lock: lock,
            event_sink,
            pause_handler,
            tracked_artifact_paths: Vec::new(),
            auto_trust_workspace: false,
        };
        host.flush_kernel_outputs(&redactor)?;
        Ok(host)
    }

    /// Open an existing `SessionTree` JSONL file (repairing any truncated
    /// trailing line on disk) under an exclusive advisory `.lock` guard.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on lock contention, corrupt interior nodes, or I/O failure.
    #[allow(clippy::too_many_arguments)]
    pub fn open_session(
        run_id: String,
        workspace_root: &Path,
        session_path: PathBuf,
        registry: ExtensionRegistry,
        policy_gate: PolicyGate,
        runtime: RuntimeRegistry,
        credentials: CredentialStore,
        event_sink: Option<Arc<dyn EventSink>>,
        pause_handler: Arc<dyn PauseHandler>,
    ) -> Result<Self, RuntimeError> {
        let canon_ws = fs::canonicalize(workspace_root)?;
        let lock = SessionLockGuard::acquire(&session_path)?;
        let (tree, session_file) = session_io::load_and_repair_session_file(&session_path)?;
        let kernel = KernelRuntime::from_tree(
            run_id.clone(),
            tree,
            registry,
            policy_gate,
            current_epoch_ms(),
        );
        let artifacts = ArtifactManager::new(&canon_ws)?;
        let blob_store = BlobStore::new(&canon_ws);

        Ok(Self {
            run_id,
            workspace_root: canon_ws,
            kernel,
            runtime,
            credentials,
            artifacts,
            blob_store,
            session_path,
            session_file,
            _lock: lock,
            event_sink,
            pause_handler,
            tracked_artifact_paths: Vec::new(),
            auto_trust_workspace: false,
        })
    }

    /// Configure workspace-relative artifact paths snapshotted into `BlobStore`
    /// at the start of every Turn.
    pub fn set_tracked_artifact_paths(&mut self, paths: Vec<String>) {
        self.tracked_artifact_paths = paths;
    }

    /// Configure additional read-only directory roots outside `workspace_root`
    /// permitted for Turn-start artifact snapshotting and read-only tools.
    pub fn set_extra_read_roots(&mut self, roots: Vec<PathBuf>) {
        self.artifacts.set_extra_read_roots(roots);
    }

    /// Borrow the underlying [`KernelRuntime`].
    #[must_use]
    pub fn kernel(&self) -> &KernelRuntime {
        &self.kernel
    }

    /// Mutably borrow the underlying [`KernelRuntime`].
    pub fn kernel_mut(&mut self) -> &mut KernelRuntime {
        &mut self.kernel
    }

    /// Borrow the [`RuntimeRegistry`].
    #[must_use]
    pub fn runtime(&self) -> &RuntimeRegistry {
        &self.runtime
    }

    /// Mutably borrow the [`RuntimeRegistry`].
    pub fn runtime_mut(&mut self) -> &mut RuntimeRegistry {
        &mut self.runtime
    }

    /// Borrow the [`ArtifactManager`].
    #[must_use]
    pub fn artifacts(&self) -> &ArtifactManager {
        &self.artifacts
    }

    /// Borrow the [`BlobStore`].
    #[must_use]
    pub fn blob_store(&self) -> &BlobStore {
        &self.blob_store
    }

    /// Borrow the [`CredentialStore`].
    #[must_use]
    pub fn credentials(&self) -> &CredentialStore {
        &self.credentials
    }

    /// Mutably borrow the [`CredentialStore`] (e.g., for `/login` persistence).
    pub fn credentials_mut(&mut self) -> &mut CredentialStore {
        &mut self.credentials
    }

    /// Return the canonical workspace root path.
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Replace the active [`EventSink`].
    pub fn set_event_sink(&mut self, sink: Option<Arc<dyn EventSink>>) {
        self.event_sink = sink;
    }

    /// Replace the active [`PauseHandler`].
    pub fn set_pause_handler(&mut self, handler: Arc<dyn PauseHandler>) {
        self.pause_handler = handler;
    }

    /// Return the active session JSONL file path.
    #[must_use]
    pub fn session_path(&self) -> &Path {
        &self.session_path
    }

    /// Flush any pending `SessionTree` JSONL lines to disk and emit buffered
    /// [`RunEvent`]s through the secret redactor and active [`EventSink`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on disk write/sync failure.
    pub fn flush_pending(&mut self) -> Result<(), RuntimeError> {
        let redactor = self.credentials.redactor();
        self.flush_kernel_outputs(&redactor)
    }

    /// Fork the session from `from_node_id` (or active head) into `branch_id`,
    /// optionally writing the forked branch prefix to `new_file_path` (`--new-file`).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `from_node_id` is missing or file I/O fails.
    pub fn fork_session(
        &mut self,
        branch_id: String,
        from_node_id: Option<&str>,
        new_file_path: Option<&Path>,
    ) -> Result<String, RuntimeError> {
        let audit_id = self
            .kernel
            .fork_session(branch_id, from_node_id, current_epoch_ms())?;
        self.flush_pending()?;

        if let Some(dest) = new_file_path {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let slice = self.kernel.session_tree().slice_to(from_node_id)?;
            let redactor = self.credentials.redactor();
            let tmp = dest.with_extension(format!("jsonl.fork.{}", std::process::id()));
            {
                let mut f = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&tmp)?;
                for node in slice.nodes() {
                    let line = node.to_jsonl_line()?;
                    let safe = redactor.redact_str(&line);
                    writeln!(f, "{safe}")?;
                }
                f.flush()?;
                f.sync_data()?;
            }
            fs::rename(&tmp, dest)?;
        }

        Ok(audit_id)
    }

    /// Rewind the active conversation head to `target_node_id` without modifying
    /// any workspace files on disk.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `target_node_id` is missing or flushing fails.
    pub fn rewind_session(
        &mut self,
        branch_id: String,
        target_node_id: &str,
    ) -> Result<(), RuntimeError> {
        self.kernel
            .rewind_session(branch_id, target_node_id, current_epoch_ms())?;
        self.flush_pending()
    }

    /// Switch the active Phase mid-turn (`/mode <preset>`).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if no Turn is active or `preset_or_loop` is unresolved.
    pub fn switch_mode(
        &mut self,
        preset_or_loop: &str,
        handoff: Value,
        config_override: Option<LoopConfig>,
    ) -> Result<String, RuntimeError> {
        let phase_id = self.kernel.switch_mode(
            preset_or_loop,
            handoff,
            config_override,
            current_epoch_ms(),
        )?;
        self.flush_pending()?;
        Ok(phase_id)
    }

    /// Execute a registered in-process or out-of-process extension slash command.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if the command is not registered or execution fails.
    pub async fn execute_command(
        &mut self,
        command: &str,
        args: &str,
    ) -> Result<Value, RuntimeError> {
        if let Some(handler) = self.runtime.commands.get(command).cloned() {
            let out = handler.execute(args, &mut self.kernel).await?;
            self.flush_pending()?;
            return Ok(out);
        }

        let ext_owner = {
            let ipc = self.runtime.ext_ipc.lock().await;
            ipc.owner_of_command(command).map(str::to_string)
        };
        if let Some(ext_name) = ext_owner {
            let head_node = self
                .kernel
                .session_tree()
                .head_id()
                .and_then(|id| self.kernel.session_tree().get(id))
                .cloned();
            let args_val = Value::String(args.to_string());
            let resp = {
                let mut ipc = self.runtime.ext_ipc.lock().await;
                let worker =
                    ipc.worker_mut(&ext_name)
                        .ok_or_else(|| RuntimeError::IpcProtocol {
                            ext_name: ext_name.clone(),
                            message: "external command worker not running".to_string(),
                        })?;
                worker
                    .command_run(command, &args_val, head_node.as_ref())
                    .await?
            };
            if let Some(steer) = resp.steering {
                self.kernel.enqueue_steering(
                    SteeringKind::Steer,
                    steer,
                    SteeringSource::User,
                    current_epoch_ms(),
                )?;
            }
            self.flush_pending()?;
            return Ok(resp.output);
        }

        Err(RuntimeError::Kernel(KernelError::UnresolvedExtensionSlot {
            slot: "command".to_string(),
            name: command.to_string(),
        }))
    }

    /// Configure whether `.gestalt/gestalt.toml` is implicitly trusted (e.g.
    /// when `--trust-workspace` is passed on the CLI or `/trust` was run).
    pub fn set_auto_trust_workspace(&mut self, auto_trust: bool) {
        self.auto_trust_workspace = auto_trust;
    }

    /// Approve `<workspace>/.gestalt/gestalt.toml` in [`WorkspaceTrustStore`]
    /// and enable workspace trust for subsequent `/reload` calls.
    ///
    /// Returns `Ok(Some(sha256_digest))` when `.gestalt/gestalt.toml` exists,
    /// or `Ok(None)` when no workspace config file is present.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if reading or persisting the trust store fails.
    pub fn trust_workspace(&mut self) -> Result<Option<String>, RuntimeError> {
        let ws_toml = self.workspace_root.join(".gestalt").join("gestalt.toml");
        self.auto_trust_workspace = true;
        if !ws_toml.exists() {
            return Ok(None);
        }
        let raw_bytes = fs::read(&ws_toml)?;
        let mut trust_store = WorkspaceTrustStore::load_default()?;
        let digest = trust_store.trust_workspace(&self.workspace_root, &raw_bytes)?;
        Ok(Some(digest))
    }

    /// Start a fresh `SessionTree` file on disk (`/new`), preserving the active
    /// `ExtensionRegistry` and `PolicyGate`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on lock contention, file creation, or kernel initialization error.
    pub fn reset_new_session(&mut self, new_session_path: PathBuf) -> Result<(), RuntimeError> {
        let lock = SessionLockGuard::acquire(&new_session_path)?;
        if let Some(parent) = new_session_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let session_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&new_session_path)?;
        self.kernel.reset_session(
            self.run_id.clone(),
            self.workspace_root.to_string_lossy().to_string(),
            current_epoch_ms(),
        )?;
        self.session_path = new_session_path;
        self.session_file = session_file;
        self._lock = lock;
        let redactor = self.credentials.redactor();
        self.flush_kernel_outputs(&redactor)?;
        Ok(())
    }

    /// Switch the active session to an existing `SessionTree` JSONL file on disk,
    /// repairing any truncated trailing line and acquiring its advisory `.lock`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on lock contention, corrupt interior nodes, or I/O failure.
    pub fn switch_session(&mut self, session_path: PathBuf) -> Result<(), RuntimeError> {
        let lock = SessionLockGuard::acquire(&session_path)?;
        let (tree, session_file) = session_io::load_and_repair_session_file(&session_path)?;
        self.kernel.replace_tree(tree, current_epoch_ms());
        self.session_path = session_path;
        self.session_file = session_file;
        self._lock = lock;
        Ok(())
    }

    /// Hot-reload an out-of-process JSON-RPC or MCP extension between Turns/Phases (`/reload <ext>`).
    ///
    /// Dynamically re-reads `<workspace>/.gestalt/gestalt.toml` (enforcing
    /// [`WorkspaceTrustStore`] and monotonic [`PolicyGate`] checks) so newly
    /// authored `[extensions.<ext>]` entries can be loaded live without restarting.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if workspace trust is missing or shutdown, unload,
    /// respawn, or `extension/describe` / MCP initialization fails.
    pub async fn reload_external_extension(&mut self, ext_name: &str) -> Result<(), RuntimeError> {
        let ws_cfg = self.apply_verified_workspace_presets()?;
        let sandbox_policy = self.kernel.policy_gate().sandbox().clone();
        let redactor = self.credentials.redactor();

        if let Some(cfg) = ws_cfg.as_ref().and_then(|c| c.extensions.get(ext_name)) {
            if cfg.protocol == ExtensionProtocol::Mcp {
                return self
                    .reconnect_mcp_extension(ext_name, cfg, &sandbox_policy, redactor)
                    .await;
            }
            let mut ipc = self.runtime.ext_ipc.lock().await;
            ipc.upsert_config(ext_name.to_string(), cfg.clone());
        }

        let mut ipc = self.runtime.ext_ipc.lock().await;
        ipc.reload_extension(
            ext_name,
            self.workspace_root.clone(),
            sandbox_policy,
            self.runtime.sandbox_driver.clone(),
            redactor,
            self.kernel.registry_mut(),
        )
        .await?;
        Ok(())
    }

    /// Hot-reload all configured and newly discovered external extensions (`/reload`).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if workspace trust is missing or reloading any extension fails.
    pub async fn reload_all_external_extensions(&mut self) -> Result<Vec<String>, RuntimeError> {
        let ws_cfg = self.apply_verified_workspace_presets()?;
        let sandbox_policy = self.kernel.policy_gate().sandbox().clone();
        let redactor = self.credentials.redactor();
        let mut reloaded = Vec::new();

        if let Some(cfg) = &ws_cfg {
            for (name, ext_cfg) in &cfg.extensions {
                match ext_cfg.protocol {
                    ExtensionProtocol::Mcp => {
                        self.reconnect_mcp_extension(
                            name,
                            ext_cfg,
                            &sandbox_policy,
                            redactor.clone(),
                        )
                        .await?;
                        reloaded.push(name.clone());
                    }
                    ExtensionProtocol::Rpc => {
                        let mut ipc = self.runtime.ext_ipc.lock().await;
                        ipc.upsert_config(name.clone(), ext_cfg.clone());
                    }
                }
            }
        }

        let mut ipc = self.runtime.ext_ipc.lock().await;
        let names = ipc.configured_extensions();
        for ext_name in names {
            ipc.reload_extension(
                &ext_name,
                self.workspace_root.clone(),
                sandbox_policy.clone(),
                self.runtime.sandbox_driver.clone(),
                redactor.clone(),
                self.kernel.registry_mut(),
            )
            .await?;
            reloaded.push(ext_name);
        }
        Ok(reloaded)
    }

    async fn reconnect_mcp_extension(
        &mut self,
        name: &str,
        cfg: &crate::config::ExternalExtensionConfig,
        sandbox_policy: &SandboxPolicy,
        redactor: StreamRedactor,
    ) -> Result<(), RuntimeError> {
        self.runtime
            .connect_or_reload_mcp(
                name,
                cfg,
                &self.workspace_root,
                sandbox_policy,
                redactor,
                self.kernel.registry_mut(),
            )
            .await
    }

    fn apply_verified_workspace_presets(&mut self) -> Result<Option<GestaltConfig>, RuntimeError> {
        let ws_cfg = self.load_verified_workspace_config()?;
        if let Some(cfg) = &ws_cfg {
            for (id, preset_cfg) in &cfg.presets {
                let preset = preset_cfg.to_preset()?;
                self.kernel
                    .registry_mut()
                    .presets
                    .insert(id.clone(), preset);
            }
        }
        Ok(ws_cfg)
    }

    fn load_verified_workspace_config(&self) -> Result<Option<GestaltConfig>, RuntimeError> {
        let ws_toml = self.workspace_root.join(".gestalt").join("gestalt.toml");
        if !ws_toml.exists() {
            return Ok(None);
        }
        let raw_bytes = fs::read(&ws_toml)?;
        let trust_store = WorkspaceTrustStore::load_default()?;
        if !self.auto_trust_workspace
            && !trust_store.is_trusted(&self.workspace_root, &raw_bytes)?
        {
            return Err(RuntimeError::UntrustedWorkspace {
                workspace_root: self.workspace_root.to_string_lossy().to_string(),
                config_digest: sha256_hex(&raw_bytes),
            });
        }
        let raw_str = String::from_utf8(raw_bytes).map_err(|err| {
            RuntimeError::ArtifactError(format!("workspace gestalt.toml is not UTF-8: {err}"))
        })?;
        let ws_cfg = GestaltConfig::from_toml_str(&raw_str)?;
        let _ = ws_cfg.to_policy_override(
            &self.kernel.policy_gate().baseline_budget(),
            self.kernel.policy_gate().sandbox(),
        )?;
        Ok(Some(ws_cfg))
    }

    /// Revert a previously promoted [`Effect::Save`] stage via `.gestalt/ledger.jsonl`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `stage_id` is missing or blob restoration fails.
    pub fn rollback_stage(&self, stage_id: &str) -> Result<LedgerEntry, RuntimeError> {
        self.artifacts.rollback(stage_id, current_epoch_ms())
    }

    /// Build a [`TriggerContext`] bound to this host's workspace, `WorkspacePathGuard`,
    /// credentials, HTTP client, and secret redactor.
    #[must_use]
    pub fn trigger_context(&self) -> TriggerContext {
        let protected = &self.kernel.policy_gate().sandbox().protected_paths;
        let path_guard =
            WorkspacePathGuard::new(&self.workspace_root, protected).unwrap_or_else(|_| {
                WorkspacePathGuard::new(Path::new("."), protected).expect("fallback path guard")
            });
        TriggerContext {
            workspace_root: self.workspace_root.clone(),
            path_guard,
            credentials: self.credentials.clone(),
            http: self.runtime.http.clone(),
            redactor: self.credentials.redactor(),
        }
    }

    /// Take the single [`mpsc::UnboundedReceiver<TriggerEvent>`] for surface
    /// event-loop multiplexing.
    pub fn take_trigger_receiver(
        &self,
    ) -> impl Future<Output = Option<mpsc::UnboundedReceiver<TriggerEvent>>> + Send + 'static {
        let mgr = self.runtime.trigger_manager.clone();
        async move { mgr.lock().await.take_event_receiver() }
    }

    /// Start all `[triggers.<id>]` entries from [`ResolvedRuntimeConfig`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if a configured trigger kind is unregistered or invalid.
    pub fn start_configured_triggers(
        &self,
        resolved: &ResolvedRuntimeConfig,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        let triggers = resolved.triggers.clone();
        let handlers = self.runtime.trigger_handlers.clone();
        let mgr_arc = self.runtime.trigger_manager.clone();
        let ctx = self.trigger_context();
        async move {
            for (id, cfg) in triggers {
                let handler = handlers.get(&cfg.kind).cloned().ok_or_else(|| {
                    RuntimeError::Kernel(KernelError::UnresolvedExtensionSlot {
                        slot: "trigger".to_string(),
                        name: cfg.kind.clone(),
                    })
                })?;
                let instance = TriggerInstanceConfig {
                    id,
                    kind: cfg.kind,
                    enabled: cfg.enabled,
                    prompt: cfg.prompt,
                    preset: cfg.preset,
                    extra_decorators: cfg.extra_decorators,
                    config: cfg.config,
                };
                let mut mgr = mgr_arc.lock().await;
                mgr.start_trigger(&handler, instance, ctx.clone())?;
            }
            Ok(())
        }
    }

    /// Validate and start a dynamic trigger instance on this host.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if no handler is registered for `instance.kind`
    /// or handler validation/startup fails.
    pub fn start_trigger(
        &self,
        instance: TriggerInstanceConfig,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        let handler = self.runtime.trigger_handlers.get(&instance.kind).cloned();
        let ctx = self.trigger_context();
        let mgr_arc = self.runtime.trigger_manager.clone();
        async move {
            let handler = handler.ok_or_else(|| {
                RuntimeError::Kernel(KernelError::UnresolvedExtensionSlot {
                    slot: "trigger".to_string(),
                    name: instance.kind.clone(),
                })
            })?;
            let mut mgr = mgr_arc.lock().await;
            mgr.start_trigger(&handler, instance, ctx)
        }
    }

    /// Pause a running trigger by ID. Returns `false` if unknown.
    pub fn pause_trigger(&self, id: &str) -> impl Future<Output = bool> + Send + 'static {
        let mgr_arc = self.runtime.trigger_manager.clone();
        let id = id.to_string();
        async move { mgr_arc.lock().await.pause_trigger(&id) }
    }

    /// Resume a paused trigger by ID.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `id` or its handler is missing or startup fails.
    pub fn resume_trigger(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<(), RuntimeError>> + Send + 'static {
        let mgr_arc = self.runtime.trigger_manager.clone();
        let handlers = self.runtime.trigger_handlers.clone();
        let ctx = self.trigger_context();
        let id = id.to_string();
        async move {
            let kind = {
                let mgr = mgr_arc.lock().await;
                mgr.trigger_kind(&id)
                    .map(ToString::to_string)
                    .ok_or_else(|| RuntimeError::ArtifactError(format!("unknown trigger '{id}'")))?
            };
            let handler = handlers.get(&kind).cloned().ok_or_else(|| {
                RuntimeError::Kernel(KernelError::UnresolvedExtensionSlot {
                    slot: "trigger".to_string(),
                    name: kind,
                })
            })?;
            let mut mgr = mgr_arc.lock().await;
            mgr.resume_trigger(&id, &handler, ctx)?;
            Ok(())
        }
    }

    /// Stop and remove a trigger by ID. Returns `false` if unknown.
    pub fn remove_trigger(&self, id: &str) -> impl Future<Output = bool> + Send + 'static {
        let mgr_arc = self.runtime.trigger_manager.clone();
        let id = id.to_string();
        async move { mgr_arc.lock().await.remove_trigger(&id) }
    }

    /// Manually fire a registered trigger immediately.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `id` is unknown.
    pub fn fire_trigger_now(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<TriggerEvent, RuntimeError>> + Send + 'static {
        let mgr_arc = self.runtime.trigger_manager.clone();
        let id = id.to_string();
        async move { mgr_arc.lock().await.fire_now(&id, current_epoch_ms()) }
    }

    /// List status snapshots for all registered triggers.
    pub fn list_triggers(&self) -> impl Future<Output = Vec<TriggerStatus>> + Send + 'static {
        let mgr_arc = self.runtime.trigger_manager.clone();
        async move { mgr_arc.lock().await.list_status() }
    }

    /// Record an idle trigger firing (`custom:trigger:fired` node + `RunEvent::TriggerFired`)
    /// immediately before starting the triggered Turn.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if node persistence fails.
    pub async fn ingest_idle_trigger_event(
        &mut self,
        event: &TriggerEvent,
    ) -> Result<String, RuntimeError> {
        self.runtime
            .trigger_manager
            .lock()
            .await
            .record_fired(&event.trigger_id, event.timestamp_ms);
        let node_id = self.kernel.append_custom_node(
            "trigger:fired".to_string(),
            json!({
                "trigger_id": event.trigger_id,
                "kind": event.kind,
                "prompt": event.prompt,
                "preset": event.preset,
                "metadata": event.metadata,
                "queued_as_follow_up": false,
            }),
            event.timestamp_ms,
        )?;
        self.flush_pending()?;
        if let Some(ref sink) = self.event_sink {
            let mut ev = RunEvent::TriggerFired {
                trigger_id: event.trigger_id.clone(),
                kind: event.kind.clone(),
                turn_or_signal_id: node_id.clone(),
                queued_as_follow_up: false,
                payload: json!({
                    "prompt": event.prompt,
                    "preset": event.preset,
                    "metadata": event.metadata,
                }),
            };
            session_io::redact_run_event(&mut ev, &self.credentials.redactor());
            sink.on_event(&ev);
        }
        Ok(node_id)
    }

    async fn ingest_active_trigger_event(
        &mut self,
        event: TriggerEvent,
    ) -> Result<(), RuntimeError> {
        self.runtime
            .trigger_manager
            .lock()
            .await
            .record_fired(&event.trigger_id, event.timestamp_ms);
        let _ = self.kernel.append_custom_node(
            "trigger:fired".to_string(),
            json!({
                "trigger_id": event.trigger_id,
                "kind": event.kind,
                "prompt": event.prompt,
                "preset": event.preset,
                "metadata": event.metadata,
                "queued_as_follow_up": true,
            }),
            event.timestamp_ms,
        )?;
        let sig = self.kernel.enqueue_steering(
            SteeringKind::FollowUp,
            event.prompt.clone(),
            SteeringSource::Supervisor,
            event.timestamp_ms,
        )?;
        self.flush_pending()?;
        if let Some(ref sink) = self.event_sink {
            let mut ev = RunEvent::TriggerFired {
                trigger_id: event.trigger_id,
                kind: event.kind,
                turn_or_signal_id: sig.id,
                queued_as_follow_up: true,
                payload: json!({
                    "prompt": event.prompt,
                    "preset": event.preset,
                    "metadata": event.metadata,
                }),
            };
            session_io::redact_run_event(&mut ev, &self.credentials.redactor());
            sink.on_event(&ev);
        }
        Ok(())
    }

    /// Drive one complete user-facing Turn to completion or clean cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on unrecoverable kernel, lock, or pause-denial failures.
    pub async fn run_turn(
        &mut self,
        prompt: Value,
        preset_or_loop: &str,
        extra_decorators: &[String],
        cancel: CancellationToken,
    ) -> Result<TurnOutcome, RuntimeError> {
        self.run_turn_with_steering(prompt, preset_or_loop, extra_decorators, cancel, None)
            .await
    }

    /// Drive one Turn while accepting live intra-turn steering (`Steer`,
    /// `UrgentSteer`, `FollowUp`, `Abort`) over `steering_rx`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on unrecoverable kernel, lock, or pause-denial failures.
    pub async fn run_turn_with_steering(
        &mut self,
        prompt: Value,
        preset_or_loop: &str,
        extra_decorators: &[String],
        cancel: CancellationToken,
        steering_rx: Option<mpsc::UnboundedReceiver<LiveSteeringMessage>>,
    ) -> Result<TurnOutcome, RuntimeError> {
        let ctrl = match steering_rx {
            Some(rx) => ControlReceiver::Steering(rx),
            None => ControlReceiver::None,
        };
        self.run_turn_inner(prompt, preset_or_loop, extra_decorators, cancel, ctrl)
            .await
    }

    /// Drive one Turn while accepting live steering and mid-turn `SwitchMode`
    /// controls over `control_rx`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on unrecoverable kernel, lock, or pause-denial failures.
    pub async fn run_turn_with_controls(
        &mut self,
        prompt: Value,
        preset_or_loop: &str,
        extra_decorators: &[String],
        cancel: CancellationToken,
        control_rx: Option<mpsc::UnboundedReceiver<LiveTurnControl>>,
    ) -> Result<TurnOutcome, RuntimeError> {
        let ctrl = match control_rx {
            Some(rx) => ControlReceiver::Controls(rx),
            None => ControlReceiver::None,
        };
        self.run_turn_inner(prompt, preset_or_loop, extra_decorators, cancel, ctrl)
            .await
    }

    async fn run_turn_inner(
        &mut self,
        prompt: Value,
        preset_or_loop: &str,
        extra_decorators: &[String],
        cancel: CancellationToken,
        mut control_rx: ControlReceiver,
    ) -> Result<TurnOutcome, RuntimeError> {
        let artifacts_view = self
            .artifacts
            .snapshot_artifacts(&self.tracked_artifact_paths)?;
        let turn_id = self.kernel.start_turn(
            prompt,
            preset_or_loop,
            extra_decorators,
            artifacts_view,
            current_epoch_ms(),
        )?;
        let redactor = self.credentials.redactor();
        self.flush_kernel_outputs(&redactor)?;

        let mut response = DriverResponse::Poll;

        loop {
            self.kernel.set_now_ms(current_epoch_ms());
            // Drain any queued live steering or mode-switch controls before stepping.
            while let Some(ctrl) = control_rx.try_recv() {
                match ctrl {
                    LiveTurnControl::Steering(msg) => {
                        let _ = self.kernel.enqueue_steering(
                            msg.kind,
                            msg.content,
                            msg.source,
                            current_epoch_ms(),
                        );
                    }
                    LiveTurnControl::SwitchMode {
                        preset_or_loop: target_preset,
                        handoff,
                        config_override,
                    } => {
                        let _ = self.kernel.switch_mode(
                            &target_preset,
                            handoff,
                            config_override,
                            current_epoch_ms(),
                        );
                    }
                    LiveTurnControl::Trigger(ev) => {
                        let _ = self.ingest_active_trigger_event(ev).await;
                    }
                }
            }
            self.flush_kernel_outputs(&redactor)?;
            if cancel.is_cancelled() {
                let _ = self.kernel.enqueue_steering(
                    SteeringKind::Abort,
                    Value::String("cancelled".to_string()),
                    SteeringSource::User,
                    current_epoch_ms(),
                );
                self.flush_kernel_outputs(&redactor)?;
            }

            let directive = self.kernel.next_action(response)?;
            self.flush_kernel_outputs(&redactor)?;

            match directive {
                DriverDirective::TurnComplete {
                    turn_id: done_id,
                    output,
                    usage,
                } => {
                    return Ok(TurnOutcome::Completed {
                        turn_id: done_id,
                        output,
                        usage,
                    });
                }
                DriverDirective::TurnAborted {
                    turn_id: aborted_id,
                    reason,
                } => {
                    return Ok(TurnOutcome::Aborted {
                        turn_id: aborted_id,
                        reason,
                    });
                }
                DriverDirective::WaitForPause {
                    pause_id,
                    effect_id,
                    reason,
                    payload,
                } => {
                    let resolution = self
                        .pause_handler
                        .resolve_pause(&pause_id, &effect_id, reason, &payload, cancel.clone())
                        .await?;
                    response = DriverResponse::PauseResolution {
                        pause_id,
                        effect_id,
                        resolution,
                    };
                }
                DriverDirective::InvokeExternalBuilder {
                    ext_name,
                    builder_id,
                    session_slice,
                    artifacts,
                    caps,
                } => {
                    let req = ExternalBuilderRequest {
                        builder_id,
                        session_slice,
                        artifacts,
                        caps,
                    };
                    let mut ipc = self.runtime.ext_ipc.lock().await;
                    let worker =
                        ipc.worker_mut(&ext_name)
                            .ok_or_else(|| RuntimeError::IpcProtocol {
                                ext_name: ext_name.clone(),
                                message: "external builder worker not running".to_string(),
                            })?;
                    let model_input = worker.builder_build(&req, cancel.clone()).await?;
                    response = DriverResponse::ExternalBuilderOutput { model_input };
                }
                DriverDirective::InvokeExternalLoop {
                    ext_name,
                    loop_id: _,
                    phase_id,
                    turn_id: active_turn_id,
                    handoff,
                    config,
                    state_token,
                    results,
                } => {
                    let req = ExternalLoopStepRequest {
                        phase_id: phase_id.clone(),
                        turn_id: active_turn_id,
                        handoff,
                        config: config.clone(),
                        state_token: state_token.clone(),
                        results,
                    };
                    let mut out = {
                        let mut ipc = self.runtime.ext_ipc.lock().await;
                        let worker =
                            ipc.worker_mut(&ext_name)
                                .ok_or_else(|| RuntimeError::IpcProtocol {
                                    ext_name: ext_name.clone(),
                                    message: "external loop worker not running".to_string(),
                                })?;
                        worker.loop_step(&req, cancel.clone()).await?
                    };

                    // Apply any registered external decorators matching `extra_decorators`.
                    for dec_id in extra_decorators {
                        let dec_owner = {
                            let ipc = self.runtime.ext_ipc.lock().await;
                            ipc.owner_of_decorator(dec_id).map(str::to_string)
                        };
                        if let Some(owner) = dec_owner {
                            let dec_req = ExternalDecoratorStepRequest {
                                decorator_id: dec_id.clone(),
                                phase_id: phase_id.clone(),
                                inner_action: out.action.clone(),
                                config: config.clone(),
                                state_token: out.state_token.clone(),
                            };
                            let mut ipc = self.runtime.ext_ipc.lock().await;
                            if let Some(dec_worker) = ipc.worker_mut(&owner) {
                                out = dec_worker
                                    .decorator_wrap_step(&dec_req, cancel.clone())
                                    .await?;
                            }
                        }
                    }

                    response = DriverResponse::ExternalLoopOutput {
                        action: out.action,
                        state_token: out.state_token,
                    };
                }
                DriverDirective::VerifyEffect {
                    rule_id,
                    verifier,
                    pending,
                } => {
                    let verifier_result = self
                        .execute_verification_effect(&rule_id, &verifier, &pending, cancel.clone())
                        .await;
                    response = DriverResponse::VerificationResult {
                        rule_id,
                        pending_effect_id: pending.effect_id,
                        verifier_result,
                    };
                }
                DriverDirective::DispatchEffects { effects } => {
                    let batch_outcome = self
                        .execute_effect_batch(&turn_id, effects, cancel.clone(), &mut control_rx)
                        .await?;
                    match batch_outcome {
                        BatchOutcome::Completed(results) => {
                            response = DriverResponse::EffectResults(results);
                        }
                        BatchOutcome::Interrupted => {
                            self.flush_kernel_outputs(&redactor)?;
                            response = DriverResponse::Poll;
                        }
                    }
                }
            }
        }
    }

    /// Cleanly shut down external workers, background triggers, and prune clean temporary `Worktree`s.
    pub async fn shutdown(mut self) {
        {
            let mut mgr = self.runtime.trigger_manager.lock().await;
            mgr.shutdown_all();
        }
        {
            let mut ipc = self.runtime.ext_ipc.lock().await;
            ipc.shutdown_all().await;
        }
        for (_, client) in std::mem::take(&mut self.runtime.mcp_clients) {
            let mut guard = client.lock().await;
            guard.shutdown().await;
        }
        let _ = prune_clean_worktrees(&self.workspace_root).await;
    }

    fn flush_kernel_outputs(&mut self, redactor: &StreamRedactor) -> Result<(), RuntimeError> {
        let lines = self.kernel.drain_pending_jsonl_lines();
        session_io::flush_jsonl_lines(&mut self.session_file, lines, redactor)?;
        let events = self.kernel.drain_events();
        if let Some(ref sink) = self.event_sink {
            for mut ev in events {
                session_io::redact_run_event(&mut ev, redactor);
                sink.on_event(&ev);
            }
        }
        Ok(())
    }

    fn build_dispatch_context(
        &self,
        turn_id: &str,
        phase_id: &str,
        env: &EffectEnvelope,
        cancel: CancellationToken,
    ) -> dispatch::EffectDispatchContext {
        let tool_spec = match &env.effect {
            Effect::Tool { name, .. } => self.kernel.registry().tools.get(name).cloned(),
            _ => None,
        };
        let tool_handler = match &env.effect {
            Effect::Tool { name, .. } => self.runtime.tools.get(name).cloned(),
            _ => None,
        };
        let mcp_client = match &env.effect {
            Effect::Tool { name, .. } => self
                .runtime
                .mcp_tool_owners
                .get(name)
                .and_then(|owner| self.runtime.mcp_clients.get(owner))
                .cloned(),
            _ => None,
        };
        let provider_resolved = match &env.effect {
            Effect::Model { target, .. } => Some(self.runtime.providers.resolve_target(target)),
            _ => None,
        };
        dispatch::EffectDispatchContext {
            workspace_root: self.workspace_root.clone(),
            turn_id: turn_id.to_string(),
            phase_id: phase_id.to_string(),
            sandbox_policy: self.kernel.policy_gate().sandbox().clone(),
            sandbox_driver: self.runtime.sandbox_driver.clone(),
            blob_store: self.blob_store.clone(),
            artifacts: self.artifacts.clone(),
            credentials: self.credentials.clone(),
            http: self.runtime.http.clone(),
            redactor: self.credentials.redactor(),
            event_sink: self.event_sink.clone(),
            tool_spec,
            tool_handler,
            mcp_client,
            ext_ipc: self.runtime.ext_ipc.clone(),
            provider_resolved,
            cancel,
        }
    }

    async fn execute_verification_effect(
        &mut self,
        rule_id: &str,
        verifier: &EffectEnvelope,
        pending: &EffectEnvelope,
        cancel: CancellationToken,
    ) -> StepResult {
        let ext_owner = {
            let ipc = self.runtime.ext_ipc.lock().await;
            ipc.owner_of_policy(rule_id).map(str::to_string)
        };
        if let Some(ext_name) = ext_owner {
            let slice_nodes = self
                .kernel
                .session_tree()
                .slice_to(None)
                .map(|s| s.nodes().to_vec())
                .unwrap_or_default();
            let decision = {
                let mut ipc = self.runtime.ext_ipc.lock().await;
                if let Some(worker) = ipc.worker_mut(&ext_name) {
                    worker.policy_evaluate(pending, &slice_nodes).await
                } else {
                    PolicyDecision::Deny {
                        reason: format!("external policy worker '{ext_name}' missing"),
                    }
                }
            };
            return match decision {
                PolicyDecision::Allow => StepResult::Model {
                    effect_id: verifier.effect_id.clone(),
                    kind: ModelKind::Decision,
                    output: json!({ "decision": "allow" }),
                    usage: Usage::default(),
                },
                PolicyDecision::Deny { reason }
                | PolicyDecision::RequiresVerification {
                    rule_id: reason, ..
                } => StepResult::Denied {
                    effect_id: verifier.effect_id.clone(),
                    reason,
                },
            };
        }

        let ctx = self.build_dispatch_context("", "", verifier, cancel);
        dispatch::execute_single_effect(verifier, ctx).await
    }

    async fn execute_effect_batch(
        &mut self,
        turn_id: &str,
        effects: Vec<EffectEnvelope>,
        cancel: CancellationToken,
        control_rx: &mut ControlReceiver,
    ) -> Result<BatchOutcome, RuntimeError> {
        let phase_id = self
            .kernel
            .session_tree()
            .head_id()
            .and_then(|id| self.kernel.session_tree().get(id))
            .and_then(|n| n.phase_id.clone())
            .unwrap_or_default();

        let step_cancel = CancellationToken::new();
        let mut results_by_id: BTreeMap<String, StepResult> = BTreeMap::new();
        let mut join_set: JoinSet<(String, StepResult)> = JoinSet::new();

        for env in effects {
            let eff_id = env.effect_id.clone();
            if let Effect::Spawn {
                loop_id,
                input,
                budget,
                depth_limit: _,
                workspace_mode,
                config,
            } = env.effect
            {
                let child_registry = dispatch::prepare_child_registry(
                    self.kernel.registry(),
                    &loop_id,
                    config.as_ref(),
                )?;
                let spec = dispatch::SpawnTaskSpec {
                    effect_id: eff_id.clone(),
                    parent_run_id: self.run_id.clone(),
                    parent_ws: self.workspace_root.clone(),
                    parent_sandbox: self.kernel.policy_gate().sandbox().clone(),
                    parent_blobs: self.blob_store.clone(),
                    parent_creds: self.credentials.clone(),
                    parent_sink: self.event_sink.clone(),
                    child_runtime: self.runtime.clone_for_spawn(),
                    child_registry,
                    loop_id,
                    input,
                    budget,
                    child_depth: 1,
                    workspace_mode,
                    cancel: step_cancel.clone(),
                };
                join_set.spawn(async move {
                    let sr = dispatch::execute_spawn_effect(spec).await;
                    (eff_id, sr)
                });
            } else {
                let ctx =
                    self.build_dispatch_context(turn_id, &phase_id, &env, step_cancel.clone());
                join_set.spawn(async move {
                    let sr = dispatch::execute_single_effect(&env, ctx).await;
                    (eff_id, sr)
                });
            }
        }

        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    step_cancel.cancel();
                    let _ = tokio::time::timeout(Duration::from_millis(50), async {
                        while join_set.join_next().await.is_some() {}
                    }).await;
                    join_set.abort_all();
                    let _ = self.kernel.enqueue_steering(
                        SteeringKind::Abort,
                        Value::String("cancelled mid-step".to_string()),
                        SteeringSource::User,
                        current_epoch_ms(),
                    );
                    return Ok(BatchOutcome::Interrupted);
                }
                Some(ctrl) = control_rx.recv() => {
                    match ctrl {
                        LiveTurnControl::Steering(msg) => {
                            let is_preemptive = matches!(msg.kind, SteeringKind::Abort | SteeringKind::UrgentSteer);
                            let _ = self.kernel.enqueue_steering(
                                msg.kind,
                                msg.content,
                                msg.source,
                                current_epoch_ms(),
                            );
                            if is_preemptive {
                                step_cancel.cancel();
                                let _ = tokio::time::timeout(Duration::from_millis(50), async {
                                    while join_set.join_next().await.is_some() {}
                                }).await;
                                join_set.abort_all();
                                return Ok(BatchOutcome::Interrupted);
                            }
                        }
                        LiveTurnControl::SwitchMode {
                            preset_or_loop,
                            handoff,
                            config_override,
                        } => {
                            let _ = self.kernel.switch_mode(
                                &preset_or_loop,
                                handoff,
                                config_override,
                                current_epoch_ms(),
                            );
                        }
                        LiveTurnControl::Trigger(ev) => {
                            let _ = self.ingest_active_trigger_event(ev).await;
                        }
                    }
                }
                next_res = join_set.join_next() => {
                    match next_res {
                        Some(Ok((id, sr))) => {
                            results_by_id.insert(id, sr);
                        }
                        Some(Err(join_err)) => {
                            tracing::error!("effect task panicked/aborted: {join_err}");
                        }
                        None => break,
                    }
                }
            }
        }

        Ok(BatchOutcome::Completed(
            results_by_id.into_values().collect(),
        ))
    }
}

enum ControlReceiver {
    None,
    Steering(mpsc::UnboundedReceiver<LiveSteeringMessage>),
    Controls(mpsc::UnboundedReceiver<LiveTurnControl>),
}

impl ControlReceiver {
    fn try_recv(&mut self) -> Option<LiveTurnControl> {
        match self {
            Self::None => None,
            Self::Steering(rx) => rx.try_recv().ok().map(LiveTurnControl::Steering),
            Self::Controls(rx) => rx.try_recv().ok(),
        }
    }

    async fn recv(&mut self) -> Option<LiveTurnControl> {
        match self {
            Self::None => std::future::pending().await,
            Self::Steering(rx) => rx.recv().await.map(LiveTurnControl::Steering),
            Self::Controls(rx) => rx.recv().await,
        }
    }
}

enum BatchOutcome {
    Completed(Vec<StepResult>),
    Interrupted,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::UnsandboxedFallbackDriver;
    use gestalt_kernel::{
        AgentLoop, AgentLoopFactory, BudgetCap, ExtensionRegistry, LoopAction, LoopContext,
    };

    struct PingCmd;
    impl CommandHandler for PingCmd {
        fn execute<'a>(
            &'a self,
            args: &'a str,
            kernel: &'a mut KernelRuntime,
        ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
            Box::pin(async move {
                let id = kernel.append_custom_node(
                    "test:ping".into(),
                    json!({ "args": args }),
                    current_epoch_ms(),
                )?;
                Ok(json!({ "node_id": id, "pong": args }))
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
        fn create(
            &self,
            _config: &LoopConfig,
            _registry: &ExtensionRegistry,
        ) -> Box<dyn AgentLoop> {
            Box::new(DoneLoop)
        }
    }

    #[tokio::test]
    async fn host_fork_new_file_rewind_and_execute_command_work() {
        let dir =
            std::env::temp_dir().join(format!("gestalt_rt_host_seams_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");

        let mut reg = ExtensionRegistry::new();
        reg.loops.insert("done".into(), Box::new(DoneFactory));
        let gate = PolicyGate::new(BudgetCap::default(), None, SandboxPolicy::default());
        let mut rt = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
        rt.commands.insert("ping".into(), Arc::new(PingCmd));

        let session_path = dir.join(".gestalt/sessions/main.jsonl");
        let mut host = RuntimeHost::new(
            "r_seam".into(),
            &dir,
            session_path,
            reg,
            gate,
            rt,
            CredentialStore::default(),
            None,
            Arc::new(HeadlessPauseHandler::default()),
        )
        .expect("host");

        let cmd_out = host.execute_command("ping", "hello").await.expect("cmd");
        assert_eq!(cmd_out["pong"], "hello");
        let ping_node = cmd_out["node_id"].as_str().expect("node id").to_string();

        let fork_file = dir.join(".gestalt/sessions/forked.jsonl");
        host.fork_session("branch_1".into(), Some(&ping_node), Some(&fork_file))
            .expect("fork");
        assert!(fork_file.exists());
        let fork_raw = fs::read_to_string(&fork_file).expect("read fork");
        let (fork_tree, warn) =
            gestalt_kernel::SessionTree::from_jsonl_str(&fork_raw).expect("parse");
        assert!(warn.is_none());
        assert!(!fork_tree.all_nodes().is_empty());

        host.rewind_session("main".into(), &ping_node)
            .expect("rewind");
        let missing = host.execute_command("missing_cmd", "").await;
        assert!(matches!(
            missing,
            Err(RuntimeError::Kernel(
                KernelError::UnresolvedExtensionSlot { .. }
            ))
        ));

        // Create a second session file with a truncated trailing line, then switch back to main.
        let s2_path = dir.join(".gestalt/sessions/s2.jsonl");
        let _ = fs::copy(host.session_path(), &s2_path).expect("copy");
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(&s2_path)
                .expect("open");
            writeln!(f, "{{\"id\":\"partial\"").expect("write truncated");
        }
        host.switch_session(s2_path.clone()).expect("switch s2");
        assert_eq!(host.session_path(), s2_path.as_path());
        assert!(host.kernel().session_tree().get(&ping_node).is_some());

        let _ = fs::remove_dir_all(&dir);
    }
}
