use crate::registry::RuntimeRegistry;
use crate::error::Result;

pub trait GestaltExtension: Send + Sync {
    fn name(&self) -> &str;
    fn register(&self, registry: &mut RuntimeRegistry) -> Result<()>;
}
