use std::collections::HashSet;

use crate::error::{Result, RuntimeError};
use crate::manifest::{Entrypoint, ExtensionManifest, Permissions};

use super::{
    ComponentInstanceId, ComponentKind, ExtensionComponentDescriptor, ResolvedExtensionComponent,
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
    pub components: Vec<ResolvedExtensionComponent>,
}

impl ResolvedExtensionPackage {
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
            entrypoint: manifest.entrypoint.clone(),
            descriptor: None,
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
        };

        Ok(Self {
            descriptor,
            instance_id,
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
                ResolvedExtensionComponent {
                    id: ComponentInstanceId::new(&manifest.package.id, &instance_id, component.id),
                    kind: component.kind,
                    optional: component.optional,
                    entrypoint,
                    descriptor: component.descriptor,
                    tools: Vec::new(),
                    hooks: Vec::new(),
                    context_injectors: Vec::new(),
                    permissions: Permissions::default(),
                    protocol_version: None,
                    description: component.description,
                    input_schema: component.input_schema,
                    risk: component.risk,
                    read_only: component.read_only.unwrap_or(false),
                    idempotent: component.idempotent.unwrap_or(false),
                }
            })
            .collect();

        Ok(Self {
            descriptor: manifest.package,
            instance_id,
            components,
        })
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
