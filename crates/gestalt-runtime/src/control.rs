use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::error::Result;
use crate::extension::{ExtensionInstanceHealth, RuntimeExtensionSnapshot, RuntimeGeneration};
use crate::inspect::RuntimeInspect;
use crate::registry::RuntimeFingerprint;
use crate::event_bus::RuntimeEvent;
use crate::artifact_store::ArtifactStore;

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

    // Orchestration methods moved from AgentRuntimeHandle
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
        let candidate_generation = RuntimeGeneration(active.generation.0 + 1);
        let candidate_fingerprint = RuntimeFingerprint(format!(
            "{}:generation:{}",
            active.fingerprint, candidate_generation.0
        ));
        let report = ReloadExtensionsReport {
            previous_generation: active.generation,
            candidate_generation,
            candidate_fingerprint: candidate_fingerprint.clone(),
            published: !request.dry_run,
            validation_errors: Vec::new(),
        };

        if !request.dry_run {
            let candidate = Arc::new(RuntimeExtensionSnapshot {
                generation: candidate_generation,
                fingerprint: candidate_fingerprint,
                registry_snapshot: active.registry_snapshot.clone(),
                tool_catalog: active.tool_catalog.clone(),
                context_plan: active.context_plan.clone(),
                policy_plan: active.policy_plan.clone(),
                routing_plan: active.routing_plan.clone(),
                verification_plan: active.verification_plan.clone(),
                observer_plan: active.observer_plan.clone(),
                mcp_registry: active.mcp_registry.clone(),
                process_instances: active.process_instances.clone(),
                package_health: active.package_health.clone(),
            });
            self.extension_manager.publish_snapshot(candidate)?;
        }

        Ok(report)
    }

    fn current_generation(&self) -> RuntimeGeneration {
        self.extension_manager.current_generation()
    }

    fn extension_health(&self) -> Vec<ExtensionInstanceHealth> {
        self.extension_manager
            .active_snapshot()
            .package_health
            .iter()
            .cloned()
            .collect()
    }

    async fn spawn_session(
        &self,
        _session_id: &str,
        _config_override: Option<crate::config::RuntimeConfig>,
    ) -> Result<String> {
        Err(crate::error::RuntimeError::Orchestration(
            "spawn_session is not supported on a single AgentRuntime".to_string()
        ))
    }

    async fn send_message(
        &self,
        _session_id: &str,
        prompt: &str,
    ) -> Result<gestalt_core::session::RunResult> {
        let input = crate::runtime::UserInput {
            prompt: prompt.to_string(),
            session_id: Some(_session_id.to_string()),
            cancel_token: gestalt_core::cancel::CancelToken::new(),
            event_tx: None,
            artifact_dir: None,
        };
        self.run_prompt(input).await
    }

    fn subscribe(&self) -> broadcast::Receiver<Arc<RuntimeEvent>> {
        self.event_bus.subscribe()
    }

    fn artifact_store(&self) -> Arc<dyn ArtifactStore> {
        panic!("artifact_store is not supported on a single AgentRuntime")
    }

    async fn create_artifact(&self, _session_id: &str, _name: &str, _content: &[u8]) -> Result<String> {
        Err(crate::error::RuntimeError::Orchestration(
            "create_artifact is not supported on a single AgentRuntime".to_string()
        ))
    }

    async fn read_artifact(&self, _session_id: &str, _name: &str) -> Result<Vec<u8>> {
        Err(crate::error::RuntimeError::Orchestration(
            "read_artifact is not supported on a single AgentRuntime".to_string()
        ))
    }

    async fn list_artifacts(&self, _session_id: &str) -> Result<Vec<String>> {
        Err(crate::error::RuntimeError::Orchestration(
            "list_artifacts is not supported on a single AgentRuntime".to_string()
        ))
    }

    async fn enqueue_steering_message(
        &self,
        session_id: &str,
        content: &str,
        source: gestalt_core::session_queue::MessageSource,
        idempotency_key: Option<String>,
    ) -> Result<gestalt_core::session_queue::QueueAck> {
        self.enqueue_message(session_id.to_string(), content.to_string(), source, idempotency_key).await
    }
}
