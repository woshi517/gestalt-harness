use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

pub mod contract;
mod host;

pub use host::{
    ControlHostOptions, LocalControlHost, MockControlHost, DEFAULT_CONTROL_QUEUE_CAPACITY,
    MAX_ARTIFACT_READ_BYTES,
};

use crate::artifact_store::ArtifactStore;
use crate::error::Result;
use crate::event_bus::RuntimeEvent;
use crate::extension::{ExtensionInstanceHealth, RuntimeGeneration};
use crate::inspect::RuntimeInspect;
use crate::registry::RuntimeFingerprint;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReloadExtensionsRequest {
    pub dry_run: bool,
    pub force: bool,
    pub instance_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadExtensionsReport {
    pub previous_generation: RuntimeGeneration,
    pub candidate_generation: RuntimeGeneration,
    pub candidate_fingerprint: RuntimeFingerprint,
    pub published: bool,
    pub validation_errors: Vec<String>,
    pub diagnostics: Vec<crate::activation::ActivationDiagnostic>,
}

#[async_trait]
pub trait RuntimeControl: Send + Sync {
    async fn inspect_runtime(&self) -> RuntimeInspect;
    async fn reload_extensions(
        &self,
        request: ReloadExtensionsRequest,
    ) -> Result<ReloadExtensionsReport>;
    fn current_generation(&self) -> RuntimeGeneration;
    fn extension_health(&self) -> Vec<ExtensionInstanceHealth>;
}

#[async_trait]
pub trait HostControl: Send + Sync {
    async fn spawn_session(
        &self,
        session_id: &str,
        config_override: Option<crate::config::RuntimeConfig>,
    ) -> Result<String>;
    async fn send_message(
        &self,
        session_id: &str,
        prompt: &str,
    ) -> Result<gestalt_core::session::RunResult>;
    fn subscribe(&self) -> broadcast::Receiver<Arc<RuntimeEvent>>;
    fn artifact_store(&self) -> Arc<dyn ArtifactStore>;
    async fn create_artifact(&self, session_id: &str, name: &str, content: &[u8])
        -> Result<String>;
    async fn read_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>>;
    async fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>>;
    async fn enqueue_steering_message(
        &self,
        session_id: &str,
        content: &str,
        source: gestalt_core::session_queue::MessageSource,
        idempotency_key: Option<String>,
    ) -> Result<gestalt_core::session_queue::QueueAck>;
    async fn respond_to_approval(
        &self,
        approval_id: &str,
        decision: gestalt_core::approval::ApprovalDecision,
    ) -> Result<()>;
}

#[async_trait]
impl RuntimeControl for crate::runtime::AgentRuntime {
    async fn inspect_runtime(&self) -> RuntimeInspect {
        self.inspect()
    }

    async fn reload_extensions(
        &self,
        request: ReloadExtensionsRequest,
    ) -> Result<ReloadExtensionsReport> {
        let _guard = self.extension_manager.reload_mutex.lock().await;
        let active = self.extension_manager.active_snapshot();
        let pipeline = crate::activation::ExtensionActivationPipeline {
            discovery: Arc::new(crate::activation::StaticExtensionSource::new(
                active.resolved_packages.iter().cloned().collect(),
            )),
            launcher: self.extension_manager.launcher.clone(),
            base_composition: Arc::new(crate::activation::BaseRuntimeComposition {
                tool_catalog: self.tools.clone(),
                #[cfg(feature = "mcp")]
                mcp_registry: self.mcp_registry.clone(),
                base_registry: self.registry_snapshot.clone(),
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
        let diagnostics = candidate.diagnostics.clone();
        let report = ReloadExtensionsReport {
            previous_generation: active.generation,
            candidate_generation: candidate.snapshot.generation,
            candidate_fingerprint: candidate.snapshot.fingerprint.clone(),
            published: !request.dry_run,
            validation_errors: diagnostics
                .iter()
                .map(|diag| diag.message.clone())
                .collect(),
            diagnostics,
        };

        if !request.dry_run {
            self.extension_manager
                .publish_snapshot(candidate.snapshot.clone())?;
            candidate.commit();
        }

        Ok(report)
    }

    fn current_generation(&self) -> RuntimeGeneration {
        self.extension_manager.current_generation()
    }

    fn extension_health(&self) -> Vec<ExtensionInstanceHealth> {
        self.extension_manager
            .combined_health(&self.extension_manager.active_snapshot())
    }
}
