use crate::error::{Result, RuntimeError};
use crate::manifest::ExtensionManifest;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredExtension {
    pub manifest_path: PathBuf,
    pub manifest: ExtensionManifest,
    pub manifest_hash: String,
    pub enabled: bool,
}

pub struct ExtensionDiscovery {
    workspace_root: PathBuf,
    global_dir: Option<PathBuf>,
}

impl ExtensionDiscovery {
    pub fn new(workspace_root: PathBuf, global_dir: Option<PathBuf>) -> Self {
        Self {
            workspace_root,
            global_dir,
        }
    }

    pub fn discover_all(&self, explicit_paths: &[PathBuf]) -> Result<Vec<DiscoveredExtension>> {
        let mut discovered = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        // 1. Explicit CLI loads
        for path in explicit_paths {
            let (manifest_path, content) = if path.is_dir() {
                let p = path.join("gestalt.extension.toml");
                (p.clone(), std::fs::read_to_string(&p))
            } else {
                (path.clone(), std::fs::read_to_string(path))
            };

            let content = content.map_err(|e| {
                RuntimeError::Extension(format!(
                    "Failed to read explicit manifest at {:?}: {}",
                    path, e
                ))
            })?;

            let manifest = ExtensionManifest::parse(&content).map_err(|e| {
                RuntimeError::Extension(format!("Invalid explicit manifest: {}", e))
            })?;
            manifest.validate(true).map_err(|e| {
                RuntimeError::Extension(format!("Validation failed for explicit manifest: {}", e))
            })?;

            if seen_ids.insert(manifest.id.clone()) {
                let hash = compute_content_hash(&content);
                discovered.push(DiscoveredExtension {
                    manifest_path,
                    manifest,
                    manifest_hash: hash,
                    enabled: true,
                });
            }
        }

        // 2. Project local `.gestalt/extensions`
        let project_ext_dir = self.workspace_root.join(".gestalt/extensions");
        if project_ext_dir.exists() && project_ext_dir.is_dir() {
            let mut entries = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&project_ext_dir) {
                for entry in rd.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let manifest_file = path.join("gestalt.extension.toml");
                        if manifest_file.exists() && manifest_file.is_file() {
                            entries.push(manifest_file);
                        }
                    }
                }
            }
            // Deterministic ordering by path string alphabetically
            entries.sort();
            for manifest_file in entries {
                if let Ok(content) = std::fs::read_to_string(&manifest_file) {
                    if let Ok(manifest) = ExtensionManifest::parse(&content) {
                        if manifest.validate(true).is_ok() && seen_ids.insert(manifest.id.clone()) {
                            let hash = compute_content_hash(&content);
                            discovered.push(DiscoveredExtension {
                                manifest_path: manifest_file,
                                manifest,
                                manifest_hash: hash,
                                enabled: true,
                            });
                        }
                    }
                }
            }
        }

        // 3. User global config dir extensions
        if let Some(ref gdir) = self.global_dir {
            let global_ext_dir = gdir.join("extensions");
            if global_ext_dir.exists() && global_ext_dir.is_dir() {
                let mut entries = Vec::new();
                if let Ok(rd) = std::fs::read_dir(&global_ext_dir) {
                    for entry in rd.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            let manifest_file = path.join("gestalt.extension.toml");
                            if manifest_file.exists() && manifest_file.is_file() {
                                entries.push(manifest_file);
                            }
                        }
                    }
                }
                // Deterministic ordering
                entries.sort();
                for manifest_file in entries {
                    if let Ok(content) = std::fs::read_to_string(&manifest_file) {
                        if let Ok(manifest) = ExtensionManifest::parse(&content) {
                            if manifest.validate(true).is_ok() && seen_ids.insert(manifest.id.clone()) {
                                let hash = compute_content_hash(&content);
                                discovered.push(DiscoveredExtension {
                                    manifest_path: manifest_file,
                                    manifest,
                                    manifest_hash: hash,
                                    enabled: true,
                                });
                            }
                        }
                    }
                }
            }
        }

        Ok(discovered)
    }
}

fn compute_content_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}
