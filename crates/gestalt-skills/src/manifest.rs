use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Parsed YAML frontmatter from a `SKILL.md` file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    #[serde(default, rename = "allowed-tools", skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
}

/// The complete parsed content of a `SKILL.md` file.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillFile {
    pub manifest: SkillManifest,
    pub body: String,
    pub raw: String,
}

impl SkillManifest {
    /// Parse the YAML frontmatter and markdown body from raw SKILL.md content.
    pub fn parse(raw: &str) -> Result<SkillFile, String> {
        let trimmed = raw.trim_start();
        if !trimmed.starts_with("---") {
            return Err("SKILL.md must start with YAML frontmatter delimited by '---'".to_string());
        }

        // Find the end of frontmatter
        let after_open = &trimmed[3..];
        let Some(end_pos) = after_open.find("---") else {
            return Err("YAML frontmatter not closed with '---'".to_string());
        };

        let yaml_text = &after_open[..end_pos].trim();
        let body = after_open[end_pos + 3..].trim_start().to_string();

        let manifest: Self = serde_yaml::from_str(yaml_text)
            .map_err(|e| format!("YAML frontmatter parse error: {e}"))?;

        Ok(SkillFile {
            manifest,
            body,
            raw: raw.to_string(),
        })
    }

    /// Validate the manifest against Agent Skills naming rules.
    pub fn validate(&self, expected_dir_name: Option<&str>) -> Result<(), String> {
        let name = &self.name;

        if name.is_empty() || name.len() > 64 {
            return Err(format!(
                "Skill name must be 1-64 characters, got {}",
                name.len()
            ));
        }

        if name.starts_with('-') || name.ends_with('-') {
            return Err("Skill name must not start or end with a hyphen".to_string());
        }

        if name.contains("--") {
            return Err("Skill name must not contain consecutive hyphens".to_string());
        }

        let valid = name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            return Err(
                "Skill name may only contain lowercase letters, numbers, and hyphens".to_string(),
            );
        }

        if self.description.is_empty() {
            return Err("Skill description is required".to_string());
        }

        if self.description.len() > 1024 {
            return Err(format!(
                "Skill description must be at most 1024 characters, got {}",
                self.description.len()
            ));
        }

        if let Some(ref compat) = self.compatibility {
            if compat.len() > 500 {
                return Err(format!(
                    "Skill compatibility must be at most 500 characters, got {}",
                    compat.len()
                ));
            }
        }

        if let Some(expected) = expected_dir_name {
            if name != expected {
                return Err(format!(
                    "Skill name '{}' does not match directory name '{}'",
                    name, expected
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal() {
        let raw = "---\nname: pdf-processing\ndescription: Extract PDF text.\n---\n\n# Instructions\n";
        let file = SkillManifest::parse(raw).unwrap();
        assert_eq!(file.manifest.name, "pdf-processing");
        assert_eq!(file.manifest.description, "Extract PDF text.");
        assert!(file.body.contains("# Instructions"));
    }

    #[test]
    fn test_parse_full() {
        let raw = r#"---
name: data-analysis
description: Analyze data files.
license: MIT
metadata:
  author: test
allowed-tools: Read Search
---

# Data Analysis
"#;
        let file = SkillManifest::parse(raw).unwrap();
        assert_eq!(file.manifest.name, "data-analysis");
        assert_eq!(file.manifest.license, Some("MIT".to_string()));
        assert_eq!(file.manifest.allowed_tools, Some("Read Search".to_string()));
        assert_eq!(file.manifest.metadata.get("author"), Some(&"test".to_string()));
    }

    #[test]
    fn test_validate_name_rules() {
        let mut m = SkillManifest {
            name: "valid-name".to_string(),
            description: "A valid description.".to_string(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: None,
        };
        assert!(m.validate(None).is_ok());

        m.name = "InvalidName".to_string();
        assert!(m.validate(None).is_err());

        m.name = "-invalid".to_string();
        assert!(m.validate(None).is_err());

        m.name = "invalid--name".to_string();
        assert!(m.validate(None).is_err());

        m.name = "a".repeat(65);
        assert!(m.validate(None).is_err());
    }

    #[test]
    fn test_validate_dir_name_mismatch() {
        let m = SkillManifest {
            name: "foo".to_string(),
            description: "A desc.".to_string(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: None,
        };
        assert!(m.validate(Some("foo")).is_ok());
        assert!(m.validate(Some("bar")).is_err());
    }
}
