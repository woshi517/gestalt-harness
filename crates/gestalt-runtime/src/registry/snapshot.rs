use std::collections::BTreeMap;
use std::sync::Arc;

use gestalt_core::tool::{Tool, ToolSchema};
use gestalt_core::ContextStability;
use sha2::{Digest, Sha256};

use crate::context::ContextContributor;
use crate::registry::{ContextContributorMetadata, RuntimeRegistryBuilder, ToolMetadata};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFingerprint(pub String);

impl RuntimeFingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RuntimeFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone)]
pub struct ToolRegistrationSnapshot {
    pub name: String,
    pub schema: ToolSchema,
    pub schema_hash: String,
    pub tool: Option<Arc<dyn Tool>>,
    pub extension_id: Option<String>,
}

impl From<&ToolMetadata> for ToolRegistrationSnapshot {
    fn from(value: &ToolMetadata) -> Self {
        Self {
            name: value.name.clone(),
            schema: value.schema.clone(),
            schema_hash: value.schema_hash.clone(),
            tool: value.tool.clone(),
            extension_id: value.extension_id.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ContextContributorSnapshot {
    pub name: String,
    pub contributor: Arc<dyn ContextContributor>,
    pub stability: ContextStability,
    pub extension_id: Option<String>,
}

impl From<&ContextContributorMetadata> for ContextContributorSnapshot {
    fn from(value: &ContextContributorMetadata) -> Self {
        Self {
            name: value.name.clone(),
            contributor: value.contributor.clone(),
            stability: value.stability,
            extension_id: value.extension_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRegistration {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifierRegistration {
    pub name: String,
}

#[derive(Clone)]
pub struct RuntimeRegistrySnapshot {
    pub fingerprint: RuntimeFingerprint,
    pub tools: BTreeMap<String, ToolRegistrationSnapshot>,
    pub context_contributors: BTreeMap<String, ContextContributorSnapshot>,
    pub hooks: Vec<HookRegistration>,
    pub verifiers: Vec<VerifierRegistration>,
    pub extensions: Vec<String>,
}

impl RuntimeRegistrySnapshot {
    pub(crate) fn from_builder(builder: &RuntimeRegistryBuilder) -> Self {
        let tools = builder
            .tools
            .iter()
            .map(|(name, metadata)| (name.clone(), ToolRegistrationSnapshot::from(metadata)))
            .collect::<BTreeMap<_, _>>();
        let context_contributors = builder
            .context_contributors
            .iter()
            .map(|(name, metadata)| (name.clone(), ContextContributorSnapshot::from(metadata)))
            .collect::<BTreeMap<_, _>>();
        let hooks = builder
            .hooks
            .iter()
            .cloned()
            .map(|name| HookRegistration { name })
            .collect::<Vec<_>>();
        let verifiers = builder
            .verifiers
            .iter()
            .cloned()
            .map(|name| VerifierRegistration { name })
            .collect::<Vec<_>>();
        let extensions = builder.extensions.clone();
        let fingerprint = compute_registry_fingerprint(
            &tools,
            &context_contributors,
            &hooks,
            &verifiers,
            &extensions,
        );

        Self {
            fingerprint,
            tools,
            context_contributors,
            hooks,
            verifiers,
            extensions,
        }
    }
}

fn compute_registry_fingerprint(
    tools: &BTreeMap<String, ToolRegistrationSnapshot>,
    context_contributors: &BTreeMap<String, ContextContributorSnapshot>,
    hooks: &[HookRegistration],
    verifiers: &[VerifierRegistration],
    extensions: &[String],
) -> RuntimeFingerprint {
    let mut hasher = Sha256::new();
    for (name, tool) in tools {
        hasher.update(b"tool:");
        hasher.update(name.as_bytes());
        hasher.update(b":");
        hasher.update(tool.schema_hash.as_bytes());
        hasher.update(b";");
    }
    for (name, contributor) in context_contributors {
        hasher.update(b"context:");
        hasher.update(name.as_bytes());
        hasher.update(b":");
        hasher.update(format!("{:?}", contributor.stability).as_bytes());
        if let Some(extension_id) = &contributor.extension_id {
            hasher.update(b":");
            hasher.update(extension_id.as_bytes());
        }
        hasher.update(b";");
    }
    for hook in hooks {
        hasher.update(b"hook:");
        hasher.update(hook.name.as_bytes());
        hasher.update(b";");
    }
    for verifier in verifiers {
        hasher.update(b"verifier:");
        hasher.update(verifier.name.as_bytes());
        hasher.update(b";");
    }
    for extension in extensions {
        hasher.update(b"extension:");
        hasher.update(extension.as_bytes());
        hasher.update(b";");
    }
    RuntimeFingerprint(format!("{:x}", hasher.finalize()))
}
