mod bash;
mod common;
mod find_files;
mod patch;
mod read;
mod search;
#[cfg(test)]
mod test_support;
mod web_fetch;
mod write;

pub use bash::{BashInput, BashTool};
pub use find_files::{FindFilesInput, FindFilesTool};
pub use patch::{PatchInput, PatchTool};
pub use read::{ReadInput, ReadTool};
pub use search::{SearchInput, SearchTool};
pub use web_fetch::{WebFetchInput, WebFetchTool};
pub use write::{WriteInput, WriteTool};

use std::sync::Arc;

use gestalt_core::ToolError;

pub fn default_registry() -> Result<crate::ToolRegistry, ToolError> {
    let mut registry = crate::ToolRegistry::new();
    registry.register(Arc::new(ReadTool))?;
    registry.register(Arc::new(SearchTool))?;
    registry.register(Arc::new(FindFilesTool))?;
    registry.register(Arc::new(WriteTool))?;
    registry.register(Arc::new(PatchTool))?;
    registry.register(Arc::new(BashTool::default()))?;
    registry.register(Arc::new(WebFetchTool::default()))?;
    Ok(registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gestalt_core::ToolCatalog;

    #[test]
    fn schemas_should_include_public_contracts() {
        let registry = default_registry().expect("registry builds");
        let schemas = registry.schemas();

        assert_eq!(schemas.len(), 7);
    }
}
