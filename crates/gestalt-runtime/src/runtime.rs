use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

use gestalt_core::{
    approval::ApprovalProvider,
    cancel::CancelToken,
    context::{ContextPipeline, TokenBudget},
    event::AgentEvent,
    message::Message,
    policy::PolicyEngine,
    provider::Provider,
    session::{RunResult, Session, SessionConfig},
    snapshot::{GitWorkspaceSnapshotter, WorkspaceSnapshotter},
    tool::{ToolCatalog, ToolContext},
    trace::TraceSink,
    AgentLoop,
};

use crate::config::RuntimeConfig;
use crate::error::{Result, RuntimeError};
use crate::inspect::{RuntimeInspect, ToolInspectInfo};

use crate::composition_hooks::{
    CompositionHooks, OnEventCtx, RuntimeContextHookAdapter, RuntimeNextTurnHookAdapter,
    RuntimeToolHookAdapter, RuntimeTraceHookAdapter,
};
use crate::context::{ContextContributor, RuntimeContextPipeline};
use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use crate::policy::RuntimePolicyEngine;
use crate::registry::RuntimeRegistry;
use std::sync::Mutex;

pub struct UserInput {
    pub prompt: String,
    pub session_id: Option<String>,
    pub cancel_token: CancelToken,
    pub event_tx: Option<UnboundedSender<AgentEvent>>,
    pub artifact_dir: Option<std::path::PathBuf>,
}

pub struct AgentRuntime {
    pub provider: Arc<dyn Provider>,
    pub tools: Arc<dyn ToolCatalog>,
    pub middleware: Arc<dyn ContextPipeline>,
    pub policy: Arc<dyn PolicyEngine>,
    pub approval: Arc<dyn ApprovalProvider>,
    pub trace_sink: Option<Arc<dyn TraceSink>>,
    pub config: RuntimeConfig,
    pub registry: RuntimeRegistry,
    pub hooks: gestalt_core::HookRegistry,
    pub composition_hooks: Option<Arc<dyn CompositionHooks>>,
    pub event_bus: RuntimeEventBus,
}

impl AgentRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: Arc<dyn ToolCatalog>,
        middleware: Arc<dyn ContextPipeline>,
        policy: Arc<dyn PolicyEngine>,
        approval: Arc<dyn ApprovalProvider>,
        trace_sink: Option<Arc<dyn TraceSink>>,
        config: RuntimeConfig,
        hooks: gestalt_core::HookRegistry,
        registry: RuntimeRegistry,
        composition_hooks: Option<Arc<dyn CompositionHooks>>,
        event_bus: RuntimeEventBus,
    ) -> Self {
        Self {
            provider,
            tools,
            middleware,
            policy,
            approval,
            trace_sink,
            config,
            registry,
            hooks,
            composition_hooks,
            event_bus,
        }
    }

    pub async fn run_prompt(&self, input: UserInput) -> Result<RunResult> {
        let session_id = input
            .session_id
            .unwrap_or_else(|| format!("session-{}", uuid::Uuid::new_v4()));

        let snapshotter = GitWorkspaceSnapshotter;
        let snapshot = snapshotter.capture(&self.config.workspace_root).await?;

        let model = if self.config.model.is_empty() {
            self.provider.default_model().to_string()
        } else {
            self.config.model.clone()
        };

        let mut session = Session::new(
            session_id.clone(),
            SessionConfig {
                model,
                provider: self.config.provider.clone(),
                max_tokens: self.config.max_tokens,
                temperature: self.config.temperature,
                max_turns: self.config.max_turns,
            },
            TokenBudget {
                model_limit: self.config.max_context_window.unwrap_or(120_000),
                reserved_output: self.config.reserved_output_tokens.unwrap_or(8_000),
                used_system: 0,
                used_history: 0,
                used_sources: 0,
                used_tools: 0,
                used_memory: 0,
                minimum_turn_budget: 16,
            },
            ToolContext {
                working_dir: self.config.workspace_root.clone(),
                workspace_root: Some(self.config.workspace_root.clone()),
                timeout: Duration::from_secs(self.config.bash_timeout_secs.unwrap_or(60)),
                allow_network: self.config.allow_network,
                environment: self.config.environment.clone(),
                max_output_bytes: self.config.max_output_tokens.unwrap_or(4_000),
                artifact_dir: input.artifact_dir,
                current_tool_call_id: None,
            },
            self.config.execution_mode,
            snapshot.clone(),
        );

        self.event_bus.publish(RuntimeEvent::SessionSpawned {
            session_id: session_id.clone(),
        });

        let snapshot_id: String = snapshot.content_hash.chars().take(12).collect();
        let snapshot_event = AgentEvent::WorkspaceSnapshotCaptured {
            snapshot_id,
            dirty: snapshot.git_dirty.unwrap_or(false),
        };
        self.event_bus.publish_agent(snapshot_event.clone());
        if let Some(ref sink) = self.trace_sink {
            let _ = sink.emit(snapshot_event.clone());
        }
        if let Some(ref tx) = input.event_tx {
            let _ = tx.send(snapshot_event);
        }

        session.history.push(Message::User {
            content: vec![gestalt_core::message::ContentBlock::Text {
                text: input.prompt.clone(),
            }],
        });

        let user_msg_event = AgentEvent::UserMessage {
            content: input.prompt.clone(),
        };
        self.event_bus.publish_agent(user_msg_event.clone());
        if let Some(ref sink) = self.trace_sink {
            let _ = sink.emit(user_msg_event.clone());
        }
        if let Some(ref tx) = input.event_tx {
            let _ = tx.send(user_msg_event);
        }

        self.run_session(
            &mut session,
            &input.cancel_token,
            input.event_tx,
            None,
        )
            .await
    }

    pub async fn run_session(
        &self,
        session: &mut Session,
        cancel_token: &CancelToken,
        event_tx: Option<UnboundedSender<AgentEvent>>,
        initial_prompt_snapshot_hash: Option<String>,
    ) -> Result<RunResult> {
        let mut core_hooks = self.hooks.clone();
        let mut middleware = self.middleware.clone();
        let mut policy = self.policy.clone();

        let mut maybe_block_reason = None;
        let mut maybe_trace_worker = None;
        let mut maybe_trace_tx = None;

        if let Some(ref comp_hooks) = self.composition_hooks {
            let block_reason = Arc::new(Mutex::new(None));
            maybe_block_reason = Some(block_reason.clone());

            let (trace_tx, mut trace_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
            maybe_trace_tx = Some(trace_tx.clone());

            let comp_hooks_clone = comp_hooks.clone();
            let session_id_clone = session.id.clone();
            let event_bus_clone = self.event_bus.clone();
            let trace_worker = tokio::spawn(async move {
                while let Some(event) = trace_rx.recv().await {
                    event_bus_clone.publish(RuntimeEvent::HookStarted {
                        hook_name: "on_event".to_string(),
                        lifecycle_point: "on_event".to_string(),
                    });
                    let ctx = OnEventCtx {
                        session_id: session_id_clone.clone(),
                        event,
                    };
                    match comp_hooks_clone.on_event(&ctx).await {
                        Ok(()) => {
                            event_bus_clone.publish(RuntimeEvent::HookCompleted {
                                hook_name: "on_event".to_string(),
                                lifecycle_point: "on_event".to_string(),
                                outcome: "Continue".to_string(),
                            });
                        }
                        Err(err) => {
                            event_bus_clone.publish(RuntimeEvent::HookFailed {
                                hook_name: "on_event".to_string(),
                                lifecycle_point: "on_event".to_string(),
                                error: err.to_string(),
                            });
                        }
                    }
                }
            });
            maybe_trace_worker = Some(trace_worker);

            let patch_store = Arc::new(Mutex::new(Vec::new()));

            middleware = Arc::new(RuntimeContextPipeline {
                base: middleware.clone(),
                patch_store: patch_store.clone(),
            });

            policy = Arc::new(RuntimePolicyEngine {
                base: policy.clone(),
                hooks: comp_hooks.clone(),
                session_id: session.id.clone(),
                event_bus: self.event_bus.clone(),
            });

            let contributors: Vec<Arc<dyn ContextContributor>> = self
                .registry
                .context_contributors
                .values()
                .map(|c| c.contributor.clone())
                .collect();

            core_hooks.register_context_hook(Arc::new(RuntimeContextHookAdapter {
                hooks: comp_hooks.clone(),
                patch_store,
                contributors,
                workspace_root: self.config.workspace_root.clone(),
                block_reason: Some(block_reason),
                event_bus: self.event_bus.clone(),
                prompt_snapshot_state: Arc::new(Mutex::new(initial_prompt_snapshot_hash)),
            }));

            core_hooks.register_tool_hook(Arc::new(RuntimeToolHookAdapter {
                hooks: comp_hooks.clone(),
                event_bus: self.event_bus.clone(),
            }));

            core_hooks.register_next_turn_hook(Arc::new(RuntimeNextTurnHookAdapter {
                hooks: comp_hooks.clone(),
                event_bus: self.event_bus.clone(),
            }));

            core_hooks.register_trace_hook(Arc::new(RuntimeTraceHookAdapter { tx: trace_tx }));
        }

        let loop_result = {
            let loop_ = AgentLoop::new(
                self.provider.clone(),
                self.tools.clone(),
                middleware,
                policy,
                self.approval.clone(),
                self.config.max_turns,
            )
            .with_hooks(core_hooks);

            let event_bus_clone = self.event_bus.clone();
            loop_
                .run(
                    session,
                    cancel_token,
                    self.trace_sink.as_ref().map(|x| x.as_ref()),
                    |event| {
                        event_bus_clone.publish_agent(event.clone());
                        if let Some(ref sink) = self.trace_sink {
                            let _ = sink.emit(event.clone());
                        }
                        if let Some(ref tx) = event_tx {
                            let _ = tx.send(event.clone());
                        }
                        if let Some(ref br) = maybe_block_reason {
                            if let Some(reason) = &*br.lock().unwrap() {
                                return Err(gestalt_core::error::HarnessError::Policy(
                                    gestalt_core::error::PolicyError::Denied(reason.clone()),
                                ));
                            }
                        }
                        Ok::<(), gestalt_core::error::HarnessError>(())
                    },
                )
                .await
        };

        if let Some(tx) = maybe_trace_tx {
            drop(tx);
        }
        if let Some(worker) = maybe_trace_worker {
            let _ = worker.await;
        }

        match loop_result {
            Ok(run_result) => Ok(run_result),
            Err(gestalt_core::error::HarnessError::Cancelled) => {
                let interrupted_event = AgentEvent::Interrupted {
                    reason: "signal".to_string(),
                };
                self.event_bus.publish_agent(interrupted_event.clone());
                if let Some(ref sink) = self.trace_sink {
                    let _ = sink.emit(interrupted_event.clone());
                }
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(interrupted_event);
                }
                Err(RuntimeError::Harness(
                    gestalt_core::error::HarnessError::Cancelled,
                ))
            }
            Err(e) => Err(RuntimeError::Harness(e)),
        }
    }

    pub fn inspect(&self) -> RuntimeInspect {
        let schemas = self.tools.schemas();
        let tools: Vec<ToolInspectInfo> = schemas
            .iter()
            .map(|s| {
                let name = s
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let schema_hash = crate::registry::compute_schema_hash(s);
                ToolInspectInfo { name, schema_hash }
            })
            .collect();
        let tool_schema_hash = crate::registry::compute_tool_schema_hash(&schemas);

        let mut hook_names = self.registry.hooks.clone();
        if self.composition_hooks.is_some() {
            hook_names.push("RuntimeContextHookAdapter".to_string());
            hook_names.push("RuntimeToolHookAdapter".to_string());
            hook_names.push("RuntimeTraceHookAdapter".to_string());
        }
        let hook_contract_hash = crate::inspect::compute_hook_contract_hash(&hook_names);

        // Try to compute policy fingerprint if policy source is known or policies.toml exists
        let policies_path = self.config.workspace_root.join(".gestalt/policies.toml");
        let (policy_fingerprint, policy_source_path) = if policies_path.exists() {
            let content = std::fs::read_to_string(&policies_path).unwrap_or_default();
            let fp = crate::inspect::compute_policy_fingerprint(&content);
            (Some(fp), Some(policies_path.to_string_lossy().to_string()))
        } else {
            (None, None)
        };

        // Determine trace sink kind
        let trace_sink_kind = self
            .trace_sink
            .as_ref()
            .map(|_| "JsonlTraceSink".to_string());

        let enabled_cli_features = self.config.enabled_cli_features.clone();

        RuntimeInspect {
            provider_name: self.config.provider.clone(),
            provider_model: self.config.model.clone(),
            execution_mode: format!("{:?}", self.config.execution_mode),
            max_turns: self.config.max_turns,
            context_pipeline_version: self.middleware.version().to_string(),
            tools,
            tool_schema_hash,
            policy_fingerprint,
            policy_source_path,
            hooks: hook_names,
            hook_contract_hash,
            verifiers: self.registry.verifiers.clone(),
            extensions: self.registry.extensions.clone(),
            context_injectors: self.registry.context_contributors.keys().cloned().collect(),
            trace_sink_kind,
            trace_run_dir: None,
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
            enabled_cli_features,
        }
    }
}
