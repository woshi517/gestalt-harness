use std::sync::Arc;

use async_trait::async_trait;

use crate::error::{Result, RuntimeError};
use crate::extension::{ExtensionManager, ExtensionRuntimeComponent};

use super::client::LifecycleClient;
use super::protocol::{
    CapabilityDescriptorV2, InitializeRequestV2, InitializeResponseV2, LifecycleInvokeRequestV2,
    LifecycleInvokeResponseV2, PROTOCOL_V2_METHOD_DESCRIBE_CAPABILITIES, PROTOCOL_V2_METHOD_INVOKE,
};

pub struct ProcessLifecycleClient {
    manager: Arc<ExtensionManager>,
    component: ExtensionRuntimeComponent,
}

impl ProcessLifecycleClient {
    pub fn new(manager: Arc<ExtensionManager>, component: ExtensionRuntimeComponent) -> Self {
        Self { manager, component }
    }

    async fn process(&self) -> Result<Arc<crate::extension::ExtensionProcessInstance>> {
        self.manager.launch_process(&self.component).await
    }

    async fn broker(&self) -> Result<Arc<crate::process_extension::ProcessExtensionBroker>> {
        self.process()
            .await?
            .broker()
            .ok_or_else(|| RuntimeError::Extension("process instance has no broker".to_string()))
    }
}

#[async_trait]
impl LifecycleClient for ProcessLifecycleClient {
    async fn initialize(&self, request: InitializeRequestV2) -> Result<InitializeResponseV2> {
        let broker = self.broker().await?;
        let negotiated_version = broker.negotiated_version();
        if !request
            .supported_versions
            .iter()
            .any(|version| version == &negotiated_version)
        {
            return Err(RuntimeError::Extension(format!(
                "unsupported negotiated lifecycle protocol version '{}'",
                negotiated_version
            )));
        }
        Ok(InitializeResponseV2 { negotiated_version })
    }

    async fn describe_capabilities(&self) -> Result<Vec<CapabilityDescriptorV2>> {
        let process = self.process().await?;
        let _guard = process.begin_call()?;
        let value = self
            .broker()
            .await?
            .call(PROTOCOL_V2_METHOD_DESCRIBE_CAPABILITIES, None)
            .await
            .map_err(RuntimeError::Extension)?;
        serde_json::from_value(value)
            .map_err(|err| RuntimeError::Extension(format!("invalid capability payload: {err}")))
    }

    async fn invoke(&self, request: LifecycleInvokeRequestV2) -> Result<LifecycleInvokeResponseV2> {
        let process = self.process().await?;
        let _guard = process.begin_call()?;
        let params = serde_json::to_value(request).map_err(|err| {
            RuntimeError::Extension(format!("failed to serialize lifecycle request: {err}"))
        })?;
        let value = self
            .broker()
            .await?
            .call(PROTOCOL_V2_METHOD_INVOKE, Some(params))
            .await
            .map_err(RuntimeError::Extension)?;
        serde_json::from_value(value)
            .map_err(|err| RuntimeError::Extension(format!("invalid lifecycle response: {err}")))
    }

    async fn shutdown(&self) -> Result<()> {
        self.manager.shutdown_process(&self.component).await
    }
}
