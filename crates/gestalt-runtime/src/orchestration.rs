use crate::artifact_store::ArtifactStore;
use crate::builder::AgentRuntimeBuilder;
use crate::error::{Result, RuntimeError};
use crate::event_bus::{RuntimeEvent, RuntimeEventBus};
use crate::runtime::AgentRuntime;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

pub struct OrchestrationTask {
    pub prompt: String,
    pub input_artifacts: Vec<String>,
}

pub struct OrchestrationResult {
    pub output: String,
    pub output_artifacts: Vec<String>,
}

#[async_trait::async_trait]
pub trait Orchestrator: Send + Sync {
    async fn execute(
        &self,
        handle: Arc<dyn AgentRuntimeHandle>,
        task: OrchestrationTask,
    ) -> Result<OrchestrationResult>;
}

pub struct RuntimeHost {
    pub workspace_root: std::path::PathBuf,
    pub config: crate::config::RuntimeConfig,
    pub extension_manager: Arc<crate::extension::ExtensionManager>,
    pub session_registry: Arc<Mutex<HashMap<String, Arc<AgentRuntime>>>>,
    pub event_bus: RuntimeEventBus,
    pub artifact_store: Arc<dyn ArtifactStore>,
    pub approval_broker: Arc<crate::activation::HostApprovalBroker>,
    pub extension_source: Arc<dyn crate::activation::ExtensionSource>,
    pub builder: AgentRuntimeBuilder,
}

struct EmptyToolCatalog;

impl gestalt_core::tool::ToolCatalog for EmptyToolCatalog {
    fn schemas(&self) -> Vec<gestalt_core::tool::ToolSchema> {
        Vec::new()
    }
    fn get(&self, _name: &str) -> Option<Arc<dyn gestalt_core::tool::Tool>> {
        None
    }
}

fn default_global_extension_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".gestalt"))
}

impl RuntimeHost {
    pub fn new(builder: AgentRuntimeBuilder, artifact_store: Arc<dyn ArtifactStore>) -> Self {
        let workspace_root = builder.config.workspace_root.clone();
        let config = builder.config.clone();
        let event_bus = builder.event_bus.clone();
        let approval_broker = Arc::new(crate::activation::HostApprovalBroker::new());
        let discovery_source: Arc<dyn crate::activation::ExtensionSource> =
            Arc::new(crate::discovery::DiscoverySource::new(
                crate::discovery::ExtensionDiscovery::new(
                    workspace_root.clone(),
                    default_global_extension_dir(),
                ),
                Vec::new(),
            ));

        let registry_snapshot = builder.registry.snapshot();
        let extension_snapshot = crate::extension::RuntimeExtensionSnapshot::from_registry_snapshot(
            crate::extension::RuntimeGeneration(0),
            registry_snapshot.clone(),
            builder.tools.clone().unwrap_or_else(|| {
                Arc::new(
                    crate::tool_catalog::ComposedToolCatalog::new(
                        Arc::new(EmptyToolCatalog),
                        std::collections::BTreeMap::new(),
                    )
                    .unwrap(),
                )
            }),
            #[cfg(feature = "mcp")]
            builder.mcp_registry.clone().unwrap_or_else(|| {
                Arc::new(crate::mcp::McpRegistry::new(
                    workspace_root.clone(),
                    std::collections::HashMap::new(),
                ))
            }),
        );
        let extension_snapshot = Arc::new(extension_snapshot);
        let host_context =
            crate::activation::HostLaunchContext::from_runtime_config(&config, event_bus.clone());
        let extension_manager = Arc::new(crate::extension::ExtensionManager::new(
            extension_snapshot.clone(),
            event_bus.clone(),
            Arc::new(crate::extension::LocalProcessLauncher),
            host_context,
        ));

        if !builder.extension_packages.is_empty() {
            let mut packages = builder.extension_packages.clone();
            crate::extension::apply_trust_decisions(
                &mut packages,
                &config.trusted_extension_ids,
                &config.trusted_extension_pins,
            );
            let pipeline = crate::activation::ExtensionActivationPipeline {
                discovery: Arc::new(crate::activation::StaticExtensionSource::new(packages)),
                launcher: Arc::new(crate::extension::LocalProcessLauncher),
                base_composition: Arc::new(crate::activation::BaseRuntimeComposition {
                    tool_catalog: builder.tools.clone().unwrap_or_else(|| {
                        Arc::new(
                            crate::tool_catalog::ComposedToolCatalog::new(
                                Arc::new(EmptyToolCatalog),
                                std::collections::BTreeMap::new(),
                            )
                            .unwrap(),
                        )
                    }),
                    #[cfg(feature = "mcp")]
                    mcp_registry: builder.mcp_registry.clone().unwrap_or_else(|| {
                        Arc::new(crate::mcp::McpRegistry::new(
                            workspace_root.clone(),
                            std::collections::HashMap::new(),
                        ))
                    }),
                    base_registry: registry_snapshot.clone(),
                }),
                host_context: crate::activation::HostLaunchContext::from_runtime_config(
                    &config,
                    event_bus.clone(),
                ),
            };
            let manager = extension_manager.clone();
            let pipeline_result = std::thread::spawn(move || {
                let runtime = tokio::runtime::Runtime::new().map_err(|err| {
                    RuntimeError::Builder(format!(
                        "failed to create tokio runtime for extension activation: {err}"
                    ))
                })?;
                runtime.block_on(async move {
                    let request = crate::activation::ActivationRequest {
                        current: Some(extension_snapshot.clone()),
                        target_instance: None,
                        force: false,
                        mode: crate::activation::ActivationMode::Commit,
                    };
                    let mut candidate = pipeline.run(request, &manager).await?;
                    manager.publish_snapshot(candidate.snapshot.clone())?;
                    candidate.commit();
                    Ok::<(), RuntimeError>(())
                })
            })
            .join()
            .unwrap_or_else(|_| {
                Err(RuntimeError::Builder(
                    "extension activation thread panicked".to_string(),
                ))
            });
            if let Err(err) = pipeline_result {
                panic!("failed to initialize runtime host extensions: {err}");
            }
        }

        Self {
            workspace_root,
            config,
            extension_manager,
            session_registry: Arc::new(Mutex::new(HashMap::new())),
            event_bus,
            artifact_store,
            approval_broker,
            extension_source: discovery_source,
            builder,
        }
    }
}

#[async_trait::async_trait]
impl crate::control::HostControl for RuntimeHost {
    async fn spawn_session(
        &self,
        session_id: &str,
        config_override: Option<crate::config::RuntimeConfig>,
    ) -> Result<String> {
        let mut runtimes = self.session_registry.lock().unwrap();
        if runtimes.contains_key(session_id) {
            return Err(RuntimeError::Orchestration(format!(
                "Session already exists: {}",
                session_id
            )));
        }

        let mut session_builder = self.builder.clone();
        if let Some(mut config) = config_override {
            // Per-session overrides must not change workspace root, extension discovery roots, extension instances, grants, direct MCP configuration, or package trust decisions.
            config.workspace_root = self.config.workspace_root.clone();
            config.extension_instances = self.config.extension_instances.clone();
            config.mcp_servers = self.config.mcp_servers.clone();
            config.trusted_extension_ids = self.config.trusted_extension_ids.clone();
            config.trusted_extension_pins = self.config.trusted_extension_pins.clone();
            config.extension_timeouts = self.config.extension_timeouts.clone();
            config.extension_limits = self.config.extension_limits.clone();
            session_builder = session_builder.config(config);
        }

        session_builder.event_bus = self.event_bus.clone();
        session_builder.extension_manager = Some(self.extension_manager.clone());
        session_builder.approval = Some(self.approval_broker.clone());

        let runtime = session_builder.build()?;
        runtimes.insert(session_id.to_string(), Arc::new(runtime));

        self.event_bus.publish(RuntimeEvent::SessionSpawned {
            session_id: session_id.to_string(),
        });

        Ok(session_id.to_string())
    }

    async fn send_message(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<gestalt_core::session::RunResult> {
        let runtime = {
            let runtimes = self.session_registry.lock().unwrap();
            runtimes.get(session_id).cloned().ok_or_else(|| {
                RuntimeError::Orchestration(format!("Session not found: {}", session_id))
            })?
        };

        let input = crate::runtime::UserInput {
            prompt: prompt.to_string(),
            session_id: Some(session_id.to_string()),
            cancel_token: gestalt_core::cancel::CancelToken::new(),
            event_tx: None,
            artifact_dir: None,
        };

        let result = runtime.run_prompt(input).await?;
        Ok(result)
    }

    async fn enqueue_steering_message(
        &self,
        session_id: &str,
        content: &str,
        source: gestalt_core::session_queue::MessageSource,
        idempotency_key: Option<String>,
    ) -> Result<gestalt_core::session_queue::QueueAck> {
        let runtime = {
            let runtimes = self.session_registry.lock().unwrap();
            runtimes.get(session_id).cloned().ok_or_else(|| {
                RuntimeError::Orchestration(format!("Session not found: {}", session_id))
            })?
        };

        runtime
            .enqueue_message(
                session_id.to_string(),
                content.to_string(),
                source,
                idempotency_key,
            )
            .await
    }

    fn subscribe(&self) -> broadcast::Receiver<Arc<RuntimeEvent>> {
        self.event_bus.subscribe()
    }

    fn artifact_store(&self) -> Arc<dyn ArtifactStore> {
        self.artifact_store.clone()
    }

    async fn create_artifact(
        &self,
        session_id: &str,
        name: &str,
        content: &[u8],
    ) -> Result<String> {
        let uri = self
            .artifact_store
            .put_artifact(session_id, name, content)?;
        self.event_bus.publish(RuntimeEvent::ArtifactRouted {
            session_id: session_id.to_string(),
            path: uri.clone(),
            size_bytes: content.len(),
        });
        Ok(uri)
    }

    async fn read_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>> {
        self.artifact_store.get_artifact(session_id, name)
    }

    async fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>> {
        self.artifact_store.list_artifacts(session_id)
    }

    async fn respond_to_approval(
        &self,
        approval_id: &str,
        decision: gestalt_core::approval::ApprovalDecision,
    ) -> Result<()> {
        self.approval_broker.respond(approval_id, decision)
    }
}

#[async_trait::async_trait]
impl crate::control::RuntimeControl for RuntimeHost {
    async fn inspect_runtime(&self) -> crate::inspect::RuntimeInspect {
        let runtimes = self.session_registry.lock().unwrap();
        if let Some(runtime) = runtimes.values().next() {
            runtime.inspect()
        } else if let Ok(runtime) = self.builder.clone().build() {
            runtime.inspect()
        } else {
            crate::inspect::RuntimeInspect::default()
        }
    }

    async fn reload_extensions(
        &self,
        request: crate::control::ReloadExtensionsRequest,
    ) -> Result<crate::control::ReloadExtensionsReport> {
        let active = self.extension_manager.active_snapshot();
        let mut discovered = self.extension_source.discover_packages()?;
        crate::extension::apply_trust_decisions(
            &mut discovered,
            &self.config.trusted_extension_ids,
            &self.config.trusted_extension_pins,
        );
        let pipeline = crate::activation::ExtensionActivationPipeline {
            discovery: Arc::new(crate::activation::StaticExtensionSource::new(discovered)),
            launcher: self.extension_manager.launcher.clone(),
            base_composition: Arc::new(crate::activation::BaseRuntimeComposition {
                tool_catalog: self.builder.tools.clone().unwrap_or_else(|| {
                    Arc::new(
                        crate::tool_catalog::ComposedToolCatalog::new(
                            Arc::new(EmptyToolCatalog),
                            std::collections::BTreeMap::new(),
                        )
                        .unwrap(),
                    )
                }),
                #[cfg(feature = "mcp")]
                mcp_registry: self.builder.mcp_registry.clone().unwrap_or_else(|| {
                    Arc::new(crate::mcp::McpRegistry::new(
                        self.workspace_root.clone(),
                        std::collections::HashMap::new(),
                    ))
                }),
                base_registry: self.builder.registry.snapshot(),
            }),
            host_context: self.extension_manager.host_context.clone(),
        };

        let mut candidate = pipeline
            .run(
                crate::activation::ActivationRequest {
                    current: Some(active.clone()),
                    target_instance: request.instance_id.clone(),
                    force: request.force,
                    mode: if request.dry_run {
                        crate::activation::ActivationMode::DryRun
                    } else {
                        crate::activation::ActivationMode::Commit
                    },
                },
                &self.extension_manager,
            )
            .await?;

        let report = crate::control::ReloadExtensionsReport {
            previous_generation: active.generation,
            candidate_generation: candidate.snapshot.generation,
            candidate_fingerprint: candidate.snapshot.fingerprint.clone(),
            published: !request.dry_run,
            validation_errors: Vec::new(),
        };

        if !request.dry_run {
            self.extension_manager
                .publish_snapshot(candidate.snapshot.clone())?;
            candidate.commit();
        }

        Ok(report)
    }

    fn current_generation(&self) -> crate::extension::RuntimeGeneration {
        self.extension_manager.current_generation()
    }

    fn extension_health(&self) -> Vec<crate::extension::ExtensionInstanceHealth> {
        self.extension_manager
            .combined_health(&self.extension_manager.active_snapshot())
    }
}

#[async_trait::async_trait]
pub trait AgentRuntimeHandle: crate::control::RuntimeControl + crate::control::HostControl {}

pub struct DefaultAgentRuntimeHandle {
    host: Arc<RuntimeHost>,
}

impl DefaultAgentRuntimeHandle {
    pub fn new(builder: AgentRuntimeBuilder, artifact_store: Arc<dyn ArtifactStore>) -> Self {
        Self {
            host: Arc::new(RuntimeHost::new(builder, artifact_store)),
        }
    }
}

#[async_trait::async_trait]
impl crate::control::HostControl for DefaultAgentRuntimeHandle {
    async fn spawn_session(
        &self,
        session_id: &str,
        config_override: Option<crate::config::RuntimeConfig>,
    ) -> Result<String> {
        self.host.spawn_session(session_id, config_override).await
    }

    async fn send_message(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<gestalt_core::session::RunResult> {
        self.host.send_message(session_id, prompt).await
    }

    async fn enqueue_steering_message(
        &self,
        session_id: &str,
        content: &str,
        source: gestalt_core::session_queue::MessageSource,
        idempotency_key: Option<String>,
    ) -> Result<gestalt_core::session_queue::QueueAck> {
        self.host
            .enqueue_steering_message(session_id, content, source, idempotency_key)
            .await
    }

    fn subscribe(&self) -> broadcast::Receiver<Arc<RuntimeEvent>> {
        self.host.subscribe()
    }

    fn artifact_store(&self) -> Arc<dyn ArtifactStore> {
        self.host.artifact_store()
    }

    async fn create_artifact(
        &self,
        session_id: &str,
        name: &str,
        content: &[u8],
    ) -> Result<String> {
        self.host.create_artifact(session_id, name, content).await
    }

    async fn read_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>> {
        self.host.read_artifact(session_id, name).await
    }

    async fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>> {
        self.host.list_artifacts(session_id).await
    }

    async fn respond_to_approval(
        &self,
        approval_id: &str,
        decision: gestalt_core::approval::ApprovalDecision,
    ) -> Result<()> {
        self.host.respond_to_approval(approval_id, decision).await
    }
}

#[async_trait::async_trait]
impl crate::control::RuntimeControl for DefaultAgentRuntimeHandle {
    async fn inspect_runtime(&self) -> crate::inspect::RuntimeInspect {
        self.host.inspect_runtime().await
    }

    async fn reload_extensions(
        &self,
        request: crate::control::ReloadExtensionsRequest,
    ) -> Result<crate::control::ReloadExtensionsReport> {
        self.host.reload_extensions(request).await
    }

    fn current_generation(&self) -> crate::extension::RuntimeGeneration {
        self.host.current_generation()
    }

    fn extension_health(&self) -> Vec<crate::extension::ExtensionInstanceHealth> {
        self.host.extension_health()
    }
}

impl AgentRuntimeHandle for DefaultAgentRuntimeHandle {}
