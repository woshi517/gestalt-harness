use gestalt_core::tool::{Tool, ToolCatalog, ToolSchema};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ComposedToolCatalog {
    base: Arc<dyn ToolCatalog>,
    extension_tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ComposedToolCatalog {
    pub fn new(
        base: Arc<dyn ToolCatalog>,
        extension_tools: BTreeMap<String, Arc<dyn Tool>>,
    ) -> Result<Self, String> {
        let base_schemas = base.schemas();
        for schema in &base_schemas {
            let name = schema.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if extension_tools.contains_key(name) {
                return Err(format!(
                    "Duplicate tool name colliding with base tool: {}",
                    name
                ));
            }
        }
        Ok(Self {
            base,
            extension_tools,
        })
    }
}

impl ToolCatalog for ComposedToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        let mut schemas = self.base.schemas();
        for tool in self.extension_tools.values() {
            schemas.push(tool.schema());
        }
        // Preserve deterministic schema ordering: sort alphabetically by name
        schemas.sort_by(|a, b| {
            let name_a = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let name_b = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
            name_a.cmp(name_b)
        });
        schemas
    }

    fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if let Some(tool) = self.extension_tools.get(name) {
            Some(tool.clone())
        } else {
            self.base.get(name)
        }
    }
}
