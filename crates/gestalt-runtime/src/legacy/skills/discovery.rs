use crate::skill_manifest::SkillManifest;
use crate::{SkillDescriptor, SkillError, SkillSource, SkillTrustLevel};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct SkillDiscovery {
    workspace_root: PathBuf,
    global_dir: Option<PathBuf>,
    home_dir: Option<PathBuf>,
}

impl SkillDiscovery {
    pub fn new(
        workspace_root: PathBuf,
        global_dir: Option<PathBuf>,
        home_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            workspace_root,
            global_dir,
            home_dir,
        }
    }

    /// Discover skills from all sources in deterministic order:
    /// 1. Explicit paths
    /// 2. Workspace `.gestalt/skills/`
    /// 3. Workspace `.agents/skills/`
    /// 4. Global `~/.config/gestalt/skills/`
    /// 5. Global `~/.agents/skills/`
    pub fn discover_all(
        &self,
        explicit_paths: &[PathBuf],
    ) -> std::result::Result<Vec<SkillDescriptor>, SkillError> {
        let mut discovered = Vec::new();
        let mut seen_names = HashSet::new();

        // 1. Explicit paths
        for path in explicit_paths {
            if let Some(skill) = Self::load_explicit(path, &mut seen_names)? {
                discovered.push(skill);
            }
        }

        // 2. Workspace `.gestalt/skills/`
        let gestalt_skills = self.workspace_root.join(".gestalt/skills");
        if gestalt_skills.exists() && gestalt_skills.is_dir() {
            Self::collect_from_dir(
                &gestalt_skills,
                SkillSource::WorkspaceLocal,
                SkillTrustLevel::Workspace,
                &mut seen_names,
                &mut discovered,
            );
        }

        // 3. Workspace `.agents/skills/`
        let agents_skills = self.workspace_root.join(".agents/skills");
        if agents_skills.exists() && agents_skills.is_dir() {
            Self::collect_from_dir(
                &agents_skills,
                SkillSource::WorkspaceLocal,
                SkillTrustLevel::Workspace,
                &mut seen_names,
                &mut discovered,
            );
        }

        // 4. Global `~/.config/gestalt/skills/`
        if let Some(ref gdir) = self.global_dir {
            let global_skills = gdir.join("skills");
            if global_skills.exists() && global_skills.is_dir() {
                Self::collect_from_dir(
                    &global_skills,
                    SkillSource::GlobalConfig,
                    SkillTrustLevel::Global,
                    &mut seen_names,
                    &mut discovered,
                );
            }
        }

        // 5. Global `~/.agents/skills/`
        if let Some(ref hdir) = self.home_dir {
            let home_agents_skills = hdir.join(".agents/skills");
            if home_agents_skills.exists() && home_agents_skills.is_dir() {
                Self::collect_from_dir(
                    &home_agents_skills,
                    SkillSource::GlobalConfig,
                    SkillTrustLevel::Global,
                    &mut seen_names,
                    &mut discovered,
                );
            }
        }

        Ok(discovered)
    }

    fn load_explicit(
        path: &Path,
        seen_names: &mut HashSet<String>,
    ) -> std::result::Result<Option<SkillDescriptor>, SkillError> {
        let skill_root = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()
                .ok_or_else(|| {
                    SkillError::Validation("Explicit path has no parent directory".to_string())
                })?
                .to_path_buf()
        };

        let manifest_path = skill_root.join("SKILL.md");
        if !manifest_path.exists() {
            return Ok(None);
        }

        let raw = std::fs::read_to_string(&manifest_path)?;
        let file = SkillManifest::parse(&raw).map_err(SkillError::YamlParse)?;
        let dir_name = skill_root.file_name().and_then(|n| n.to_str());
        file.manifest
            .validate(dir_name)
            .map_err(SkillError::Validation)?;

        if !seen_names.insert(file.manifest.name.clone()) {
            return Ok(None);
        }

        let hash = compute_content_hash(&raw);

        Ok(Some(SkillDescriptor {
            name: file.manifest.name,
            description: file.manifest.description,
            skill_root,
            manifest_path,
            manifest_hash: hash,
            trust_level: SkillTrustLevel::Explicit,
            source: SkillSource::ExplicitPath,
            license: file.manifest.license,
            compatibility: file.manifest.compatibility,
            metadata: file.manifest.metadata,
            allowed_tools: file.manifest.allowed_tools,
        }))
    }

    fn collect_from_dir(
        dir: &Path,
        source: SkillSource,
        trust_level: SkillTrustLevel,
        seen_names: &mut HashSet<String>,
        discovered: &mut Vec<SkillDescriptor>,
    ) {
        let mut entries = Vec::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let manifest_file = path.join("SKILL.md");
                    if manifest_file.exists() && manifest_file.is_file() {
                        entries.push(path);
                    }
                }
            }
        }

        entries.sort();

        for skill_root in entries {
            let manifest_path = skill_root.join("SKILL.md");
            let raw = match std::fs::read_to_string(&manifest_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let file = match SkillManifest::parse(&raw) {
                Ok(f) => f,
                Err(_) => continue,
            };

            let dir_name = skill_root.file_name().and_then(|n| n.to_str());
            if file.manifest.validate(dir_name).is_err() {
                continue;
            }

            if !seen_names.insert(file.manifest.name.clone()) {
                continue;
            }

            let hash = compute_content_hash(&raw);

            discovered.push(SkillDescriptor {
                name: file.manifest.name,
                description: file.manifest.description,
                skill_root,
                manifest_path,
                manifest_hash: hash,
                trust_level,
                source,
                license: file.manifest.license,
                compatibility: file.manifest.compatibility,
                metadata: file.manifest.metadata,
                allowed_tools: file.manifest.allowed_tools,
            });
        }
    }
}

fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_skill_dir(root: &Path, name: &str, content: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let mut file = std::fs::File::create(dir.join("SKILL.md")).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        dir
    }

    #[test]
    fn test_discovery_ordering() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let gestalt = root.join(".gestalt/skills");
        std::fs::create_dir_all(&gestalt).unwrap();
        make_skill_dir(
            &gestalt,
            "alpha-skill",
            "---\nname: alpha-skill\ndescription: Alpha.\n---\n",
        );
        make_skill_dir(
            &gestalt,
            "beta-skill",
            "---\nname: beta-skill\ndescription: Beta.\n---\n",
        );

        let discovery = SkillDiscovery::new(root.to_path_buf(), None, None);
        let found = discovery.discover_all(&[]).unwrap();

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "alpha-skill");
        assert_eq!(found[1].name, "beta-skill");
    }

    #[test]
    fn test_explicit_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let gestalt = root.join(".gestalt/skills");
        std::fs::create_dir_all(&gestalt).unwrap();
        make_skill_dir(
            &gestalt,
            "my-skill",
            "---\nname: my-skill\ndescription: Workspace.\n---\n",
        );

        let explicit = root.join("explicit/my-skill");
        std::fs::create_dir_all(&explicit).unwrap();
        let mut file = std::fs::File::create(explicit.join("SKILL.md")).unwrap();
        file.write_all("---\nname: my-skill\ndescription: Explicit.\n---\n".as_bytes())
            .unwrap();

        let discovery = SkillDiscovery::new(root.to_path_buf(), None, None);
        let found = discovery.discover_all(&[explicit]).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "Explicit.");
        assert_eq!(found[0].trust_level, SkillTrustLevel::Explicit);
    }

    #[test]
    fn test_duplicate_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let gestalt = root.join(".gestalt/skills");
        std::fs::create_dir_all(&gestalt).unwrap();
        make_skill_dir(
            &gestalt,
            "my-skill",
            "---\nname: my-skill\ndescription: First.\n---\n",
        );
        make_skill_dir(
            &gestalt,
            "my-skill-dup",
            "---\nname: my-skill\ndescription: Dup.\n---\n",
        );

        let discovery = SkillDiscovery::new(root.to_path_buf(), None, None);
        let found = discovery.discover_all(&[]).unwrap();

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "First.");
    }
}
