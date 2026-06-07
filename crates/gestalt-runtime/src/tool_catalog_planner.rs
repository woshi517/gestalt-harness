use gestalt_core::tool::ToolCatalog;
use gestalt_core::tool_descriptor::ToolDescriptor;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolProfile {
    #[default]
    All,
    BuiltInOnly,
    Selected { names: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct ToolCatalogPlanner {
    pub profile: ToolProfile,
}

impl ToolCatalogPlanner {
    pub fn new(profile: ToolProfile) -> Self {
        Self { profile }
    }

    pub fn plan(&self, catalog: &dyn ToolCatalog) -> Vec<ToolDescriptor> {
        let mut all_descriptors = catalog.descriptors();
        // Ensure deterministic ordering by canonical string representation of ID
        all_descriptors.sort_by_key(|a| a.id.to_string());

        self.plan_descriptors(all_descriptors)
    }

    pub fn plan_descriptors(&self, mut descs: Vec<ToolDescriptor>) -> Vec<ToolDescriptor> {
        descs.sort_by_key(|a| a.id.to_string());
        match &self.profile {
            ToolProfile::All => descs,
            ToolProfile::BuiltInOnly => {
                descs
                    .into_iter()
                    .filter(|desc| matches!(desc.id.namespace, gestalt_core::tool_descriptor::ToolNamespace::BuiltIn))
                    .collect()
            }
            ToolProfile::Selected { names } => {
                descs
                    .into_iter()
                    .filter(|desc| {
                        names.contains(&desc.id.name) || names.contains(&desc.id.to_string())
                    })
                    .collect()
            }
        }
    }
}
