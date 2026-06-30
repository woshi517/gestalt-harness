use crate::error::{Result, RuntimeError};
use crate::extension::{ExtensionManifestV2, ResolvedExtensionPackage};
use std::path::PathBuf;

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
            let content = std::fs::read_to_string(&manifest_file).map_err(|err| {
                RuntimeError::Extension(format!(
                    "Failed to read project manifest at {:?}: {}",
                    manifest_file, err
                ))
            })?;
            let package = parse_package_manifest(&content).map_err(|err| {
                RuntimeError::Extension(format!(
                    "Invalid project manifest {:?}: {}",
                    manifest_file, err
                ))
            })?;
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

        if let Some(ref gdir) = self.global_dir {
            for manifest_file in collect_extension_manifest_paths(gdir.join("extensions")) {
                let content = std::fs::read_to_string(&manifest_file).map_err(|err| {
                    RuntimeError::Extension(format!(
                        "Failed to read global manifest at {:?}: {}",
                        manifest_file, err
                    ))
                })?;
                let package = parse_package_manifest(&content).map_err(|err| {
                    RuntimeError::Extension(format!(
                        "Invalid global manifest {:?}: {}",
                        manifest_file, err
                    ))
                })?;
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
    #[cfg(feature = "toml-config")]
    {
        let value = content
            .parse::<toml::Value>()
            .map_err(|err| format!("TOML parse error: {}", err))?;
        let Some(manifest_version_value) = value.get("manifest_version") else {
            return Err("missing required manifest_version = 2".to_string());
        };
        let Some(manifest_version) = manifest_version_value.as_integer() else {
            return Err("manifest_version must be an integer".to_string());
        };
        if manifest_version != 2 {
            return Err(format!(
                "unsupported manifest_version {}; only manifest_version = 2 is supported",
                manifest_version
            ));
        }
    }
    #[cfg(not(feature = "toml-config"))]
    {
        let _ = content;
        return Err(
            "feature 'toml-config' is not enabled for extension manifest parsing".to_string(),
        );
    }

    let manifest = ExtensionManifestV2::parse(content)?;
    ResolvedExtensionPackage::from_v2_manifest(manifest.clone(), manifest.package.id)
        .map_err(|err| err.to_string())
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
