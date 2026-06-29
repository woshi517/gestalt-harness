use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

pub mod activation;
pub mod discovery;
pub mod events;
pub mod index;
pub mod manifest;
pub mod policy;
pub mod resources;
pub mod contributor;

// Re-exports for ergonomic access
pub use activation::{
    load_skill_body, render_active_skill_instructions, ActivationEngine, ActivationState,
};
pub use discovery::SkillDiscovery;
pub use events::{ActivationReason, SkillEvent};
pub use index::SkillIndex;
pub use manifest::{SkillFile, SkillManifest};
pub use policy::{effective_tool_policy, SkillToolPolicy};
pub use resources::{
    resolve_skill_resource, resolve_skill_resource_tracked, ResourceAccessRecorder,
};

/// A discovered skill with only metadata loaded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub skill_root: PathBuf,
    pub manifest_path: PathBuf,
    pub manifest_hash: String,
    pub trust_level: SkillTrustLevel,
    pub source: SkillSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
}

/// Trust level assigned to a discovered skill based on its source.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillTrustLevel {
    /// Loaded from an explicit user-provided path.
    Explicit,
    /// Discovered from the workspace-local `.gestalt/skills/` directory.
    Workspace,
    /// Discovered from the global `~/.config/gestalt/skills/` directory.
    Global,
    /// Downloaded or from an untrusted source.
    Downloaded,
}

/// Where a skill was discovered from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SkillSource {
    ExplicitPath,
    WorkspaceLocal,
    GlobalConfig,
    Downloaded,
}

/// An active skill with its full instruction body loaded.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActiveSkill {
    pub descriptor: SkillDescriptor,
    pub full_body: String,
}

/// Errors that can occur when working with skills.
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("YAML parse error: {0}")]
    YamlParse(String),
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Skill not found: {0}")]
    NotFound(String),
    #[error("Resource path escapes skill root: {0}")]
    ResourceEscape(PathBuf),
}
