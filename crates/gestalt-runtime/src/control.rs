use async_trait::async_trait;
use std::sync::Arc;

use crate::error::Result;
use crate::extension::{ExtensionInstanceHealth, RuntimeExtensionSnapshot, RuntimeGeneration};
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
}
