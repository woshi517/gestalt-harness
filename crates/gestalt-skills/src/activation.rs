use crate::{ActiveSkill, Result, SkillDescriptor, SkillError};
use crate::index::SkillIndex;
use std::collections::HashSet;

/// Tracks which skills are active and why.
#[derive(Debug, Clone, Default)]
pub struct ActivationState {
    /// Skills explicitly activated by the user for this session.
    pub explicit: HashSet<String>,
    /// Skills explicitly deactivated by the user.
    pub deactivated: HashSet<String>,
    /// Skills requested via CLI --skill flags.
    pub cli_requested: Vec<String>,
}

impl ActivationState {
    pub fn new(cli_requested: Vec<String>) -> Self {
        Self {
            explicit: HashSet::new(),
            deactivated: HashSet::new(),
            cli_requested,
        }
    }

    pub fn activate(&mut self, name: &str) {
        self.explicit.insert(name.to_string());
        self.deactivated.remove(name);
    }

    pub fn deactivate(&mut self, name: &str) {
        self.deactivated.insert(name.to_string());
        self.explicit.remove(name);
    }
}

/// V1 deterministic activation engine.
///
/// Resolves the active skill set in precedence order:
/// 1. Explicit user activation
/// 2. CLI-provided activation
/// 3. Deterministic trigger/description matching over current task text
pub struct ActivationEngine;

impl ActivationEngine {
    /// Resolve active skills for the current turn.
    pub fn resolve(
        index: &SkillIndex,
        state: &ActivationState,
        current_task: Option<&str>,
    ) -> Vec<String> {
        let mut active = HashSet::new();

        // 1. Explicit user activation
        for name in &state.explicit {
            if index.contains(name) && !state.deactivated.contains(name) {
                active.insert(name.clone());
            }
        }

        // 2. CLI-provided activation (if not explicitly deactivated)
        for name in &state.cli_requested {
            if index.contains(name) && !state.deactivated.contains(name) {
                active.insert(name.clone());
            }
        }

        // 3. Deterministic trigger matching (only for trusted skills)
        if let Some(task) = current_task {
            let task_lower = task.to_lowercase();
            for desc in index.skills() {
                if active.contains(&desc.name) || state.deactivated.contains(&desc.name) {
                    continue;
                }
                // Only auto-activate trusted skills (Explicit or Workspace)
                if !matches!(desc.trust_level, crate::SkillTrustLevel::Explicit | crate::SkillTrustLevel::Workspace) {
                    continue;
                }
                let desc_lower = desc.description.to_lowercase();
                let trigger_words: Vec<&str> = desc_lower
                    .split_whitespace()
                    .filter(|w| w.len() > 3)
                    .collect();
                let name_lower = desc.name.to_lowercase();
                let name_words: Vec<&str> = name_lower
                    .split('-')
                    .collect();
                let match_score = trigger_words.iter().filter(|&&w| task_lower.contains(w)).count()
                    + name_words.iter().filter(|&&w| task_lower.contains(w)).count();
                if match_score > 0 {
                    active.insert(desc.name.clone());
                }
            }
        }

        let mut result: Vec<String> = active.into_iter().collect();
        result.sort();
        result
    }
}

/// Load the full SKILL.md body for a descriptor.
pub fn load_skill_body(descriptor: &SkillDescriptor) -> Result<ActiveSkill> {
    let full_body = std::fs::read_to_string(&descriptor.manifest_path)
        .map_err(SkillError::Io)?;
    Ok(ActiveSkill {
        descriptor: descriptor.clone(),
        full_body,
    })
}

/// Render active skill instructions as a context block.
pub fn render_active_skill_instructions(active: &[ActiveSkill]) -> String {
    let mut lines = vec!["<active_skills>".to_string()];
    for skill in active {
        lines.push(format!("## Skill: {}", skill.descriptor.name));
        lines.push(skill.descriptor.description.clone());
        lines.push("".to_string());
        lines.push(skill.full_body.clone());
        lines.push("".to_string());
    }
    lines.push("</active_skills>".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SkillSource, SkillTrustLevel};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn dummy_skill(name: &str, description: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.to_string(),
            description: description.to_string(),
            skill_root: PathBuf::from("/tmp"),
            manifest_path: PathBuf::from("/tmp/SKILL.md"),
            manifest_hash: "h".to_string(),
            trust_level: SkillTrustLevel::Workspace,
            source: SkillSource::WorkspaceLocal,
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: None,
        }
    }

    #[test]
    fn test_explicit_activation() {
        let index = SkillIndex::new(vec![
            dummy_skill("pdf", "Process PDFs."),
            dummy_skill("search", "Search code."),
        ]);
        let mut state = ActivationState::new(vec![]);
        state.activate("pdf");

        let active = ActivationEngine::resolve(&index, &state, None);
        assert_eq!(active, vec!["pdf"]);
    }

    #[test]
    fn test_cli_activation() {
        let index = SkillIndex::new(vec![dummy_skill("pdf", "Process PDFs.")]);
        let state = ActivationState::new(vec!["pdf".to_string()]);
        let active = ActivationEngine::resolve(&index, &state, None);
        assert_eq!(active, vec!["pdf"]);
    }

    #[test]
    fn test_deactivation_overrides() {
        let index = SkillIndex::new(vec![dummy_skill("pdf", "Process PDFs.")]);
        let mut state = ActivationState::new(vec!["pdf".to_string()]);
        state.deactivate("pdf");

        let active = ActivationEngine::resolve(&index, &state, None);
        assert!(active.is_empty());
    }

    #[test]
    fn test_trigger_match() {
        let index = SkillIndex::new(vec![dummy_skill("pdf", "Process PDF documents and forms.")]);
        let state = ActivationState::new(vec![]);
        let active = ActivationEngine::resolve(&index, &state, Some("Please extract text from this PDF"));
        assert_eq!(active, vec!["pdf"]);
    }

    #[test]
    fn test_trigger_no_match() {
        let index = SkillIndex::new(vec![dummy_skill("pdf", "Process PDF documents.")]);
        let state = ActivationState::new(vec![]);
        let active = ActivationEngine::resolve(&index, &state, Some("Write a hello world program"));
        assert!(active.is_empty());
    }
}
