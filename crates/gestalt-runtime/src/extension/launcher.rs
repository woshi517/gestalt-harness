use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{Result, RuntimeError};
use crate::manifest::{Capabilities, Entrypoint, ExtensionManifest, Permissions};
use crate::process_extension::ProcessExtensionBroker;
use crate::RuntimeEventBus;

use super::{ExtensionProcessInstance, ExtensionRuntimeComponent};

#[async_trait]
pub trait ExtensionLauncher: Send + Sync {
    async fn launch(
        &self,
        component: &ExtensionRuntimeComponent,
    ) -> Result<Arc<ExtensionProcessInstance>>;
}

pub struct NoopExtensionLauncher;

#[async_trait]
impl ExtensionLauncher for NoopExtensionLauncher {
    async fn launch(
        &self,
        component: &ExtensionRuntimeComponent,
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
    ) -> Result<Arc<ExtensionProcessInstance>> {
        let manifest = ExtensionManifest {
            id: component.id.package_id.clone(),
            name: component.id.canonical_id(),
            version: "0.0.0".to_string(),
            manifest_version: Some("1".to_string()),
            protocol_version: Some(
                component
                    .protocol_fingerprint
                    .clone()
                    .unwrap_or_else(|| "2.0".to_string()),
            ),
            runtime: "stdio".to_string(),
            entrypoint: Entrypoint {
                command: component.entrypoint_command.clone(),
                args: component.entrypoint_args.clone(),
            },
            capabilities: match component.kind {
                super::ComponentKind::CommandTool => Capabilities {
                    tools: true,
                    ..Default::default()
                },
                _ => Capabilities::default(),
            },
            permissions: Permissions {
                allow_shell: true,
                allow_workspace_read: true,
                ..Default::default()
            },
            tools: Vec::new(),
            hooks: Vec::new(),
            context_injectors: Vec::new(),
        };
        let broker = Arc::new(
            ProcessExtensionBroker::spawn(
                manifest,
                RuntimeEventBus::new(),
                Default::default(),
                Default::default(),
                component.trust_fingerprint == "true",
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
