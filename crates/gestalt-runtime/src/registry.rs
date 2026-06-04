use std::collections::HashMap;
use std::sync::Arc;
use gestalt_core::tool::ToolSchema;
use crate::error::{Result, RuntimeError};
use crate::context::ContextContributor;

#[derive(Clone)]
pub struct ToolMetadata {
    pub name: String,
    pub schema: ToolSchema,
    pub schema_hash: String,
}

pub type ProviderFactory = Arc<
    dyn Fn(serde_json::Value) -> std::result::Result<Arc<dyn gestalt_core::provider::Provider>, gestalt_core::error::HarnessError>
        + Send
        + Sync,
>;

pub struct ProviderMetadata {
    pub name: String,
    pub factory: ProviderFactory,
}

pub struct RuntimeRegistry {
    pub tools: HashMap<String, ToolMetadata>,
    pub providers: HashMap<String, ProviderMetadata>,
    pub context_contributors: HashMap<String, Arc<dyn ContextContributor>>,
    pub verifiers: Vec<String>,
    pub hooks: Vec<String>,
    pub extensions: Vec<String>,
}

impl Default for RuntimeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            providers: HashMap::new(),
            context_contributors: HashMap::new(),
            verifiers: Vec::new(),
            hooks: Vec::new(),
            extensions: Vec::new(),
        }
    }

    pub fn register_tool(&mut self, name: String, schema: ToolSchema) -> Result<()> {
        if self.tools.contains_key(&name) {
            return Err(RuntimeError::Registry(format!("Duplicate tool registered: {}", name)));
        }
        let schema_hash = compute_schema_hash(&schema);
        self.tools.insert(name.clone(), ToolMetadata {
            name,
            schema,
            schema_hash,
        });
        Ok(())
    }

    pub fn register_provider(&mut self, name: String, factory: ProviderFactory) -> Result<()> {
        if self.providers.contains_key(&name) {
            return Err(RuntimeError::Registry(format!("Duplicate provider registered: {}", name)));
        }
        self.providers.insert(name.clone(), ProviderMetadata {
            name,
            factory,
        });
        Ok(())
    }

    pub fn register_context_contributor(&mut self, name: String, contributor: Arc<dyn ContextContributor>) -> Result<()> {
        if self.context_contributors.contains_key(&name) {
            return Err(RuntimeError::Registry(format!("Duplicate context contributor registered: {}", name)));
        }
        self.context_contributors.insert(name, contributor);
        Ok(())
    }

    pub fn register_verifier(&mut self, name: String) -> Result<()> {
        if self.verifiers.contains(&name) {
            return Err(RuntimeError::Registry(format!("Duplicate verifier registered: {}", name)));
        }
        self.verifiers.push(name);
        Ok(())
    }

    pub fn register_hook(&mut self, name: String) -> Result<()> {
        if self.hooks.contains(&name) {
            return Err(RuntimeError::Registry(format!("Duplicate hook registered: {}", name)));
        }
        self.hooks.push(name);
        Ok(())
    }

    pub fn register_extension(&mut self, name: String) -> Result<()> {
        if self.extensions.contains(&name) {
            return Err(RuntimeError::Registry(format!("Duplicate extension registered: {}", name)));
        }
        self.extensions.push(name);
        Ok(())
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
