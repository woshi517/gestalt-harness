use crate::error::{Result, RuntimeError};
use crate::extension::{ExtensionManifestV2, ResolvedExtensionPackage};
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

#[derive(Debug, Clone)]
pub struct DiscoveredExtensionPackage {
    pub manifest_path: PathBuf,
    pub source_root: PathBuf,
    pub package: ResolvedExtensionPackage,
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
                            if manifest.validate(true).is_ok()
                                && seen_ids.insert(manifest.id.clone())
                            {
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

    pub fn discover_packages(
        &self,
        explicit_paths: &[PathBuf],
    ) -> Result<Vec<DiscoveredExtensionPackage>> {
        let mut discovered = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        for path in explicit_paths {
            let (manifest_path, content) = read_manifest_from_path(path).map_err(|err| {
                RuntimeError::Extension(format!(
                    "Failed to read explicit manifest at {:?}: {}",
                    path, err
                ))
            })?;
            let package = parse_package_manifest(&content).map_err(|err| {
                RuntimeError::Extension(format!("Invalid explicit manifest: {}", err))
            })?;
            if seen_ids.insert(package.descriptor.id.clone()) {
                let source_root = manifest_path
                    .parent()
                    .map_or_else(|| self.workspace_root.clone(), |path| path.to_path_buf());
                let manifest_hash = compute_content_hash(&content);
                let mut package = package;
                package.source_root = Some(source_root.clone());
                package.manifest_hash = Some(manifest_hash.clone());
                discovered.push(DiscoveredExtensionPackage {
                    manifest_path,
                    source_root,
                    package,
                    manifest_hash,
                    enabled: true,
                });
            }
        }

        for manifest_file in self.project_manifest_paths() {
            if let Ok(content) = std::fs::read_to_string(&manifest_file) {
                if let Ok(package) = parse_package_manifest(&content) {
                    if seen_ids.insert(package.descriptor.id.clone()) {
                        let source_root = manifest_file
                            .parent()
                            .map_or_else(|| self.workspace_root.clone(), |path| path.to_path_buf());
                        let manifest_hash = compute_content_hash(&content);
                        let mut package = package;
                        package.source_root = Some(source_root.clone());
                        package.manifest_hash = Some(manifest_hash.clone());
                        discovered.push(DiscoveredExtensionPackage {
                            manifest_path: manifest_file,
                            source_root,
                            package,
                            manifest_hash,
                            enabled: true,
                        });
                    }
                }
            }
        }

        if let Some(ref gdir) = self.global_dir {
            for manifest_file in collect_extension_manifest_paths(gdir.join("extensions")) {
                if let Ok(content) = std::fs::read_to_string(&manifest_file) {
                    if let Ok(package) = parse_package_manifest(&content) {
                        if seen_ids.insert(package.descriptor.id.clone()) {
                            let source_root = manifest_file.parent().map_or_else(
                                || self.workspace_root.clone(),
                                |path| path.to_path_buf(),
                            );
                            let manifest_hash = compute_content_hash(&content);
                            let mut package = package;
                            package.source_root = Some(source_root.clone());
                            package.manifest_hash = Some(manifest_hash.clone());
                            discovered.push(DiscoveredExtensionPackage {
                                manifest_path: manifest_file,
                                source_root,
                                package,
                                manifest_hash,
                                enabled: true,
                            });
                        }
                    }
                }
            }
        }

        Ok(discovered)
    }

    fn project_manifest_paths(&self) -> Vec<PathBuf> {
        collect_extension_manifest_paths(self.workspace_root.join(".gestalt/extensions"))
    }
}

fn compute_content_hash(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn read_manifest_from_path(path: &PathBuf) -> std::io::Result<(PathBuf, String)> {
    if path.is_dir() {
        let manifest_path = path.join("gestalt.extension.toml");
        let content = std::fs::read_to_string(&manifest_path)?;
        Ok((manifest_path, content))
    } else {
        let content = std::fs::read_to_string(path)?;
        Ok((path.clone(), content))
    }
}

fn collect_extension_manifest_paths(extension_dir: PathBuf) -> Vec<PathBuf> {
    let mut entries = Vec::new();
    if extension_dir.exists() && extension_dir.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&extension_dir) {
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
    }
    entries.sort();
    entries
}

fn parse_package_manifest(content: &str) -> std::result::Result<ResolvedExtensionPackage, String> {
    if is_manifest_v2(content)? {
        let manifest = ExtensionManifestV2::parse(content)?;
        ResolvedExtensionPackage::from_v2_manifest(manifest.clone(), manifest.package.id)
            .map_err(|err| err.to_string())
    } else {
        let manifest = ExtensionManifest::parse(content)?;
        ResolvedExtensionPackage::from_v1_manifest(manifest).map_err(|err| err.to_string())
    }
}

fn is_manifest_v2(content: &str) -> std::result::Result<bool, String> {
    let value = content
        .parse::<toml::Value>()
        .map_err(|err| format!("TOML parse error: {}", err))?;
    Ok(value
        .get("manifest_version")
        .and_then(toml::Value::as_integer)
        == Some(2))
}

pub struct DiscoverySource {
    pub discovery: ExtensionDiscovery,
    pub explicit_paths: Vec<PathBuf>,
}

impl DiscoverySource {
    pub fn new(discovery: ExtensionDiscovery, explicit_paths: Vec<PathBuf>) -> Self {
        Self {
            discovery,
            explicit_paths,
        }
    }
}

impl crate::activation::ExtensionSource for DiscoverySource {
    fn discover_packages(&self) -> Result<Vec<ResolvedExtensionPackage>> {
        let discovered = self.discovery.discover_packages(&self.explicit_paths)?;
        Ok(discovered.into_iter().map(|dp| dp.package).collect())
    }
}
