#![forbid(unsafe_code)]
//! Effect dispatch helpers: standalone `Model`/`Tool`/`Save` execution,
//! `validate_tool_args` schema recovery, stateful streaming delta redaction,
//! 32 KB blob spilling, and child `Effect::Spawn` execution.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};

use gestalt_kernel::{
    AgentLoop, AgentLoopFactory, ArtifactStoreView, BudgetCap, ContextBuilder, Effect,
    EffectEnvelope, ExtensionRegistry, LoopConfig, ModelCapabilities, PolicyGate, PolicyOverride,
    RunEvent, SandboxPolicy, SaveMode, SessionSlice, StepResult, ToolSpec, WorkspaceMode,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::artifacts::ArtifactManager;
use crate::blobs::BlobStore;
use crate::credentials::{CredentialStore, StreamRedactor};
use crate::driver::session_io::current_epoch_ms;
use crate::driver::worktree::{capture_worktree_diff_blob, create_worktree};
use crate::driver::{
    EventSink, HeadlessPauseHandler, RuntimeHost, RuntimeRegistry, ToolExecutionContext,
    ToolHandler, TurnOutcome,
};
use crate::ext_ipc::ExtIpcHost;
use crate::mcp::McpClient;
use crate::provider::{ModelRequest, ProviderContext, ResolvedProviderTarget, SharedHttpClient};
use crate::sandbox::{SandboxDriver, WorkspacePathGuard};
use crate::{CancellationToken, ProviderErrorKind, RuntimeError};

/// Self-contained dispatch snapshot for executing a single `Model`, `Tool`, or
/// `Save` effect inside a `JoinSet` task.
pub(crate) struct EffectDispatchContext {
    pub workspace_root: PathBuf,
    pub turn_id: String,
    pub phase_id: String,
    pub sandbox_policy: SandboxPolicy,
    pub sandbox_driver: Arc<dyn SandboxDriver>,
    pub blob_store: BlobStore,
    pub artifacts: ArtifactManager,
    pub credentials: CredentialStore,
    pub http: SharedHttpClient,
    pub redactor: StreamRedactor,
    pub event_sink: Option<Arc<dyn EventSink>>,
    pub tool_spec: Option<ToolSpec>,
    pub tool_handler: Option<Arc<dyn ToolHandler>>,
    pub mcp_client: Option<Arc<Mutex<McpClient>>>,
    pub ext_ipc: Arc<Mutex<ExtIpcHost>>,
    pub provider_resolved: Option<Result<ResolvedProviderTarget, RuntimeError>>,
    pub cancel: CancellationToken,
}

/// Build a single stateful redacting callback for `StreamDelta` emission.
///
/// String payloads pass through a shared [`StreamRedactor::push_chunk`] window
/// so secrets split across consecutive LLM SSE token deltas never leak.
pub(crate) fn make_delta_callback(
    sink: Option<Arc<dyn EventSink>>,
    redactor: StreamRedactor,
    turn_id: String,
    phase_id: String,
    effect_id: String,
) -> impl Fn(&str, Value) + Send + Sync {
    let stream_redactor = Arc::new(StdMutex::new(redactor));
    move |channel: &str, mut payload: Value| {
        let Some(ref s) = sink else {
            return;
        };
        if let Ok(mut guard) = stream_redactor.lock() {
            match &mut payload {
                Value::String(text) => {
                    let safe = guard.push_chunk(text);
                    if safe.is_empty() {
                        return;
                    }
                    *text = safe;
                }
                other => guard.redact_json(other),
            }
        }
        s.on_event(&RunEvent::StreamDelta {
            turn_id: turn_id.clone(),
            phase_id: phase_id.clone(),
            effect_id: effect_id.clone(),
            channel: channel.to_string(),
            payload,
        });
    }
}

/// Execute a single `Model`, `Tool`, or `Save` effect envelope.
pub(crate) async fn execute_single_effect(
    env: &EffectEnvelope,
    ctx: EffectDispatchContext,
) -> StepResult {
    match &env.effect {
        Effect::Model {
            target: _,
            kind,
            input,
            schema,
        } => {
            let Some(resolved) = ctx.provider_resolved else {
                return StepResult::Error {
                    effect_id: env.effect_id.clone(),
                    message: format!(
                        "[{}] missing provider resolution",
                        ProviderErrorKind::Fatal.tag()
                    ),
                    retryable: false,
                };
            };
            let target = match resolved {
                Ok(v) => v,
                Err(err) => {
                    return StepResult::Error {
                        effect_id: env.effect_id.clone(),
                        message: format!("[{}] {err}", ProviderErrorKind::Fatal.tag()),
                        retryable: false,
                    };
                }
            };
            let req = ModelRequest {
                effect_id: env.effect_id.clone(),
                turn_id: ctx.turn_id.clone(),
                phase_id: ctx.phase_id.clone(),
                provider_id: target.provider.id().to_string(),
                model: target.model,
                kind: *kind,
                input: input.clone(),
                schema: schema.clone(),
                config: target.config,
            };
            let provider_ctx = ProviderContext {
                credentials: &ctx.credentials,
                http: &ctx.http,
                blobs: &ctx.blob_store,
                redactor: &ctx.redactor,
                cancel: ctx.cancel.clone(),
            };
            let on_delta = make_delta_callback(
                ctx.event_sink.clone(),
                ctx.redactor.clone(),
                ctx.turn_id.clone(),
                ctx.phase_id.clone(),
                env.effect_id.clone(),
            );

            match target.provider.invoke(&req, &provider_ctx, &on_delta).await {
                Ok(mut resp) => {
                    ctx.redactor.redact_json(&mut resp.output);
                    StepResult::Model {
                        effect_id: env.effect_id.clone(),
                        kind: resp.kind,
                        output: resp.output,
                        usage: resp.usage,
                    }
                }
                Err(RuntimeError::ProviderError { kind, message }) => StepResult::Error {
                    effect_id: env.effect_id.clone(),
                    message: format!("[{}] {}", kind.tag(), ctx.redactor.redact_str(&message)),
                    retryable: kind == ProviderErrorKind::Retryable,
                },
                Err(err) => StepResult::Error {
                    effect_id: env.effect_id.clone(),
                    message: format!(
                        "[{}] {}",
                        ProviderErrorKind::Fatal.tag(),
                        ctx.redactor.redact_str(&err.to_string())
                    ),
                    retryable: false,
                },
            }
        }
        Effect::Tool { name, args, .. } => execute_tool_effect(env, name, args, ctx).await,
        Effect::Save {
            artifact,
            patch,
            mode,
        } => {
            let stage_id = format!("stg_{}", env.effect_id);
            let now = current_epoch_ms();
            let res = match mode {
                SaveMode::Stage => {
                    ctx.artifacts
                        .stage_save(&env.effect_id, &stage_id, artifact, patch, now)
                }
                SaveMode::Promote => {
                    ctx.artifacts
                        .promote_save(&env.effect_id, &stage_id, artifact, patch, now)
                }
            };
            match res {
                Ok(sr) => sr,
                Err(RuntimeError::SandboxViolation(reason)) => StepResult::Denied {
                    effect_id: env.effect_id.clone(),
                    reason,
                },
                Err(err) => StepResult::Error {
                    effect_id: env.effect_id.clone(),
                    message: err.to_string(),
                    retryable: false,
                },
            }
        }
        Effect::Spawn { .. } | Effect::Switch { .. } | Effect::Pause { .. } => StepResult::Error {
            effect_id: env.effect_id.clone(),
            message: "unexpected control effect in standalone dispatcher".to_string(),
            retryable: false,
        },
    }
}

async fn execute_tool_effect(
    env: &EffectEnvelope,
    name: &str,
    args: &Value,
    ctx: EffectDispatchContext,
) -> StepResult {
    let Some(spec) = ctx.tool_spec else {
        return StepResult::Tool {
            effect_id: env.effect_id.clone(),
            output: Value::String(format!("validation error: unknown tool '{name}'")),
            is_error: true,
            blob_digest: None,
        };
    };

    if let Err(validation_err) = validate_tool_args(&spec.parameters_schema, args) {
        return StepResult::Tool {
            effect_id: env.effect_id.clone(),
            output: Value::String(format!(
                "validation error for tool '{name}': {validation_err}"
            )),
            is_error: true,
            blob_digest: None,
        };
    }

    let on_delta = make_delta_callback(
        ctx.event_sink.clone(),
        ctx.redactor.clone(),
        ctx.turn_id.clone(),
        ctx.phase_id.clone(),
        env.effect_id.clone(),
    );

    if let Some(handler) = ctx.tool_handler {
        let path_guard =
            match WorkspacePathGuard::new(&ctx.workspace_root, &ctx.sandbox_policy.protected_paths)
            {
                Ok(g) => g,
                Err(err) => {
                    return StepResult::Error {
                        effect_id: env.effect_id.clone(),
                        message: err.to_string(),
                        retryable: false,
                    };
                }
            };
        let tool_ctx = ToolExecutionContext {
            workspace_root: ctx.workspace_root.clone(),
            cwd: ctx.workspace_root.clone(),
            turn_id: ctx.turn_id.clone(),
            phase_id: ctx.phase_id.clone(),
            effect_id: env.effect_id.clone(),
            sandbox_policy: ctx.sandbox_policy.clone(),
            path_guard,
            sandbox_driver: ctx.sandbox_driver.clone(),
            blob_store: ctx.blob_store.clone(),
            cancel: ctx.cancel.clone(),
        };

        return match handler.execute(args, &tool_ctx, &on_delta).await {
            Ok(mut outcome) => {
                ctx.redactor.redact_json(&mut outcome.output);
                spill_tool_output(
                    &env.effect_id,
                    outcome.output,
                    outcome.is_error,
                    &ctx.blob_store,
                )
            }
            Err(RuntimeError::SandboxViolation(msg)) => StepResult::Denied {
                effect_id: env.effect_id.clone(),
                reason: msg,
            },
            Err(err) => StepResult::Tool {
                effect_id: env.effect_id.clone(),
                output: Value::String(ctx.redactor.redact_str(&err.to_string())),
                is_error: true,
                blob_digest: None,
            },
        };
    }

    if let Some(mcp) = ctx.mcp_client {
        let mut guard = mcp.lock().await;
        return match guard
            .call_tool(name, args, &env.effect_id, &ctx.blob_store, ctx.cancel)
            .await
        {
            Ok(sr) => sr,
            Err(err) => StepResult::Error {
                effect_id: env.effect_id.clone(),
                message: ctx.redactor.redact_str(&err.to_string()),
                retryable: false,
            },
        };
    }

    let mut ipc = ctx.ext_ipc.lock().await;
    let owner = ipc.owner_of_tool(name).map(str::to_string);
    if let Some(ext_name) = owner
        && let Some(worker) = ipc.worker_mut(&ext_name)
    {
        return match worker
            .tool_call(
                name,
                args,
                &ctx.workspace_root,
                ctx.blob_store.root(),
                &on_delta,
                ctx.cancel,
            )
            .await
        {
            Ok(StepResult::Tool {
                mut output,
                is_error,
                ..
            }) => {
                ctx.redactor.redact_json(&mut output);
                spill_tool_output(&env.effect_id, output, is_error, &ctx.blob_store)
            }
            Ok(other) => other,
            Err(err) => StepResult::Error {
                effect_id: env.effect_id.clone(),
                message: ctx.redactor.redact_str(&err.to_string()),
                retryable: false,
            },
        };
    }

    StepResult::Error {
        effect_id: env.effect_id.clone(),
        message: format!("no runtime handler bound for tool '{name}'"),
        retryable: false,
    }
}

/// One-shot `AgentLoopFactory` wrapping an already-instantiated `Box<dyn AgentLoop>`
/// so a parent `RuntimeHost` can execute an in-process child subagent inside
/// `Effect::Spawn`.
struct PrebuiltLoopFactory {
    id: String,
    digest: String,
    inner: StdMutex<Option<Box<dyn AgentLoop>>>,
}

impl AgentLoopFactory for PrebuiltLoopFactory {
    fn id(&self) -> &str {
        &self.id
    }
    fn digest(&self) -> &str {
        &self.digest
    }
    fn create(&self, _config: &LoopConfig, _registry: &ExtensionRegistry) -> Box<dyn AgentLoop> {
        if let Ok(mut guard) = self.inner.lock()
            && let Some(lp) = guard.take()
        {
            return lp;
        }
        Box::new(ExhaustedSpawnLoop)
    }
}

struct ExhaustedSpawnLoop;
impl AgentLoop for ExhaustedSpawnLoop {
    fn id(&self) -> &str {
        "exhausted_spawn"
    }
    fn digest(&self) -> &str {
        "es0"
    }
    fn step(
        &mut self,
        _ctx: &mut dyn gestalt_kernel::LoopContext,
        _r: Option<Vec<StepResult>>,
    ) -> gestalt_kernel::LoopAction {
        gestalt_kernel::LoopAction::Done(Value::Null)
    }
}

struct NullContextBuilder;
impl ContextBuilder for NullContextBuilder {
    fn id(&self) -> &str {
        "default"
    }
    fn digest(&self) -> &str {
        "null_v1"
    }
    fn build(
        &self,
        _session: &SessionSlice,
        _artifacts: &ArtifactStoreView,
        _caps: &ModelCapabilities,
    ) -> Value {
        Value::Null
    }
}

/// Parameters needed to execute a real isolated child subagent for `Effect::Spawn`.
pub(crate) struct SpawnTaskSpec {
    pub effect_id: String,
    pub parent_run_id: String,
    pub parent_ws: PathBuf,
    pub parent_sandbox: SandboxPolicy,
    pub parent_blobs: BlobStore,
    pub parent_creds: CredentialStore,
    pub parent_sink: Option<Arc<dyn EventSink>>,
    pub child_runtime: RuntimeRegistry,
    pub child_registry: ExtensionRegistry,
    pub loop_id: String,
    pub input: Value,
    pub budget: BudgetCap,
    pub child_depth: u8,
    pub workspace_mode: WorkspaceMode,
    pub cancel: CancellationToken,
}

/// Build a child [`ExtensionRegistry`] carrying a pre-instantiated loop (if
/// in-process) plus all declarative external/tool/preset descriptors.
pub(crate) fn prepare_child_registry(
    parent_reg: &ExtensionRegistry,
    loop_id: &str,
    config_override: Option<&LoopConfig>,
) -> Result<ExtensionRegistry, RuntimeError> {
    let mut child_reg = ExtensionRegistry::new();
    child_reg.tools = parent_reg.tools.clone();
    child_reg.commands = parent_reg.commands.clone();
    child_reg.ui = parent_reg.ui.clone();
    child_reg.external_loops = parent_reg.external_loops.clone();
    child_reg.external_builders = parent_reg.external_builders.clone();
    child_reg.external_decorators = parent_reg.external_decorators.clone();
    child_reg.external_policies = parent_reg.external_policies.clone();
    child_reg
        .builders
        .insert("default".to_string(), Box::new(NullContextBuilder));

    if let Ok(stack) = parent_reg.resolve_phase_stack(loop_id, &[], config_override)
        && let Some(lp) = stack.in_process_loop
    {
        child_reg.loops.insert(
            loop_id.to_string(),
            Box::new(PrebuiltLoopFactory {
                id: stack.base_loop_id,
                digest: stack.loop_digest,
                inner: StdMutex::new(Some(lp)),
            }),
        );
    }
    Ok(child_reg)
}

/// Execute an isolated child subagent Turn for `Effect::Spawn`, honoring
/// Executes a spawned child turn in isolation with its own `RuntimeHost`,
/// `WorkspaceMode` (`ReadOnly`, `Shared`, `Worktree`), `budget`, and `child_depth`.
///
/// Returns a boxed `Send` future to break the mutually recursive `async fn`
/// type cycle with `RuntimeHost::run_turn`.
pub(crate) fn execute_spawn_effect(
    spec: SpawnTaskSpec,
) -> Pin<Box<dyn Future<Output = StepResult> + Send>> {
    Box::pin(async move {
        let (child_ws, worktree_path_str) = match spec.workspace_mode {
            WorkspaceMode::Worktree => {
                match create_worktree(&spec.parent_ws, &spec.effect_id).await {
                    Ok(wt) => {
                        let s = wt.to_string_lossy().to_string();
                        (wt, Some(s))
                    }
                    Err(err) => {
                        return StepResult::Error {
                            effect_id: spec.effect_id,
                            message: err.to_string(),
                            retryable: false,
                        };
                    }
                }
            }
            WorkspaceMode::ReadOnly | WorkspaceMode::Shared => (spec.parent_ws.clone(), None),
        };

        let mut child_gate = PolicyGate::new(spec.budget, None, spec.parent_sandbox.clone());
        if spec.workspace_mode == WorkspaceMode::ReadOnly {
            let mut ro_sandbox = spec.parent_sandbox.clone();
            ro_sandbox.protected_paths.insert(".".to_string());
            let _ = child_gate.apply_monotonic_override(&PolicyOverride {
                budget: None,
                allowed_tools: None,
                sandbox: Some(ro_sandbox),
            });
        }

        let child_session_path = spec
            .parent_ws
            .join(".gestalt")
            .join("sessions")
            .join(format!(
                "{}_spawn_{}.jsonl",
                spec.parent_run_id, spec.effect_id
            ));

        let mut child_host = match RuntimeHost::new(
            format!("{}_{}", spec.parent_run_id, spec.effect_id),
            &child_ws,
            child_session_path,
            spec.child_registry,
            child_gate,
            spec.child_runtime,
            spec.parent_creds,
            spec.parent_sink,
            Arc::new(HeadlessPauseHandler::default()),
        ) {
            Ok(h) => h,
            Err(err) => {
                return StepResult::Error {
                    effect_id: spec.effect_id,
                    message: format!("failed to initialize spawned child host: {err}"),
                    retryable: false,
                };
            }
        };
        child_host.kernel_mut().set_spawn_depth(spec.child_depth);

        let outcome: Result<TurnOutcome, RuntimeError> = child_host
            .run_turn(spec.input, &spec.loop_id, &[], spec.cancel)
            .await;

        let child_head_id = child_host
            .kernel()
            .session_tree()
            .head_id()
            .unwrap_or("unknown")
            .to_string();

        let (output, usage) = match outcome {
            Ok(TurnOutcome::Completed { output, usage, .. }) => (output, usage),
            Ok(TurnOutcome::Aborted { reason, .. }) => {
                return StepResult::Aborted {
                    effect_id: spec.effect_id,
                    reason,
                };
            }
            Err(err) => {
                return StepResult::Error {
                    effect_id: spec.effect_id,
                    message: format!("spawned child turn failed: {err}"),
                    retryable: false,
                };
            }
        };

        let diff_blob_digest = if let Some(ref wt_str) = worktree_path_str {
            match capture_worktree_diff_blob(std::path::Path::new(wt_str), &spec.parent_blobs).await
            {
                Ok(d) => d,
                Err(err) => {
                    return StepResult::Error {
                        effect_id: spec.effect_id,
                        message: err.to_string(),
                        retryable: false,
                    };
                }
            }
        } else {
            None
        };

        StepResult::SpawnCompleted {
            effect_id: spec.effect_id,
            child_head_id,
            worktree_path: worktree_path_str,
            diff_blob_digest,
            output: if output.is_null() {
                json!({ "loop_id": spec.loop_id, "workspace_mode": spec.workspace_mode })
            } else {
                output
            },
            usage,
        }
    })
}

pub(crate) fn spill_tool_output(
    effect_id: &str,
    output: Value,
    is_error: bool,
    blob_store: &BlobStore,
) -> StepResult {
    let serialized = match &output {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    if serialized.len() > blob_store.spill_threshold() {
        match blob_store.spill_if_oversized(&serialized) {
            Ok((preview, digest)) => StepResult::Tool {
                effect_id: effect_id.to_string(),
                output: Value::String(preview),
                is_error,
                blob_digest: digest,
            },
            Err(err) => StepResult::Error {
                effect_id: effect_id.to_string(),
                message: err.to_string(),
                retryable: false,
            },
        }
    } else {
        StepResult::Tool {
            effect_id: effect_id.to_string(),
            output,
            is_error,
            blob_digest: None,
        }
    }
}

/// Validate tool arguments against a JSON Schema (`type`, `required`, and
/// `properties` types) so malformed tool calls return recoverable
/// `StepResult::Tool { is_error: true }` instead of crashing the Turn.
///
/// # Errors
///
/// Returns a concise diagnostic string describing the first schema violation.
pub fn validate_tool_args(schema: &Value, args: &Value) -> Result<(), String> {
    if schema.is_null() {
        return Ok(());
    }
    if let Some(expected_type) = schema.get("type").and_then(Value::as_str)
        && !matches_json_type(expected_type, args)
    {
        return Err(format!("expected type '{expected_type}'"));
    }

    if let Some(obj) = args.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key_val in required {
                if let Some(key) = key_val.as_str()
                    && !obj.contains_key(key)
                {
                    return Err(format!("missing required property '{key}'"));
                }
            }
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (k, v) in obj {
                if let Some(prop_schema) = props.get(k)
                    && let Some(prop_type) = prop_schema.get("type").and_then(Value::as_str)
                    && !matches_json_type(prop_type, v)
                {
                    return Err(format!("property '{k}' expected type '{prop_type}'"));
                }
            }
        }
    }
    Ok(())
}

fn matches_json_type(expected: &str, value: &Value) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "null" => value.is_null(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_tool_args_checks_required_and_types() {
        let schema = json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string" },
                "limit": { "type": "integer" }
            }
        });
        assert!(validate_tool_args(&schema, &json!({"path": "a.rs", "limit": 10})).is_ok());
        assert!(validate_tool_args(&schema, &json!({"limit": 10})).is_err());
        assert!(validate_tool_args(&schema, &json!({"path": 123})).is_err());
    }
}
