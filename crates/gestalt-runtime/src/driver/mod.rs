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
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use gestalt_kernel::{
    DriverDirective, DriverResponse, Effect, EffectEnvelope, ExtensionRegistry, KernelRuntime,
    ModelKind, PauseReason, PolicyDecision, PolicyGate, PolicyRule, RunEvent, SandboxPolicy,
    SessionSlice, SteeringKind, SteeringSource, StepResult, Usage,
};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinSet;

use crate::artifacts::{ArtifactManager, LedgerEntry};
use crate::blobs::BlobStore;
use crate::config::{ExtensionProtocol, ResolvedRuntimeConfig};
use crate::credentials::{CredentialStore, StreamRedactor};
use crate::ext_ipc::{
    ExtIpcHost, ExternalBuilderRequest, ExternalDecoratorStepRequest, ExternalLoopStepRequest,
};
use crate::mcp::McpClient;
use crate::provider::{ProviderRegistry, SharedHttpClient, StreamDeltaFn};
use crate::sandbox::{SandboxDriver, WorkspacePathGuard};
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
                    let mut client = McpClient::connect(
                        name.clone(),
                        ext_cfg,
                        workspace_root,
                        &sandbox_policy,
                        &self.sandbox_driver,
                        redactor.clone(),
                    )
                    .await?;
                    let tool_names = client.register_tools(registry).await?;
                    for t in tool_names {
                        self.mcp_tool_owners.insert(t, name.clone());
                    }
                    self.mcp_clients
                        .insert(name.clone(), Arc::new(Mutex::new(client)));
                }
            }
        }
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
        })
    }

    /// Configure workspace-relative artifact paths snapshotted into `BlobStore`
    /// at the start of every Turn.
    pub fn set_tracked_artifact_paths(&mut self, paths: Vec<String>) {
        self.tracked_artifact_paths = paths;
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

    /// Return the active session JSONL file path.
    #[must_use]
    pub fn session_path(&self) -> &Path {
        &self.session_path
    }

    /// Hot-reload an out-of-process JSON-RPC extension between Turns/Phases (`/reload <ext>`).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if shutdown, unload, respawn, or `extension/describe` fails.
    pub async fn reload_external_extension(&mut self, ext_name: &str) -> Result<(), RuntimeError> {
        let sandbox_policy = self.kernel.policy_gate().sandbox().clone();
        let redactor = self.credentials.redactor();
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

    /// Revert a previously promoted [`Effect::Save`] stage via `.gestalt/ledger.jsonl`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `stage_id` is missing or blob restoration fails.
    pub fn rollback_stage(&self, stage_id: &str) -> Result<LedgerEntry, RuntimeError> {
        self.artifacts.rollback(stage_id, current_epoch_ms())
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
        mut steering_rx: Option<mpsc::UnboundedReceiver<LiveSteeringMessage>>,
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
            // Drain any queued live steering messages before stepping.
            if let Some(ref mut rx) = steering_rx {
                while let Ok(msg) = rx.try_recv() {
                    let _ = self.kernel.enqueue_steering(
                        msg.kind,
                        msg.content,
                        msg.source,
                        current_epoch_ms(),
                    );
                }
                self.flush_kernel_outputs(&redactor)?;
            }
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
                        .execute_effect_batch(
                            &turn_id,
                            effects,
                            cancel.clone(),
                            steering_rx.as_mut(),
                        )
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

    /// Cleanly shut down external workers and prune clean temporary `Worktree`s.
    pub async fn shutdown(mut self) {
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
        mut steering_rx: Option<&mut mpsc::UnboundedReceiver<LiveSteeringMessage>>,
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
            let steering_fut = async {
                match steering_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };

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
                Some(msg) = steering_fut => {
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

enum BatchOutcome {
    Completed(Vec<StepResult>),
    Interrupted,
}
