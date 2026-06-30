use crate::error::Result;
use crate::registry::RuntimeRegistryBuilder;

pub trait RuntimeModule: Send + Sync {
    fn id(&self) -> &str;
    fn register(&self, registry: &mut RuntimeRegistryBuilder) -> Result<()>;
}
