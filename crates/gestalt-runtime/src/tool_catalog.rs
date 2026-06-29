use gestalt_core::tool::{Tool, ToolCatalog, ToolSchema};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ComposedToolCatalog {
    base: Arc<dyn ToolCatalog>,
    extension_tools: BTreeMap<String, Arc<dyn Tool>>,
    planner: Option<crate::tool_catalog_planner::ToolCatalogPlanner>,
    #[cfg(feature = "mcp")]
    mcp_registry: Option<Arc<crate::mcp::McpRegistry>>,
    event_bus: Option<crate::event_bus::RuntimeEventBus>,
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
            #[cfg(feature = "mcp")]
            mcp_registry: None,
            event_bus: None,
        })
    }

    pub fn with_planner(
        mut self,
        planner: crate::tool_catalog_planner::ToolCatalogPlanner,
    ) -> Self {
        self.planner = Some(planner);
        self
    }

    #[cfg(feature = "mcp")]
    pub fn with_mcp(mut self, mcp_registry: Arc<crate::mcp::McpRegistry>) -> Self {
        self.mcp_registry = Some(mcp_registry);
        self
    }

    pub fn with_event_bus(mut self, event_bus: crate::event_bus::RuntimeEventBus) -> Self {
        self.event_bus = Some(event_bus);
        self
    }
}

impl ToolCatalog for ComposedToolCatalog {
    fn schemas(&self) -> Vec<ToolSchema> {
        let mut schemas = self.base.schemas();
        for tool in self.extension_tools.values() {
            schemas.push(tool.schema());
        }

        // Dynamic MCP schemas
        #[cfg(feature = "mcp")]
        if let Some(ref mcp_reg) = self.mcp_registry {
            let cached = mcp_reg.get_cached_tools();
            for (server_id, schema) in cached {
                let trust_level = mcp_reg.get_server_trust_level(&server_id.0);
                let mcp_tool = crate::mcp::McpBackedTool::new(
                    mcp_reg.clone(),
                    server_id.0.clone(),
                    schema.name.clone(),
                    schema.description.clone(),
                    schema.input_schema.clone(),
                    trust_level.as_deref(),
                    self.event_bus.clone(),
                );
                schemas.push(mcp_tool.schema());
            }
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
            return Some(tool.clone());
        }

        // Check if name is a canonical ID string
        if let Ok(id) = name.parse::<gestalt_core::tool_descriptor::CanonicalToolId>() {
            if let Some(tool) = self.get_by_id(&id) {
                return Some(tool);
            }
        }

        self.base.get(name)
    }

    fn get_by_id(
        &self,
        id: &gestalt_core::tool_descriptor::CanonicalToolId,
    ) -> Option<Arc<dyn Tool>> {
        match &id.namespace {
            gestalt_core::tool_descriptor::ToolNamespace::BuiltIn => self.base.get_by_id(id),
            gestalt_core::tool_descriptor::ToolNamespace::Extension(_) => self
                .extension_tools
                .values()
                .find(|tool| tool.descriptor().id == *id)
                .cloned(),
            gestalt_core::tool_descriptor::ToolNamespace::Mcp(server_name) => {
                #[cfg(feature = "mcp")]
                if let Some(ref mcp_reg) = self.mcp_registry {
                    if let Some(schema) = mcp_reg.get_cached_tool(server_name, &id.name) {
                        let trust_level = mcp_reg.get_server_trust_level(server_name);
                        let mcp_tool = crate::mcp::McpBackedTool::new(
                            mcp_reg.clone(),
                            server_name.clone(),
                            id.name.clone(),
                            schema.description,
                            schema.input_schema,
                            trust_level.as_deref(),
                            self.event_bus.clone(),
                        );
                        return Some(Arc::new(mcp_tool));
                    }
                }
                None
            }
        }
    }

    fn descriptors(&self) -> Vec<gestalt_core::tool_descriptor::ToolDescriptor> {
        let mut descs = self.base.descriptors();
        for tool in self.extension_tools.values() {
            descs.push(tool.descriptor());
        }

        // Dynamic MCP descriptors
        #[cfg(feature = "mcp")]
        if let Some(ref mcp_reg) = self.mcp_registry {
            let cached = mcp_reg.get_cached_tools();
            for (server_id, schema) in cached {
                let trust_level = mcp_reg.get_server_trust_level(&server_id.0);
                let mcp_tool = crate::mcp::McpBackedTool::new(
                    mcp_reg.clone(),
                    server_id.0.clone(),
                    schema.name.clone(),
                    schema.description.clone(),
                    schema.input_schema.clone(),
                    trust_level.as_deref(),
                    self.event_bus.clone(),
                );
                descs.push(mcp_tool.descriptor());
            }
        }

        if let Some(ref planner) = self.planner {
            descs = planner.plan_descriptors(descs);
        }
        // Deterministic ordering by canonical ID string
        descs.sort_by_key(|a| a.id.to_string());
        descs
    }
}
