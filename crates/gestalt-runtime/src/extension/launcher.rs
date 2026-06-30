use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{Result, RuntimeError};
use crate::process_extension::ProcessExtensionBroker;

use super::{ExtensionProcessInstance, ExtensionRuntimeComponent};

use crate::activation::HostLaunchContext;

#[async_trait]
pub trait ExtensionLauncher: Send + Sync {
    async fn launch(
        &self,
        component: &ExtensionRuntimeComponent,
        host_context: &HostLaunchContext,
    ) -> Result<Arc<ExtensionProcessInstance>>;
}

pub struct NoopExtensionLauncher;

#[async_trait]
impl ExtensionLauncher for NoopExtensionLauncher {
    async fn launch(
        &self,
        component: &ExtensionRuntimeComponent,
        _host_context: &HostLaunchContext,
    ) -> Result<Arc<ExtensionProcessInstance>> {
        Err(RuntimeError::Extension(format!(
            "No extension launcher configured for component '{}'",
            component.id.canonical_id()
        )))
    }
}

pub struct LocalProcessLauncher;

#[async_trait]
impl ExtensionLauncher for LocalProcessLauncher {
    async fn launch(
        &self,
        component: &ExtensionRuntimeComponent,
        host_context: &HostLaunchContext,
    ) -> Result<Arc<ExtensionProcessInstance>> {
        let timeouts = crate::config::ExtensionTimeoutsConfig {
            initialize_ms: Some(host_context.timeout_initialize_ms),
            hook_ms: Some(host_context.timeout_hook_ms),
            context_ms: Some(host_context.timeout_context_ms),
            tool_ms: Some(host_context.timeout_tool_ms),
            shutdown_ms: Some(host_context.timeout_shutdown_ms),
        };
        let limits = crate::config::ExtensionLimitsConfig {
            max_message_bytes: Some(host_context.max_message_bytes),
            max_pending_requests: Some(host_context.max_pending_requests),
            max_protocol_errors: None,
        };

        let broker = Arc::new(
            ProcessExtensionBroker::spawn_with_component(
                component.clone(),
                host_context.event_bus.clone(),
                timeouts,
                limits,
                component.trust.is_trusted(),
            )
            .await?,
        );
        let process = Arc::new(ExtensionProcessInstance::with_broker(
            component.id.canonical_id(),
            broker,
        ));
        process.transition_to(super::ExtensionProcessState::Ready);
        Ok(process)
    }
}
