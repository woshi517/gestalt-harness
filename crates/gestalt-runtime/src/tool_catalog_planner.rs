use gestalt_core::tool::ToolCatalog;
use gestalt_core::tool_descriptor::ToolDescriptor;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolProfile {
    #[default]
    All,
    BuiltInOnly,
    Selected {
        names: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct ToolCatalogPlanner {
    pub profile: ToolProfile,
    /// Additional skill-scoped tool name filter. Applied as intersection
    /// after the base profile filter.
    pub skill_allowed_names: Option<Vec<String>>,
    pub skill_state: Option<Arc<Mutex<crate::skill_contributor::SkillContributorState>>>,
    pub mcp_discovery_threshold: Option<usize>,
    pub mcp_discovery_state: Option<Arc<Mutex<crate::mcp_discovery::McpDiscoveryState>>>,
    pub mcp_registry: Option<Arc<crate::legacy_mcp::McpRegistry>>,
}

impl ToolCatalogPlanner {
    pub fn new(profile: ToolProfile) -> Self {
        Self {
            profile,
            skill_allowed_names: None,
            skill_state: None,
            mcp_discovery_threshold: None,
            mcp_discovery_state: None,
            mcp_registry: None,
        }
    }

    pub fn with_skill_filter(mut self, allowed_names: Vec<String>) -> Self {
        self.skill_allowed_names = Some(allowed_names);
        self
    }

    pub fn with_skill_state(
        mut self,
        state: Arc<Mutex<crate::skill_contributor::SkillContributorState>>,
    ) -> Self {
        self.skill_state = Some(state);
        self
    }

    pub fn with_mcp(
        mut self,
        threshold: Option<usize>,
        state: Arc<Mutex<crate::mcp_discovery::McpDiscoveryState>>,
        registry: Arc<crate::legacy_mcp::McpRegistry>,
    ) -> Self {
        self.mcp_discovery_threshold = threshold;
        self.mcp_discovery_state = Some(state);
        self.mcp_registry = Some(registry);
        self
    }

    pub fn plan(&self, catalog: &dyn ToolCatalog) -> Vec<ToolDescriptor> {
        let mut all_descriptors = catalog.descriptors();
        // Ensure deterministic ordering by canonical string representation of ID
        all_descriptors.sort_by_key(|a| a.id.to_string());

        self.plan_descriptors(all_descriptors)
    }

    pub fn plan_descriptors(&self, mut descs: Vec<ToolDescriptor>) -> Vec<ToolDescriptor> {
        descs.sort_by_key(|a| a.id.to_string());
        let filtered: Vec<ToolDescriptor> = match &self.profile {
            ToolProfile::All => descs,
            ToolProfile::BuiltInOnly => descs
                .into_iter()
                .filter(|desc| {
                    matches!(
                        desc.id.namespace,
                        gestalt_core::tool_descriptor::ToolNamespace::BuiltIn
                    )
                })
                .collect(),
            ToolProfile::Selected { names } => descs
                .into_iter()
                .filter(|desc| {
                    names.contains(&desc.id.name) || names.contains(&desc.id.to_string())
                })
                .collect(),
        };

        let dynamic_allowed = self.skill_state.as_ref().and_then(|state| {
            let guard = state.lock().ok()?;
            let active = guard.active_descriptors();
            let policy = crate::legacy_skills::effective_tool_policy(&active);
            if policy.restricts_tools {
                let mut allowed = policy.allowed_tool_names.into_iter().collect::<Vec<_>>();
                allowed.sort();
                Some(allowed)
            } else {
                None
            }
        });
        let allowed = dynamic_allowed
            .as_ref()
            .or(self.skill_allowed_names.as_ref());

        // Apply skill filter as intersection
        let mut final_filtered = if let Some(allowed) = allowed {
            filtered
                .into_iter()
                .filter(|desc| allowed.contains(&desc.id.name))
                .collect()
        } else {
            filtered
        };

        // Apply MCP progressive discovery filtering
        if let (Some(threshold), Some(ref mcp_state), Some(ref mcp_reg)) = (
            self.mcp_discovery_threshold,
            &self.mcp_discovery_state,
            &self.mcp_registry,
        ) {
            let cached_tools = mcp_reg.get_cached_tools();
            let total_mcp_tools = cached_tools.len();
            if total_mcp_tools > threshold {
                let selected = if let Ok(guard) = mcp_state.lock() {
                    guard.selected_tools.clone()
                } else {
                    Vec::new()
                };
                final_filtered.retain(|desc| {
                    match &desc.id.namespace {
                        gestalt_core::tool_descriptor::ToolNamespace::Mcp(_) => {
                            // Only keep if it is in the selected tools list by canonical ID or unique provider name
                            let provider_name = gestalt_core::tool_name_mapping::ToolNameMapping::generate_provider_name(&desc.id);
                            selected.contains(&desc.id.to_string()) || selected.contains(&provider_name)
                        }
                        _ => true,
                    }
                });
            }
        }

        final_filtered
    }
}
