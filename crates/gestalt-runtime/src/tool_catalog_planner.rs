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
}

impl ToolCatalogPlanner {
    pub fn new(profile: ToolProfile) -> Self {
        Self {
            profile,
            skill_allowed_names: None,
            skill_state: None,
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
            let policy = gestalt_skills::effective_tool_policy(&active);
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
        if let Some(allowed) = allowed {
            return filtered
                .into_iter()
                .filter(|desc| allowed.contains(&desc.id.name))
                .collect();
        }

        filtered
    }
}
