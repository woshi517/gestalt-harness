use gestalt_core::tool::{Tool, ToolCatalog, ToolSchema};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ComposedToolCatalog {
    base: Arc<dyn ToolCatalog>,
    extension_tools: BTreeMap<String, Arc<dyn Tool>>,
    planner: Option<crate::tool_catalog_planner::ToolCatalogPlanner>,
}

impl ComposedToolCatalog {
    pub fn new(
        base: Arc<dyn ToolCatalog>,
        extension_tools: BTreeMap<String, Arc<dyn Tool>>,
    ) -> Result<Self, String> {
        // Built-in and extension tools no longer collide on flat name check since canonical IDs differ
        Ok(Self {
            base,
            extension_tools,
            planner: None,
        })
    }

    pub fn with_planner(
        mut self,
        planner: crate::tool_catalog_planner::ToolCatalogPlanner,
    ) -> Self {
        self.planner = Some(planner);
        self
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

    fn get_by_id(
        &self,
        id: &gestalt_core::tool_descriptor::CanonicalToolId,
    ) -> Option<Arc<dyn Tool>> {
        match &id.namespace {
            gestalt_core::tool_descriptor::ToolNamespace::BuiltIn => self.base.get_by_id(id),
            gestalt_core::tool_descriptor::ToolNamespace::Extension(_)
            | gestalt_core::tool_descriptor::ToolNamespace::Mcp(_) => self
                .extension_tools
                .values()
                .find(|tool| tool.descriptor().id == *id)
                .cloned(),
        }
    }

    fn descriptors(&self) -> Vec<gestalt_core::tool_descriptor::ToolDescriptor> {
        let mut descs = self.base.descriptors();
        for tool in self.extension_tools.values() {
            descs.push(tool.descriptor());
        }
        if let Some(ref planner) = self.planner {
            descs = planner.plan_descriptors(descs);
        }
        // Deterministic ordering by canonical ID string
        descs.sort_by(|a, b| a.id.to_string().cmp(&b.id.to_string()));
        descs
    }
}
