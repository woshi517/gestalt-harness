use crate::SkillDescriptor;
use std::collections::HashMap;

/// A deterministic index of discovered skills for fast lookup.
#[derive(Debug, Clone, Default)]
pub struct SkillIndex {
    skills: Vec<SkillDescriptor>,
    by_name: HashMap<String, usize>,
}

impl SkillIndex {
    pub fn new(skills: Vec<SkillDescriptor>) -> Self {
        let mut by_name = HashMap::with_capacity(skills.len());
        for (idx, skill) in skills.iter().enumerate() {
            by_name.insert(skill.name.clone(), idx);
        }
        Self { skills, by_name }
    }

    pub fn skills(&self) -> &[SkillDescriptor] {
        &self.skills
    }

    pub fn get(&self, name: &str) -> Option<&SkillDescriptor> {
        self.by_name.get(name).map(|&idx| &self.skills[idx])
    }

    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn names(&self) -> Vec<String> {
        self.skills.iter().map(|s| s.name.clone()).collect()
    }

    /// Merge another index, with existing entries taking precedence.
    pub fn merge(&mut self, other: SkillIndex) {
        for skill in other.skills {
            if !self.by_name.contains_key(&skill.name) {
                let idx = self.skills.len();
                self.by_name.insert(skill.name.clone(), idx);
                self.skills.push(skill);
            }
        }
    }

    /// Build a compact available-skills index string for context injection.
    pub fn to_context_index(&self) -> String {
        let mut lines = vec!["<available_skills>".to_string()];
        for skill in &self.skills {
            lines.push(format!("- {}: {}", skill.name, skill.description));
        }
        lines.push("</available_skills>".to_string());
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SkillSource, SkillTrustLevel};
    use std::path::PathBuf;

    fn dummy_skill(name: &str, description: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.to_string(),
            description: description.to_string(),
            skill_root: PathBuf::from("/dev/null"),
            manifest_path: PathBuf::from("/dev/null/SKILL.md"),
            manifest_hash: "abc".to_string(),
            trust_level: SkillTrustLevel::Workspace,
            source: SkillSource::WorkspaceLocal,
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: None,
        }
    }

    #[test]
    fn test_lookup() {
        let index = SkillIndex::new(vec![
            dummy_skill("alpha", "Alpha skill"),
            dummy_skill("beta", "Beta skill"),
        ]);
        assert_eq!(index.get("alpha").unwrap().description, "Alpha skill");
        assert!(index.get("gamma").is_none());
    }

    #[test]
    fn test_context_index() {
        let index = SkillIndex::new(vec![dummy_skill("pdf", "Process PDFs.")]);
        let ctx = index.to_context_index();
        assert!(ctx.contains("<available_skills>"));
        assert!(ctx.contains("pdf: Process PDFs."));
        assert!(ctx.contains("</available_skills>"));
    }
}
