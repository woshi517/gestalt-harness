use crate::SkillDescriptor;
use std::collections::HashSet;

/// Parsed tool permissions from a skill's `allowed-tools` declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillToolPolicy {
    pub allowed_tool_names: HashSet<String>,
    pub restricts_tools: bool,
}

impl SkillToolPolicy {
    /// Parse the `allowed-tools` space-separated string from a descriptor.
    pub fn from_descriptor(descriptor: &SkillDescriptor) -> Self {
        let Some(ref raw) = descriptor.allowed_tools else {
            return Self::default();
        };

        let mut allowed = HashSet::new();
        for token in raw.split_whitespace() {
            allowed.insert(token.to_string());
        }

        Self {
            allowed_tool_names: allowed,
            restricts_tools: true,
        }
    }

    /// Check if a tool name is allowed by this skill policy.
    pub fn allows(&self, tool_name: &str) -> bool {
        if !self.restricts_tools {
            return true;
        }
        self.allowed_tool_names.contains(tool_name)
    }
}

/// Compute the effective tool policy for a set of active skills.
///
/// V1 uses strict intersection: a tool is allowed only if ALL active
/// skills that declare restrictions allow it.
pub fn effective_tool_policy(skills: &[SkillDescriptor]) -> SkillToolPolicy {
    let mut restricts = Vec::new();
    for skill in skills {
        let policy = SkillToolPolicy::from_descriptor(skill);
        if policy.restricts_tools {
            restricts.push(policy.allowed_tool_names);
        }
    }

    if restricts.is_empty() {
        return SkillToolPolicy::default();
    }

    // Start with the first restriction set, then intersect.
    let mut effective = restricts[0].clone();
    for set in &restricts[1..] {
        effective.retain(|name| set.contains(name));
    }

    SkillToolPolicy {
        allowed_tool_names: effective,
        restricts_tools: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SkillSource, SkillTrustLevel};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn desc_with_tools(name: &str, tools: Option<&str>) -> SkillDescriptor {
        SkillDescriptor {
            name: name.to_string(),
            description: "test".to_string(),
            skill_root: PathBuf::from("/tmp"),
            manifest_path: PathBuf::from("/tmp/SKILL.md"),
            manifest_hash: "h".to_string(),
            trust_level: SkillTrustLevel::Workspace,
            source: SkillSource::WorkspaceLocal,
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: tools.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_single_skill_policy() {
        let d = desc_with_tools("a", Some("Read Search"));
        let p = SkillToolPolicy::from_descriptor(&d);
        assert!(p.allows("Read"));
        assert!(p.allows("Search"));
        assert!(!p.allows("Bash"));
    }

    #[test]
    fn test_no_restriction() {
        let d = desc_with_tools("a", None);
        let p = SkillToolPolicy::from_descriptor(&d);
        assert!(p.allows("Anything"));
        assert!(!p.restricts_tools);
    }

    #[test]
    fn test_intersection() {
        let a = desc_with_tools("a", Some("Read Search Bash"));
        let b = desc_with_tools("b", Some("Read Search"));
        let eff = effective_tool_policy(&[a, b]);
        assert!(eff.allows("Read"));
        assert!(eff.allows("Search"));
        assert!(!eff.allows("Bash"));
    }

    #[test]
    fn test_intersection_with_unrestricted() {
        let a = desc_with_tools("a", Some("Read Search"));
        let b = desc_with_tools("b", None);
        let eff = effective_tool_policy(&[a, b]);
        assert!(eff.allows("Read"));
        assert!(eff.allows("Search"));
        assert!(!eff.allows("Bash"));
    }
}
