use crate::error::Result;
use crate::registry::RuntimeRegistry;

pub trait GestaltExtension: Send + Sync {
    fn name(&self) -> &str;
    fn register(&self, registry: &mut RuntimeRegistry) -> Result<()>;
    fn as_process_extension(&self) -> Option<&crate::process_extension::ProcessExtension> {
        None
    }
}
