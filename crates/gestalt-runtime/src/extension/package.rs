use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::error::{Result, RuntimeError};
use crate::manifest::{Entrypoint, ExtensionManifest};
use sha2::{Digest, Sha256};

use super::{
    ComponentInstanceId, ComponentKind, ExtensionComponentDescriptor, ExtensionGrantConfig,
    ExtensionInstanceConfig, ResolvedExtensionComponent,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionPackageDescriptor {
    pub id: String,
    pub name: String,
    pub version: String,
}

impl ExtensionPackageDescriptor {
    pub fn canonical_id(&self) -> String {
        format!("package:{}", self.id)
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        validate_stable_id("Package ID", &self.id)?;
        if self.id.starts_with("gestalt") || self.id.starts_with("harness") {
            return Err(format!(
                "Package ID '{}' starts with a reserved namespace ('gestalt' or 'harness')",
                self.id
            ));
        }
        if self.name.trim().is_empty() {
            return Err("Package name cannot be empty".to_string());
        }
        if self.version.trim().is_empty() {
            return Err("Package version cannot be empty".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ExtensionCompatibility {
    #[serde(default)]
    pub gestalt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionManifestV2 {
    pub manifest_version: u16,
    pub package: ExtensionPackageDescriptor,
    #[serde(default)]
    pub compatibility: ExtensionCompatibility,
    #[serde(default)]
    pub components: Vec<ExtensionComponentDescriptor>,
}

impl ExtensionManifestV2 {
    pub fn parse(content: &str) -> std::result::Result<Self, String> {
        toml::from_str(content).map_err(|e| format!("TOML parse error: {}", e))
    }

    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.manifest_version != 2 {
            return Err(format!(
                "Unsupported manifest_version '{}'; expected 2",
                self.manifest_version
            ));
        }
        self.package.validate()?;
        if self.components.is_empty() {
            return Err("Manifest v2 must declare at least one component".to_string());
        }

        let mut seen = HashSet::new();
        for component in &self.components {
            validate_stable_id("Component ID", &component.id)?;
            if !seen.insert(component.id.as_str()) {
                return Err(format!("Duplicate component id '{}'", component.id));
            }
        }

        for component in &self.components {
            if requires_entrypoint(&component.kind) && component.entrypoint.is_none() {
                return Err(format!(
                    "Component '{}' of kind '{:?}' requires an entrypoint",
                    component.id, component.kind
                ));
            }
            if component.kind == ComponentKind::ClientProduct && component.descriptor.is_none() {
                return Err(format!(
                    "Client product component '{}' must declare descriptor",
                    component.id
                ));
            }
            if component.kind == ComponentKind::CommandTool {
                if component
                    .description
                    .as_deref()
                    .unwrap_or("")
                    .trim()
                    .is_empty()
                {
                    return Err(format!(
                        "Command tool component '{}' must declare description",
                        component.id
                    ));
                }
                if component.input_schema.is_none() {
                    return Err(format!(
                        "Command tool component '{}' must declare input_schema",
                        component.id
                    ));
                }
                if component.risk.is_none() {
                    return Err(format!(
                        "Command tool component '{}' must declare risk",
                        component.id
                    ));
                }
                if component.read_only.is_none() {
                    return Err(format!(
                        "Command tool component '{}' must declare read_only",
                        component.id
                    ));
                }
                if component.idempotent.is_none() {
                    return Err(format!(
                        "Command tool component '{}' must declare idempotent",
                        component.id
                    ));
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedExtensionPackage {
    pub descriptor: ExtensionPackageDescriptor,
    pub instance_id: String,
    pub source_root: Option<PathBuf>,
    pub manifest_hash: Option<String>,
    pub trust: crate::extension_trust::ExtensionTrust,
    pub effective_config: serde_json::Value,
    pub effective_grants: ExtensionGrantConfig,
    pub components: Vec<ResolvedExtensionComponent>,
}

impl ResolvedExtensionPackage {
    pub fn apply_trust_decision(&mut self, trusted: bool) {
        if !trusted {
            self.trust = crate::extension_trust::ExtensionTrust::Untrusted;
            return;
        }

        self.trust = match self.manifest_hash.clone() {
            Some(manifest_hash) => {
                crate::extension_trust::ExtensionTrust::IntegrityTrusted { manifest_hash }
            }
            None => crate::extension_trust::ExtensionTrust::BuiltIn,
        };
    }

    pub fn with_instance(
        mut self,
        instance_id: impl Into<String>,
        config: serde_json::Value,
        grants: ExtensionGrantConfig,
    ) -> Self {
        let instance_id = instance_id.into();
        self.instance_id = instance_id.clone();
        self.effective_config = config.clone();
        self.effective_grants = grants.clone();
        for component in &mut self.components {
            component.id.instance_id = instance_id.clone();
            component.config = config.clone();
            component.grants = grants.clone();
            component.package_source_root = self.source_root.clone();
        }
        self
    }

    pub fn from_v1_manifest(manifest: ExtensionManifest) -> Result<Self> {
        manifest
            .validate(true)
            .map_err(|err| RuntimeError::Extension(err.clone()))?;

        let descriptor = ExtensionPackageDescriptor {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
        };
        let instance_id = manifest.id.clone();
        let component = ResolvedExtensionComponent {
            id: ComponentInstanceId::new(&manifest.id, &instance_id, "legacy"),
            kind: ComponentKind::LegacyProcess,
            optional: false,
            supports_cancellation: manifest.capabilities.supports_cancellation,
            entrypoint: manifest.entrypoint.clone(),
            descriptor: None,
            config: serde_json::Value::Null,
            grants: ExtensionGrantConfig::default(),
            tools: manifest.tools.clone(),
            hooks: manifest.hooks.clone(),
            context_injectors: manifest.context_injectors.clone(),
            permissions: manifest.permissions.clone(),
            protocol_version: manifest.protocol_version.clone(),
            description: None,
            input_schema: None,
            risk: None,
            read_only: false,
            idempotent: false,
            package_source_root: None,
        };

        Ok(Self {
            descriptor,
            instance_id,
            source_root: None,
            manifest_hash: None,
            trust: crate::extension_trust::ExtensionTrust::Untrusted,
            effective_config: serde_json::Value::Null,
            effective_grants: ExtensionGrantConfig::default(),
            components: vec![component],
        })
    }

    pub fn from_v2_manifest(
        manifest: ExtensionManifestV2,
        instance_id: impl Into<String>,
    ) -> Result<Self> {
        manifest
            .validate()
            .map_err(|err| RuntimeError::Extension(err.clone()))?;
        let instance_id = instance_id.into();
        let components = manifest
            .components
            .into_iter()
            .map(|component| {
                let entrypoint = component.entrypoint.unwrap_or_else(|| Entrypoint {
                    command: String::new(),
                    args: Vec::new(),
                });
                let component_perms = component.permissions.clone().unwrap_or_default();
                ResolvedExtensionComponent {
                    id: ComponentInstanceId::new(&manifest.package.id, &instance_id, component.id),
                    kind: component.kind,
                    optional: component.optional,
                    supports_cancellation: false,
                    entrypoint,
                    descriptor: component.descriptor,
                    config: serde_json::Value::Null,
                    grants: ExtensionGrantConfig::default(),
                    tools: Vec::new(),
                    hooks: Vec::new(),
                    context_injectors: Vec::new(),
                    permissions: component_perms,
                    protocol_version: None,
                    description: component.description,
                    input_schema: component.input_schema,
                    risk: component.risk,
                    read_only: component.read_only.unwrap_or(false),
                    idempotent: component.idempotent.unwrap_or(false),
                    package_source_root: None,
                }
            })
            .collect();

        Ok(Self {
            descriptor: manifest.package,
            instance_id,
            source_root: None,
            manifest_hash: None,
            trust: crate::extension_trust::ExtensionTrust::Untrusted,
            effective_config: serde_json::Value::Null,
            effective_grants: ExtensionGrantConfig::default(),
            components,
        })
    }

    pub fn to_runtime_component(
        &self,
        component_id: &str,
    ) -> Option<super::ExtensionRuntimeComponent> {
        let component = self
            .components
            .iter()
            .find(|c| c.id.component_id == component_id)?;
        let manifest_hash = self.manifest_hash.clone();

        let dependency_lock_hash = self
            .source_root
            .as_ref()
            .and_then(|root| super::manager::compute_dependency_lock_hash(root));

        let executable_hash = self.source_root.as_ref().and_then(|root| {
            super::manager::compute_executable_hash(
                root,
                &component.entrypoint.command,
                &component.entrypoint.args,
            )
        });

        Some(super::ExtensionRuntimeComponent {
            id: component.id.clone(),
            kind: component.kind.clone(),
            optional: component.optional,
            supports_cancellation: component.supports_cancellation,
            entrypoint_command: component.entrypoint.command.clone(),
            entrypoint_args: component.entrypoint.args.clone(),
            config: component.config.clone(),
            grants_fingerprint: fingerprint_json(&component.grants),
            trust: self.trust.clone(),
            protocol_fingerprint: component.protocol_version.clone(),
            package_version: self.descriptor.version.clone(),
            manifest_hash,
            executable_hash,
            dependency_lock_hash,
            permissions: component.permissions.clone(),
            grants: component.grants.clone(),
            package_source_root: self.source_root.clone(),
        })
    }
}

pub fn resolve_configured_instances(
    discovered: &[ResolvedExtensionPackage],
    configured: &BTreeMap<String, ExtensionInstanceConfig>,
) -> Result<Vec<ResolvedExtensionPackage>> {
    if configured.is_empty() {
        return Ok(discovered.to_vec());
    }

    let mut packages_by_id = BTreeMap::new();
    for discovered_package in discovered {
        packages_by_id.insert(
            discovered_package.descriptor.id.clone(),
            discovered_package.clone(),
        );
    }

    let mut resolved = Vec::new();
    for (instance_id, instance_config) in configured {
        if !instance_config.enabled {
            continue;
        }
        let Some(template) = packages_by_id.get(&instance_config.package) else {
            return Err(RuntimeError::Extension(format!(
                "Configured extension instance '{}' references unknown package '{}'",
                instance_id, instance_config.package
            )));
        };

        let mut package = template.clone().with_instance(
            instance_id.clone(),
            instance_config.config.clone(),
            instance_config.grants.clone(),
        );
        package.components.retain(|component| {
            instance_config
                .components
                .get(&component.id.component_id)
                .copied()
                .unwrap_or(true)
        });
        if package.components.is_empty() {
            return Err(RuntimeError::Extension(format!(
                "Configured extension instance '{}' disabled all components",
                instance_id
            )));
        }
        resolved.push(package);
    }

    Ok(resolved)
}

pub fn apply_trust_decisions(packages: &mut [ResolvedExtensionPackage], trusted_ids: &[String]) {
    for package in packages {
        let trusted = trusted_ids
            .iter()
            .any(|trusted_id| trusted_id == &package.descriptor.id);
        package.apply_trust_decision(trusted);
    }
}

fn requires_entrypoint(kind: &ComponentKind) -> bool {
    matches!(
        kind,
        ComponentKind::LegacyProcess
            | ComponentKind::GestaltLifecycle
            | ComponentKind::CommandTool
            | ComponentKind::McpServer
    )
}

fn validate_stable_id(label: &str, id: &str) -> std::result::Result<(), String> {
    if id.is_empty() || id.len() > 128 {
        return Err(format!("{label} must be between 1 and 128 characters"));
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return Err(format!("{label} cannot be empty"));
    };
    if !first.is_ascii_lowercase() {
        return Err(format!("{label} must start with a lowercase letter"));
    }
    for c in chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '.' && c != '-' && c != '_' {
            return Err(format!(
                "{label} '{}' contains invalid characters. Only lowercase alphanumeric, dots, hyphens, and underscores are allowed.",
                id
            ));
        }
    }
    Ok(())
}

pub fn compute_complete_fingerprint(
    registry_fingerprint: &str,
    resolved_packages: &[ResolvedExtensionPackage],
) -> String {
    if resolved_packages.is_empty() {
        return registry_fingerprint.to_string();
    }
    let mut hasher = Sha256::new();
    hasher.update(registry_fingerprint.as_bytes());
    hasher.update(b"|packages:");
    for package in resolved_packages {
        hasher.update(package.descriptor.id.as_bytes());
        hasher.update(b":");
        hasher.update(package.instance_id.as_bytes());
        hasher.update(b":");
        hasher.update(package.descriptor.version.as_bytes());
        if let Some(mh) = &package.manifest_hash {
            hasher.update(b":");
            hasher.update(mh.as_bytes());
        }
        hasher.update(b":trust:");
        hasher.update(fingerprint_json(&package.trust).as_bytes());

        // Compute and fold package-level dependency lock hash
        if let Some(ref source_root) = package.source_root {
            if let Some(lh) = super::manager::compute_dependency_lock_hash(source_root) {
                hasher.update(b":lock:");
                hasher.update(lh.as_bytes());
            }
        }

        hasher.update(b";");
        for component in &package.components {
            hasher.update(component.id.component_id.as_bytes());
            hasher.update(b":");
            hasher.update(format!("{:?}", component.kind).as_bytes());
            hasher.update(b":");
            hasher.update(component.optional.to_string().as_bytes());
            hasher.update(b":");
            hasher.update(component.supports_cancellation.to_string().as_bytes());
            hasher.update(b":");
            hasher.update(
                serde_json::to_string(&component.config)
                    .unwrap_or_default()
                    .as_bytes(),
            );
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.grants).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.permissions).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.tools).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.hooks).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.context_injectors).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.descriptor).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.description).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.input_schema).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.risk).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.read_only).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.idempotent).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.protocol_version).as_bytes());
            hasher.update(b":");
            hasher.update(fingerprint_json(&component.package_source_root).as_bytes());
            hasher.update(b":");
            hasher.update(component.entrypoint.command.as_bytes());
            for arg in &component.entrypoint.args {
                hasher.update(b"\0");
                hasher.update(arg.as_bytes());
            }

            // Compute and fold component-level executable hash
            if let Some(ref source_root) = package.source_root {
                let resolved_command =
                    if std::path::Path::new(&component.entrypoint.command).is_absolute() {
                        PathBuf::from(&component.entrypoint.command)
                    } else {
                        source_root.join(&component.entrypoint.command)
                    };
                hasher.update(b":resolved:");
                hasher.update(resolved_command.to_string_lossy().as_bytes());
                if let Some(eh) = super::manager::compute_executable_hash(
                    source_root,
                    &component.entrypoint.command,
                    &component.entrypoint.args,
                ) {
                    hasher.update(b":exec:");
                    hasher.update(eh.as_bytes());
                }
            }

            hasher.update(b";");
        }
    }
    format!("{:x}", hasher.finalize())
}

fn fingerprint_json<T: serde::Serialize>(value: &T) -> String {
    let mut hasher = Sha256::new();
    match serde_json::to_vec(value) {
        Ok(serialized) => hasher.update(serialized),
        Err(err) => hasher.update(err.to_string().as_bytes()),
    }
    format!("{:x}", hasher.finalize())
}
