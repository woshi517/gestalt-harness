use async_trait::async_trait;

use super::protocol::{
    CapabilityDescriptorV2, InitializeRequestV2, InitializeResponseV2, LifecycleInvokeRequestV2,
    LifecycleInvokeResponseV2,
};

#[async_trait]
pub trait LifecycleClient: Send + Sync {
    async fn initialize(&self, request: InitializeRequestV2)
        -> crate::Result<InitializeResponseV2>;

    async fn describe_capabilities(&self) -> crate::Result<Vec<CapabilityDescriptorV2>>;

    async fn invoke(
        &self,
        request: LifecycleInvokeRequestV2,
    ) -> crate::Result<LifecycleInvokeResponseV2>;

    async fn shutdown(&self) -> crate::Result<()>;
}
