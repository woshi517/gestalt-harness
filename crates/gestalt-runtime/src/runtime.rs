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
    snapshot::WorkspaceSnapshotter,
    tool::{ToolCatalog, ToolContext},
    trace::TraceSink,
    AgentLoop,
};

use crate::config::RuntimeConfig;
use crate::error::{Result, RuntimeError};
use crate::inspect::{RuntimeInspect, ToolInspectInfo};
use crate::workspace_snapshot::GitWorkspaceSnapshotter;

use crate::composition_hooks::{
    CompositionHooks, OnEventCtx, RuntimeContextHookAdapter, RuntimeNextTurnHookAdapter,
    RuntimeToolHookAdapter, RuntimeTraceHookAdapter,
};
use crate::context::{ContextContributor, RuntimeContextPipeline};
use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use crate::policy::RuntimePolicyEngine;
use crate::registry::{RuntimeRegistryBuilder, RuntimeRegistrySnapshot};
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
    pub registry: RuntimeRegistryBuilder,
    pub registry_snapshot: RuntimeRegistrySnapshot,
    pub extension_snapshot: Arc<crate::extension::RuntimeExtensionSnapshot>,
    pub extension_manager: Arc<crate::extension::ExtensionManager>,
    pub hooks: gestalt_core::HookRegistry,
    pub composition_hooks: Option<Arc<dyn CompositionHooks>>,
    pub event_bus: RuntimeEventBus,
    /// Shared skill contributor state. Carried by the runtime so activation
    /// can be resolved per-turn from `before_context_build`.
    #[cfg(feature = "skills")]
    pub skill_state:
        Option<Arc<std::sync::Mutex<crate::skills::contributor::SkillContributorState>>>,
    #[cfg(feature = "mcp")]
    pub mcp_registry: Arc<crate::mcp::McpRegistry>,
    #[cfg(feature = "mcp")]
    pub mcp_discovery_state: Arc<std::sync::Mutex<crate::mcp::McpDiscoveryState>>,
    steering_queue: Arc<dyn gestalt_core::session_queue::SteeringQueue>,
    pub workspace_context_snapshot: Option<crate::workspace_context::WorkspaceContextSnapshot>,
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
        registry: RuntimeRegistryBuilder,
        registry_snapshot: RuntimeRegistrySnapshot,
        composition_hooks: Option<Arc<dyn CompositionHooks>>,
        event_bus: RuntimeEventBus,
        #[cfg(feature = "mcp")] mcp_registry: Arc<crate::mcp::McpRegistry>,
        #[cfg(feature = "mcp")] mcp_discovery_state: Arc<
            std::sync::Mutex<crate::mcp::McpDiscoveryState>,
        >,
    ) -> Self {
        let mut extension_snapshot =
            crate::extension::RuntimeExtensionSnapshot::from_registry_snapshot(
                crate::extension::RuntimeGeneration(0),
                registry_snapshot.clone(),
                tools.clone(),
                #[cfg(feature = "mcp")]
                mcp_registry.clone(),
            );
        if composition_hooks.is_some() {
            let context_plan =
                crate::extension::RuntimeExtensionSnapshot::context_plan_from_registry(
                    &registry_snapshot,
                    true,
                );
            let policy_plan = crate::lifecycle::PolicyGuardPlan::new(vec![
                crate::lifecycle::PolicyGuardRegistration {
                    descriptor: crate::lifecycle::TypedCapabilityDescriptor {
                        component_id: "native:composition_hooks:before_tool_policy".to_string(),
                        priority: 0,
                        timeout: std::time::Duration::from_secs(5),
                        failure_mode: crate::lifecycle::CapabilityFailureMode::FailClosed,
                        data_scope: crate::lifecycle::CapabilityDataScope::ToolRequest,
                    },
                    source: "native-composition-hooks".to_string(),
                },
            ]);
            extension_snapshot = extension_snapshot
                .with_context_plan(context_plan)
                .with_policy_plan(policy_plan);
        }
        let extension_snapshot = Arc::new(extension_snapshot);
        let host_context =
            crate::activation::HostLaunchContext::from_runtime_config(&config, event_bus.clone());
        let extension_manager = Arc::new(crate::extension::ExtensionManager::new(
            extension_snapshot.clone(),
            event_bus.clone(),
            Arc::new(crate::extension::NoopExtensionLauncher),
            host_context,
        ));

        Self {
            provider,
            tools,
            middleware,
            policy,
            approval,
            trace_sink,
            config,
            registry,
            registry_snapshot,
            extension_snapshot,
            extension_manager,
            hooks,
            composition_hooks,
            event_bus,
            #[cfg(feature = "skills")]
            skill_state: None,
            #[cfg(feature = "mcp")]
            mcp_registry,
            #[cfg(feature = "mcp")]
            mcp_discovery_state,
            steering_queue: Arc::new(crate::session_queue::InMemorySteeringQueue::new()),
            workspace_context_snapshot: None,
        }
    }

    /// Attach the shared skill state to the runtime. Called by the builder
    /// after the state has been registered with the contributor registry.
    #[cfg(feature = "skills")]
    pub fn with_skill_state(
        mut self,
        state: Arc<std::sync::Mutex<crate::skills::contributor::SkillContributorState>>,
    ) -> Self {
        self.skill_state = Some(state);
        self
    }

    /// Build a resource-access recorder that publishes
    /// `RuntimeEvent::SkillResourceAccessed` on this runtime's event bus. Tools
    /// can install this recorder so that any read against a skill's
    /// `references/` or `scripts/` resources becomes observable in the trace.
    #[cfg(feature = "skills")]
    pub fn skill_resource_recorder(&self) -> Option<crate::skills::ResourceAccessRecorder> {
        self.skill_state
            .as_ref()
            .and_then(|s| s.lock().ok())
            .and_then(|guard| guard.resource_recorder())
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
                top_p: self.config.top_p,
                reasoning_effort: self.config.reasoning_effort,
                text_verbosity: self.config.text_verbosity,
                metadata: self.config.metadata.clone(),
                resolved_model: self.config.resolved_model.clone(),
            },
            TokenBudget {
                model_limit: self
                    .config
                    .max_context_window
                    .or_else(|| {
                        self.config
                            .resolved_model
                            .as_ref()
                            .map(|m| m.max_context_tokens)
                    })
                    .unwrap_or(120_000),
                reserved_output: self
                    .config
                    .reserved_output_tokens
                    .or_else(|| {
                        self.config
                            .resolved_model
                            .as_ref()
                            .map(|m| m.max_output_tokens.min(8192))
                    })
                    .or_else(|| self.config.max_output_tokens.map(|v| v.min(8192)))
                    .unwrap_or(4096),
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
                ignore_patterns: self.config.ignore_patterns.clone(),
            },
            self.config.execution_mode,
            snapshot.clone(),
        );
        session.context_policy = self
            .config
            .context_management_policy
            .clone()
            .unwrap_or_default();

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

        session.append_message(Message::User {
            content: vec![gestalt_core::message::ContentBlock::Text {
                text: input.prompt.clone(),
            }],
            metadata: None,
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

        self.run_session(&mut session, &input.cancel_token, input.event_tx, None)
            .await
    }

    pub async fn run_session(
        &self,
        session: &mut Session,
        cancel_token: &CancelToken,
        event_tx: Option<UnboundedSender<AgentEvent>>,
        initial_prompt_snapshot_hash: Option<String>,
    ) -> Result<RunResult> {
        if let Some(ref resolved_model) = session.config.resolved_model {
            let run_started_event = AgentEvent::RunStarted {
                resolved_model: resolved_model.clone(),
            };
            self.event_bus.publish_agent(run_started_event.clone());
            if let Some(ref sink) = self.trace_sink {
                let _ = sink.emit(run_started_event.clone());
            }
            if let Some(ref tx) = event_tx {
                let _ = tx.send(run_started_event);
            }
        }

        let _ = self
            .steering_queue
            .update_lifecycle(gestalt_core::session_queue::QueueLifecycle::Active)
            .await;

        let snapshot_lease = self.extension_manager.acquire_lease();
        let active_extension_snapshot = snapshot_lease.snapshot.clone();
        let pinned_tools = active_extension_snapshot.tool_catalog();
        let composed_hooks: Arc<dyn crate::composition_hooks::CompositionHooks> = Arc::new(
            crate::composition_hooks::ComposedCompositionHooks {
                user_hooks: self.composition_hooks.clone(),
            },
        );
        let lifecycle_hooks: Arc<dyn crate::composition_hooks::CompositionHooks> =
            Arc::new(crate::composition_hooks::LifecycleCompositionHooks::new(
                composed_hooks,
                active_extension_snapshot.clone(),
            ));
        self.event_bus
            .publish(RuntimeEvent::RuntimeGenerationAdopted {
                session_id: session.id.clone(),
                generation: active_extension_snapshot.generation.0,
                fingerprint: active_extension_snapshot.fingerprint.to_string(),
            });

        let mut core_hooks = self.hooks.clone();
        let mut middleware = self.middleware.clone();
        let mut policy = self.policy.clone();

        let mut maybe_block_reason = None;
        let mut maybe_trace_worker = None;
        let mut maybe_trace_tx = None;

        if self.composition_hooks.is_some() || !active_extension_snapshot.lifecycle_clients.is_empty()
        {
            let block_reason = Arc::new(Mutex::new(None));
            maybe_block_reason = Some(block_reason.clone());

            let (trace_tx, mut trace_rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
            maybe_trace_tx = Some(trace_tx.clone());

            let comp_hooks_clone = lifecycle_hooks.clone();
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
            let Some(assembler) = middleware.as_assembler() else {
                return Err(RuntimeError::Builder(
                    "runtime requires an assembler-backed context pipeline; use AgentRuntimeBuilder::assembler(...) or a pipeline that implements as_assembler()".to_string(),
                ));
            };
            middleware = Arc::new(RuntimeContextPipeline {
                base: assembler,
                patch_store: patch_store.clone(),
            });

            policy = Arc::new(RuntimePolicyEngine {
                base: policy.clone(),
                hooks: lifecycle_hooks.clone(),
                session_id: session.id.clone(),
                event_bus: self.event_bus.clone(),
                #[cfg(feature = "skills")]
                skill_state: self.skill_state.clone(),
            });

            let contributors: Vec<Arc<dyn ContextContributor>> = active_extension_snapshot
                .registry_snapshot
                .context_contributors
                .values()
                .map(|c| c.contributor.clone())
                .collect();

            core_hooks.register_context_hook(Arc::new(RuntimeContextHookAdapter {
                hooks: lifecycle_hooks.clone(),
                patch_store,
                contributors,
                workspace_root: self.config.workspace_root.clone(),
                block_reason: Some(block_reason),
                event_bus: self.event_bus.clone(),
                prompt_snapshot_state: Arc::new(Mutex::new(initial_prompt_snapshot_hash)),
                #[cfg(feature = "skills")]
                skill_state: self.skill_state.clone(),
            }));

            core_hooks.register_tool_hook(Arc::new(RuntimeToolHookAdapter {
                hooks: lifecycle_hooks.clone(),
                event_bus: self.event_bus.clone(),
            }));

            core_hooks.register_next_turn_hook(Arc::new(RuntimeNextTurnHookAdapter {
                hooks: lifecycle_hooks.clone(),
                event_bus: self.event_bus.clone(),
            }));

            core_hooks.register_trace_hook(Arc::new(RuntimeTraceHookAdapter { tx: trace_tx }));
        }

        let loop_result = {
            let loop_ = AgentLoop::new(
                self.provider.clone(),
                pinned_tools,
                middleware,
                policy,
                self.approval.clone(),
                Arc::new(crate::RuntimeToolOutputMaterializer),
                self.config.max_turns,
            )
            .with_hooks(core_hooks)
            .with_steering_queue(self.steering_queue.clone());

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

        // `Completed` belongs to runtime: the outer run lifecycle is finished,
        // trace workers are shut down, and any still-pending steering can be
        // discarded instead of being carried past this run boundary.
        let _ = self
            .steering_queue
            .update_lifecycle(gestalt_core::session_queue::QueueLifecycle::Completed)
            .await;

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
        let active_extension_snapshot = self.extension_manager.active_snapshot();
        let pinned_tool_catalog = active_extension_snapshot.tool_catalog();
        let schemas = pinned_tool_catalog.schemas();
        let tools: Vec<ToolInspectInfo> = schemas
            .iter()
            .map(|s| {
                let name = s
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let schema_hash = crate::registry::compute_schema_hash(s);
                let backend = pinned_tool_catalog.get(&name).and_then(|t| {
                    t.descriptor()
                        .annotations
                        .get("backend")
                        .map(|ann| ann.value.clone())
                });
                ToolInspectInfo {
                    name,
                    schema_hash,
                    backend,
                }
            })
            .collect();
        let tool_schema_hash = crate::registry::compute_tool_schema_hash(&schemas);

        let mut hook_names = active_extension_snapshot
            .registry_snapshot
            .hooks
            .iter()
            .map(|hook| hook.name.clone())
            .collect::<Vec<_>>();
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

        let enabled_host_features = self.config.enabled_host_features.clone();

        let discovered_skills: Vec<crate::inspect::SkillInspectInfo> = self
            .config
            .discovered_skills
            .iter()
            .map(|s| crate::inspect::SkillInspectInfo {
                name: s.name.clone(),
                manifest_hash: s.manifest_hash.clone(),
            })
            .collect();

        #[cfg(feature = "skills")]
        let active_descriptors = self
            .skill_state
            .as_ref()
            .and_then(|state| state.lock().ok().map(|guard| guard.active_descriptors()))
            .unwrap_or_else(|| {
                self.config
                    .discovered_skills
                    .iter()
                    .filter(|s| self.config.active_skills.contains(&s.name))
                    .cloned()
                    .collect()
            });
        #[cfg(not(feature = "skills"))]
        let active_descriptors: Vec<crate::config::SkillDescriptor> = Vec::new();
        let active_skills: Vec<String> = active_descriptors
            .iter()
            .map(|skill| skill.name.clone())
            .collect();

        let skill_fingerprint = if active_descriptors.is_empty() {
            None
        } else {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            for skill in &active_descriptors {
                hasher.update(skill.name.as_bytes());
                hasher.update(skill.manifest_hash.as_bytes());
            }
            Some(format!("{:x}", hasher.finalize()))
        };

        let effective_config_fingerprint = self.config.effective_config_fingerprint.clone();

        let variant_fingerprint = Some(crate::inspect::compute_variant_fingerprint(
            &self.config.model,
            &self.config.provider,
            self.config.max_tokens,
            self.config.temperature,
            self.config.top_p,
            self.config.reasoning_effort.as_ref(),
            self.config.text_verbosity.as_ref(),
        ));

        let mut negotiated_fingerprints = active_extension_snapshot
            .negotiated_protocol
            .iter()
            .map(|(component_id, negotiated_version)| {
                format!("{}:{}", component_id, negotiated_version)
            })
            .collect::<Vec<_>>();
        let negotiated_protocol_fingerprint = if negotiated_fingerprints.is_empty() {
            None
        } else {
            use sha2::{Digest, Sha256};
            negotiated_fingerprints.sort();
            let mut hasher = Sha256::new();
            for s in &negotiated_fingerprints {
                hasher.update(s.as_bytes());
                hasher.update(b";");
            }
            Some(format!("{:x}", hasher.finalize()))
        };

        #[cfg(feature = "mcp")]
        let discovery_threshold = self.config.mcp_discovery_threshold.unwrap_or(5);
        #[cfg(feature = "mcp")]
        let mcp_servers = self.mcp_registry.get_all_states(discovery_threshold);

        let active_extension_snapshot = self.extension_manager.active_snapshot();

        RuntimeInspect {
            runtime_generation: active_extension_snapshot.generation.0,
            runtime_fingerprint: Some(active_extension_snapshot.fingerprint.to_string()),
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
            verifiers: active_extension_snapshot
                .registry_snapshot
                .verifiers
                .iter()
                .map(|verifier| verifier.name.clone())
                .collect(),
            extensions: active_extension_snapshot
                .registry_snapshot
                .extensions
                .clone(),
            context_injectors: active_extension_snapshot
                .registry_snapshot
                .context_contributors
                .keys()
                .cloned()
                .collect(),
            trace_sink_kind,
            trace_run_dir: None,
            workspace_root: self.config.workspace_root.to_string_lossy().to_string(),
            enabled_host_features,
            discovered_skills,
            active_skills,
            skill_fingerprint,
            #[cfg(feature = "mcp")]
            mcp_servers,
            #[cfg(feature = "mcp")]
            mcp_discovery_threshold: self.config.mcp_discovery_threshold,
            effective_config_fingerprint,
            variant_fingerprint,
            negotiated_protocol_fingerprint,
        }
    }

    pub async fn enqueue_message(
        &self,
        session_id: String,
        content: String,
        source: gestalt_core::session_queue::MessageSource,
        idempotency_key: Option<String>,
    ) -> Result<gestalt_core::session_queue::QueueAck> {
        let id = format!("msg-{}", uuid::Uuid::new_v4());
        let msg = gestalt_core::session_queue::QueuedSessionMessage {
            id,
            content,
            source,
            idempotency_key,
            injected_at_turn: None,
        };

        let ack = self.steering_queue.enqueue(msg.clone()).await?;

        if ack == gestalt_core::session_queue::QueueAck::Queued {
            self.event_bus.publish(RuntimeEvent::SessionMessageQueued {
                session_id,
                message: msg,
            });
        }

        Ok(ack)
    }
}
