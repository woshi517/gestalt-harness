use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{Result, RuntimeError};

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
        Err(RuntimeError::Extension(format!(
            "Local process launch is not wired through ExtensionManager yet for component '{}'",
            component.id.canonical_id()
        )))
    }
}
