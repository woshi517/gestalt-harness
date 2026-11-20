#![forbid(unsafe_code)]
//! Out-of-process stdio JSON-RPC 2.0 extension host (`extension/describe`,
//! `loop/step`, `decorator/wrap_step`, `builder/build`, `tool/call`,
//! `policy/evaluate`, `command/run`, `extension/shutdown`, and `/reload`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gestalt_kernel::{
    ArtifactEntry, EffectEnvelope, ExtensionRegistry, ExternalExtensionSpec, LoopAction,
    LoopConfig, ModelCapabilities, PolicyDecision, SandboxPolicy, SessionNode, StepResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::ExternalExtensionConfig;
use crate::credentials::StreamRedactor;
use crate::provider::StreamDeltaFn;
use crate::sandbox::SandboxDriver;
use crate::stdio_rpc::{IncomingControl, StdioJsonRpcTransport};
use crate::{CancellationToken, RuntimeError};

/// Request payload for out-of-process `loop/step`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalLoopStepRequest {
    /// Active Phase ID.
    pub phase_id: String,
    /// Active Turn ID.
    pub turn_id: String,
    /// Typed handoff JSON passed into this Phase.
    pub handoff: Value,
    /// Effective `LoopConfig`.
    pub config: LoopConfig,
    /// Opaque worker state token from the prior step.
    pub state_token: Option<Value>,
    /// Prior step results (or `None` on initial step).
    pub results: Option<Vec<StepResult>>,
}

/// Response payload from out-of-process `loop/step` or `decorator/wrap_step`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalLoopStepResponse {
    /// Resumable `LoopAction` returned by the external worker.
    pub action: LoopAction,
    /// Updated opaque state token persisted on `SessionTree`.
    #[serde(default)]
    pub state_token: Option<Value>,
}

/// Request payload for out-of-process `decorator/wrap_step`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalDecoratorStepRequest {
    /// External decorator ID.
    pub decorator_id: String,
    /// Active Phase ID.
    pub phase_id: String,
    /// Inner loop action being wrapped.
    pub inner_action: LoopAction,
    /// Effective `LoopConfig`.
    pub config: LoopConfig,
    /// Opaque decorator state token.
    pub state_token: Option<Value>,
}

/// Request payload for out-of-process `builder/build`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalBuilderRequest {
    /// External builder ID.
    pub builder_id: String,
    /// Active branch nodes from root to head.
    pub session_slice: Vec<SessionNode>,
    /// Snapshotted or live workspace artifacts.
    pub artifacts: BTreeMap<String, ArtifactEntry>,
    /// Target model capabilities.
    pub caps: ModelCapabilities,
}

/// Response payload from out-of-process `command/run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalCommandResponse {
    /// Command output JSON.
    #[serde(default)]
    pub output: Value,
    /// Optional steering signal to enqueue.
    #[serde(default)]
    pub steering: Option<Value>,
}

/// Live out-of-process stdio JSON-RPC 2.0 extension worker handle.
pub struct ExtIpcWorker {
    transport: StdioJsonRpcTransport,
}

impl ExtIpcWorker {
    /// Spawn a new out-of-process extension worker inside `sandbox_driver`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `config.command` is empty or process spawn fails.
    pub fn spawn(
        name: String,
        config: &ExternalExtensionConfig,
        workspace_root: &Path,
        sandbox_policy: &SandboxPolicy,
        sandbox_driver: &Arc<dyn SandboxDriver>,
        redactor: StreamRedactor,
    ) -> Result<Self, RuntimeError> {
        let transport = StdioJsonRpcTransport::spawn(
            name,
            config,
            workspace_root,
            sandbox_policy,
            sandbox_driver,
            redactor,
        )?;
        Ok(Self { transport })
    }

    /// Perform the `extension/describe` handshake.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on IPC timeout, crash, or invalid schema.
    pub async fn describe(&mut self) -> Result<ExternalExtensionSpec, RuntimeError> {
        let res = self
            .transport
            .call_rpc(
                "extension/describe",
                json!({}),
                CancellationToken::new(),
                true,
                |_| (None, IncomingControl::Continue),
            )
            .await?;
        let mut spec: ExternalExtensionSpec = serde_json::from_value(res)?;
        if spec.name.is_empty() {
            spec.name = self.transport.name().to_string();
        }
        Ok(spec)
    }

    /// Execute `loop/step` on the external worker.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on IPC timeout, cancellation, or invalid output.
    pub async fn loop_step(
        &mut self,
        req: &ExternalLoopStepRequest,
        cancel: CancellationToken,
    ) -> Result<ExternalLoopStepResponse, RuntimeError> {
        let params = serde_json::to_value(req)?;
        let res = self
            .transport
            .call_rpc("loop/step", params, cancel, true, |_| {
                (None, IncomingControl::Continue)
            })
            .await?;
        Ok(serde_json::from_value(res)?)
    }

    /// Execute `decorator/wrap_step` on the external worker.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on IPC timeout, cancellation, or invalid output.
    pub async fn decorator_wrap_step(
        &mut self,
        req: &ExternalDecoratorStepRequest,
        cancel: CancellationToken,
    ) -> Result<ExternalLoopStepResponse, RuntimeError> {
        let params = serde_json::to_value(req)?;
        let res = self
            .transport
            .call_rpc("decorator/wrap_step", params, cancel, true, |_| {
                (None, IncomingControl::Continue)
            })
            .await?;
        Ok(serde_json::from_value(res)?)
    }

    /// Execute `builder/build` on the external worker.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on IPC timeout, cancellation, or protocol error.
    pub async fn builder_build(
        &mut self,
        req: &ExternalBuilderRequest,
        cancel: CancellationToken,
    ) -> Result<Value, RuntimeError> {
        let params = serde_json::to_value(req)?;
        self.transport
            .call_rpc("builder/build", params, cancel, true, |_| {
                (None, IncomingControl::Continue)
            })
            .await
    }

    /// Execute `tool/call` on the external worker, forwarding `stream/delta`
    /// notifications to `on_delta`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on IPC timeout, cancellation, or invalid `StepResult`.
    pub async fn tool_call(
        &mut self,
        tool_name: &str,
        args: &Value,
        cwd: &Path,
        blob_dir: &Path,
        on_delta: StreamDeltaFn<'_>,
        cancel: CancellationToken,
    ) -> Result<StepResult, RuntimeError> {
        let params = json!({
            "tool_name": tool_name,
            "args": args,
            "cwd": cwd.to_string_lossy(),
            "blob_dir": blob_dir.to_string_lossy(),
        });
        let redactor = self.transport.redactor().clone();
        let res = self
            .transport
            .call_rpc("tool/call", params, cancel, true, |msg| {
                if msg.get("id").is_none()
                    && msg.get("method").and_then(Value::as_str) == Some("stream/delta")
                {
                    let channel = msg
                        .pointer("/params/channel")
                        .and_then(Value::as_str)
                        .unwrap_or("tool_output");
                    let mut payload = msg
                        .pointer("/params/payload")
                        .cloned()
                        .unwrap_or(Value::Null);
                    redactor.redact_json(&mut payload);
                    on_delta(channel, payload);
                }
                (None, IncomingControl::Continue)
            })
            .await?;
        Ok(serde_json::from_value(res)?)
    }

    /// Evaluate `policy/evaluate` on the external worker, **failing closed** to
    /// `PolicyDecision::Deny` on any timeout, crash, or invalid response.
    pub async fn policy_evaluate(
        &mut self,
        effect: &EffectEnvelope,
        session_slice: &[SessionNode],
    ) -> PolicyDecision {
        let params = json!({
            "effect": effect,
            "session_slice": session_slice,
        });
        let name = self.transport.name().to_string();
        match self
            .transport
            .call_rpc(
                "policy/evaluate",
                params,
                CancellationToken::new(),
                true,
                |_| (None, IncomingControl::Continue),
            )
            .await
        {
            Ok(val) => match val.get("decision").and_then(Value::as_str) {
                Some("allow") => PolicyDecision::Allow,
                Some(_) => {
                    let reason = val
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("denied by external policy")
                        .to_string();
                    PolicyDecision::Deny { reason }
                }
                None => PolicyDecision::Deny {
                    reason: format!(
                        "external policy '{name}' returned malformed decision (fail-closed)"
                    ),
                },
            },
            Err(err) => PolicyDecision::Deny {
                reason: format!("external policy '{name}' failed closed: {err}"),
            },
        }
    }

    /// Execute `command/run` on the external worker.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] on IPC timeout or protocol error.
    pub async fn command_run(
        &mut self,
        command: &str,
        args: &Value,
        session_head: Option<&SessionNode>,
    ) -> Result<ExternalCommandResponse, RuntimeError> {
        let params = json!({
            "command": command,
            "args": args,
            "session_head": session_head,
        });
        let res = self
            .transport
            .call_rpc(
                "command/run",
                params,
                CancellationToken::new(),
                true,
                |_| (None, IncomingControl::Continue),
            )
            .await?;
        Ok(serde_json::from_value(res)?)
    }

    /// Gracefully shut down the worker via `extension/shutdown`, closing stdin
    /// and falling back to killing the process if it does not exit within 250 ms.
    pub async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        let req_id = self.transport.allocate_id();
        let _ = self
            .transport
            .send_json(&json!({
                "jsonrpc": "2.0",
                "id": req_id,
                "method": "extension/shutdown",
                "params": {}
            }))
            .await;
        self.transport.close_stdin_and_shutdown(250).await;
        Ok(())
    }
}

/// Host managing all active out-of-process stdio JSON-RPC extension workers
/// and supporting live between-Turn/Phase `/reload <ext>`.
#[derive(Default)]
pub struct ExtIpcHost {
    configs: BTreeMap<String, ExternalExtensionConfig>,
    workers: BTreeMap<String, ExtIpcWorker>,
    tool_owners: BTreeMap<String, String>,
    command_owners: BTreeMap<String, String>,
    policy_owners: BTreeMap<String, String>,
    decorator_owners: BTreeMap<String, String>,
}

impl ExtIpcHost {
    /// Create an empty [`ExtIpcHost`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a worker, run `extension/describe`, and register its slots into
    /// `registry`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if spawning or `extension/describe` fails.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_and_register(
        &mut self,
        ext_name: &str,
        config: ExternalExtensionConfig,
        workspace_root: PathBuf,
        sandbox_policy: SandboxPolicy,
        sandbox_driver: Arc<dyn SandboxDriver>,
        redactor: StreamRedactor,
        registry: &mut ExtensionRegistry,
    ) -> Result<ExternalExtensionSpec, RuntimeError> {
        let mut worker = ExtIpcWorker::spawn(
            ext_name.to_string(),
            &config,
            &workspace_root,
            &sandbox_policy,
            &sandbox_driver,
            redactor,
        )?;
        let spec = worker.describe().await?;
        let canonical_name = spec.name.clone();

        for tool in &spec.tools {
            self.tool_owners
                .insert(tool.name.clone(), canonical_name.clone());
        }
        for cmd in &spec.commands {
            self.command_owners
                .insert(cmd.name.clone(), canonical_name.clone());
        }
        for pol in &spec.policies {
            self.policy_owners
                .insert(pol.id.clone(), canonical_name.clone());
        }
        for dec in &spec.decorators {
            self.decorator_owners
                .insert(dec.id.clone(), canonical_name.clone());
        }

        registry.register_external_describe(spec.clone());
        self.configs.insert(canonical_name.clone(), config);
        self.workers.insert(canonical_name, worker);
        Ok(spec)
    }

    /// Hot-reload an external extension worker between Turns/Phases (`/reload <ext>`).
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] if `ext_name` is unknown or respawning fails.
    pub async fn reload_extension(
        &mut self,
        ext_name: &str,
        workspace_root: PathBuf,
        sandbox_policy: SandboxPolicy,
        sandbox_driver: Arc<dyn SandboxDriver>,
        redactor: StreamRedactor,
        registry: &mut ExtensionRegistry,
    ) -> Result<ExternalExtensionSpec, RuntimeError> {
        let config =
            self.configs
                .get(ext_name)
                .cloned()
                .ok_or_else(|| RuntimeError::IpcProtocol {
                    ext_name: ext_name.to_string(),
                    message: "cannot reload unknown external extension".to_string(),
                })?;

        if let Some(mut old_worker) = self.workers.remove(ext_name) {
            let _ = old_worker.shutdown().await;
        }
        self.tool_owners.retain(|_, owner| owner != ext_name);
        self.command_owners.retain(|_, owner| owner != ext_name);
        self.policy_owners.retain(|_, owner| owner != ext_name);
        self.decorator_owners.retain(|_, owner| owner != ext_name);
        registry.unload_external_extension(ext_name);

        self.start_and_register(
            ext_name,
            config,
            workspace_root,
            sandbox_policy,
            sandbox_driver,
            redactor,
            registry,
        )
        .await
    }

    /// Look up a mutable handle to an active worker by extension name.
    pub fn worker_mut(&mut self, ext_name: &str) -> Option<&mut ExtIpcWorker> {
        self.workers.get_mut(ext_name)
    }

    /// Return the owning extension name for an external tool, if any.
    #[must_use]
    pub fn owner_of_tool(&self, tool_name: &str) -> Option<&str> {
        self.tool_owners.get(tool_name).map(String::as_str)
    }

    /// Return the owning extension name for an external policy rule, if any.
    #[must_use]
    pub fn owner_of_policy(&self, policy_id: &str) -> Option<&str> {
        self.policy_owners.get(policy_id).map(String::as_str)
    }

    /// Return the owning extension name for an external command, if any.
    #[must_use]
    pub fn owner_of_command(&self, cmd_name: &str) -> Option<&str> {
        self.command_owners.get(cmd_name).map(String::as_str)
    }

    /// Return the owning extension name for an external decorator, if any.
    #[must_use]
    pub fn owner_of_decorator(&self, decorator_id: &str) -> Option<&str> {
        self.decorator_owners.get(decorator_id).map(String::as_str)
    }

    /// Shut down all active external workers cleanly.
    pub async fn shutdown_all(&mut self) {
        for (_, mut worker) in std::mem::take(&mut self.workers) {
            let _ = worker.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::UnsandboxedFallbackDriver;

    #[cfg(unix)]
    #[tokio::test]
    async fn ext_ipc_lifecycle_stream_delta_reload_and_fail_closed_policy() {
        let dir = std::env::temp_dir().join(format!("gestalt_ipc_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let script_path = dir.join("worker.sh");
        std::fs::write(
            &script_path,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"extension/describe"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"name":"test_ext","loops":[{"id":"ext_loop","digest":"d1"}],"tools":[{"name":"ext_echo","description":"echo","parameters_schema":{"type":"object"},"default_side_effect":"read"}],"policies":[{"id":"ext_pol","digest":"p1"}]}}\n' "$id"
      ;;
    *'"method":"loop/step"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"action":{"action":"done","payload":{"ok":true}},"state_token":{"step":1}}}\n' "$id"
      ;;
    *'"method":"tool/call"'*)
      printf '{"jsonrpc":"2.0","method":"stream/delta","params":{"channel":"tool_output","payload":"chunk1"}}\n'
      printf '{"jsonrpc":"2.0","id":%s,"result":{"status":"tool","effect_id":"e1","output":"done","is_error":false,"blob_digest":null}}\n' "$id"
      ;;
    *'"method":"policy/evaluate"'*)
      sleep 1
      ;;
    *'"method":"extension/shutdown"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{}}\n' "$id"
      exit 0
      ;;
  esac
done
"#,
        )
        .expect("write script");

        let mut host = ExtIpcHost::new();
        let mut reg = ExtensionRegistry::new();
        let cfg = ExternalExtensionConfig {
            command: vec!["sh".to_string(), script_path.to_string_lossy().to_string()],
            protocol: crate::config::ExtensionProtocol::Rpc,
            timeout_ms: 150,
        };

        let spec = host
            .start_and_register(
                "test_ext",
                cfg,
                dir.clone(),
                SandboxPolicy::default(),
                Arc::new(UnsandboxedFallbackDriver),
                StreamRedactor::default(),
                &mut reg,
            )
            .await
            .expect("start_and_register");
        assert_eq!(spec.name, "test_ext");
        assert!(reg.tools.contains_key("ext_echo"));
        assert!(reg.external_loops.contains_key("ext_loop"));

        let deltas = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let deltas_cb = deltas.clone();
        let worker = host.worker_mut("test_ext").expect("worker");
        let tool_res = worker
            .tool_call(
                "ext_echo",
                &json!({"x": 1}),
                &dir,
                &dir,
                &move |ch, payload| {
                    if let Ok(mut guard) = deltas_cb.lock() {
                        guard.push((ch.to_string(), payload));
                    }
                },
                CancellationToken::new(),
            )
            .await
            .expect("tool_call");
        assert!(matches!(
            tool_res,
            StepResult::Tool {
                is_error: false,
                ..
            }
        ));
        assert_eq!(deltas.lock().expect("lock").len(), 1);

        let decision = worker
            .policy_evaluate(
                &EffectEnvelope {
                    effect_id: "e9".into(),
                    effect: gestalt_kernel::Effect::Tool {
                        name: "ext_echo".into(),
                        args: json!({}),
                        side_effect: gestalt_kernel::SideEffectClass::Read,
                    },
                },
                &[],
            )
            .await;
        assert!(matches!(decision, PolicyDecision::Deny { .. }));

        let reloaded = host
            .reload_extension(
                "test_ext",
                dir.clone(),
                SandboxPolicy::default(),
                Arc::new(UnsandboxedFallbackDriver),
                StreamRedactor::default(),
                &mut reg,
            )
            .await
            .expect("reload");
        assert_eq!(reloaded.name, "test_ext");

        host.shutdown_all().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
