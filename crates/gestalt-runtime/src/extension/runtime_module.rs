use crate::error::Result;
use crate::registry::{RuntimeRegistry, RuntimeRegistryBuilder};

pub trait RuntimeModule: Send + Sync {
    fn id(&self) -> &str;
    fn register(&self, registry: &mut RuntimeRegistryBuilder) -> Result<()>;
}

#[deprecated(note = "use RuntimeModule for trusted in-process runtime modules")]
pub trait GestaltExtension: Send + Sync {
    fn name(&self) -> &str;
    fn register(&self, registry: &mut RuntimeRegistry) -> Result<()>;
    fn as_process_extension(&self) -> Option<&crate::process_extension::ProcessExtension> {
        None
    }
}

impl<T> RuntimeModule for T
where
    T: GestaltExtension + ?Sized,
{
    fn id(&self) -> &str {
        self.name()
    }

    fn register(&self, registry: &mut RuntimeRegistryBuilder) -> Result<()> {
        GestaltExtension::register(self, registry)
    }
}
