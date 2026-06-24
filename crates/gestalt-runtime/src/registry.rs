use crate::context::ContextContributor;
use crate::error::{Result, RuntimeError};
use gestalt_core::tool::{Tool, ToolSchema};
use gestalt_core::ContextStability;
use std::collections::BTreeMap;
use std::sync::Arc;

pub mod snapshot;

pub use snapshot::{
    ContextContributorSnapshot, HookRegistration, RuntimeFingerprint, RuntimeRegistrySnapshot,
    ToolRegistrationSnapshot, VerifierRegistration,
};

#[derive(Clone)]
pub struct ToolMetadata {
    pub name: String,
    pub schema: ToolSchema,
    pub schema_hash: String,
    pub tool: Option<Arc<dyn Tool>>,
    pub extension_id: Option<String>,
}

pub type ProviderFactory = Arc<
    dyn Fn(
            serde_json::Value,
        ) -> std::result::Result<
            Arc<dyn gestalt_core::provider::Provider>,
            gestalt_core::error::HarnessError,
        > + Send
        + Sync,
>;

pub struct ProviderMetadata {
    pub name: String,
    pub factory: ProviderFactory,
}

impl Clone for ProviderMetadata {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            factory: self.factory.clone(),
        }
    }
}

pub struct ContextContributorMetadata {
    pub name: String,
    pub contributor: Arc<dyn ContextContributor>,
    pub stability: ContextStability,
    pub extension_id: Option<String>,
}

impl Clone for ContextContributorMetadata {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            contributor: self.contributor.clone(),
            stability: self.stability,
            extension_id: self.extension_id.clone(),
        }
    }
}

#[derive(Clone)]
pub struct RuntimeRegistryBuilder {
    pub tools: BTreeMap<String, ToolMetadata>,
    pub providers: BTreeMap<String, ProviderMetadata>,
    pub context_contributors: BTreeMap<String, ContextContributorMetadata>,
    pub verifiers: Vec<String>,
    pub hooks: Vec<String>,
    pub extensions: Vec<String>,
}

#[deprecated(note = "use RuntimeRegistryBuilder for mutable registry construction")]
pub type RuntimeRegistry = RuntimeRegistryBuilder;

impl Default for RuntimeRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeRegistryBuilder {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
            providers: BTreeMap::new(),
            context_contributors: BTreeMap::new(),
            verifiers: Vec::new(),
            hooks: Vec::new(),
            extensions: Vec::new(),
        }
    }

    pub fn register_tool(&mut self, name: String, schema: ToolSchema) -> Result<()> {
        if self.tools.contains_key(&name) {
            return Err(RuntimeError::Registry(format!(
                "Duplicate tool registered: {}",
                name
            )));
        }
        let schema_hash = compute_schema_hash(&schema);
        self.tools.insert(
            name.clone(),
            ToolMetadata {
                name,
                schema,
                schema_hash,
                tool: None,
                extension_id: None,
            },
        );
        Ok(())
    }

    pub fn register_executable_tool(
        &mut self,
        name: String,
        schema: ToolSchema,
        tool: Arc<dyn Tool>,
        extension_id: Option<String>,
    ) -> Result<()> {
        if self.tools.contains_key(&name) {
            return Err(RuntimeError::Registry(format!(
                "Duplicate tool registered: {}",
                name
            )));
        }
        let schema_hash = compute_schema_hash(&schema);
        self.tools.insert(
            name.clone(),
            ToolMetadata {
                name,
                schema,
                schema_hash,
                tool: Some(tool),
                extension_id,
            },
        );
        Ok(())
    }

    pub fn register_provider(&mut self, name: String, factory: ProviderFactory) -> Result<()> {
        if self.providers.contains_key(&name) {
            return Err(RuntimeError::Registry(format!(
                "Duplicate provider registered: {}",
                name
            )));
        }
        self.providers
            .insert(name.clone(), ProviderMetadata { name, factory });
        Ok(())
    }

    pub fn register_context_contributor(
        &mut self,
        name: String,
        contributor: Arc<dyn ContextContributor>,
    ) -> Result<()> {
        if self.context_contributors.contains_key(&name) {
            return Err(RuntimeError::Registry(format!(
                "Duplicate context contributor registered: {}",
                name
            )));
        }
        let stability = contributor.stability();
        self.context_contributors.insert(
            name.clone(),
            ContextContributorMetadata {
                name: name.clone(),
                contributor,
                stability,
                extension_id: None,
            },
        );
        Ok(())
    }

    pub fn register_executable_context_contributor(
        &mut self,
        name: String,
        contributor: Arc<dyn ContextContributor>,
        extension_id: Option<String>,
    ) -> Result<()> {
        if self.context_contributors.contains_key(&name) {
            return Err(RuntimeError::Registry(format!(
                "Duplicate context contributor registered: {}",
                name
            )));
        }
        let stability = contributor.stability();
        self.context_contributors.insert(
            name.clone(),
            ContextContributorMetadata {
                name: name.clone(),
                contributor,
                stability,
                extension_id,
            },
        );
        Ok(())
    }

    pub fn register_verifier(&mut self, name: String) -> Result<()> {
        if self.verifiers.contains(&name) {
            return Err(RuntimeError::Registry(format!(
                "Duplicate verifier registered: {}",
                name
            )));
        }
        self.verifiers.push(name);
        Ok(())
    }

    pub fn register_hook(&mut self, name: String) -> Result<()> {
        if self.hooks.contains(&name) {
            return Err(RuntimeError::Registry(format!(
                "Duplicate hook registered: {}",
                name
            )));
        }
        self.hooks.push(name);
        Ok(())
    }

    pub fn register_extension(&mut self, name: String) -> Result<()> {
        if self.extensions.contains(&name) {
            return Err(RuntimeError::Registry(format!(
                "Duplicate extension registered: {}",
                name
            )));
        }
        self.extensions.push(name);
        Ok(())
    }

    pub fn snapshot(&self) -> RuntimeRegistrySnapshot {
        RuntimeRegistrySnapshot::from_builder(self)
    }
}

pub fn compute_schema_hash(schema: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    let serialized = serde_json::to_string(schema).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn compute_tool_schema_hash(schemas: &[serde_json::Value]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted_schemas = schemas.to_vec();
    sorted_schemas.sort_by(|a, b| {
        let name_a = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let name_b = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        name_a.cmp(name_b)
    });
    let mut hasher = Sha256::new();
    for schema in sorted_schemas {
        let serialized = serde_json::to_string(&schema).unwrap_or_default();
        hasher.update(serialized.as_bytes());
        hasher.update(b";");
    }
    format!("{:x}", hasher.finalize())
}
