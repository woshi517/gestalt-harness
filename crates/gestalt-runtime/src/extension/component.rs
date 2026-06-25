use crate::manifest::{
    ContextInjectorDeclaration, Entrypoint, HookDeclaration, Permissions, ToolDeclaration,
};

use super::ComponentInstanceId;
use super::ExtensionGrantConfig;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComponentKind {
    LegacyProcess,
    GestaltLifecycle,
    CommandTool,
    McpServer,
    Skill,
    ClientProduct,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionComponentDescriptor {
    pub id: String,
    pub kind: ComponentKind,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub entrypoint: Option<Entrypoint>,
    #[serde(default)]
    pub descriptor: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub risk: Option<gestalt_core::tool::RiskLevel>,
    #[serde(default)]
    pub read_only: Option<bool>,
    #[serde(default)]
    pub idempotent: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedExtensionComponent {
    pub id: ComponentInstanceId,
    pub kind: ComponentKind,
    pub optional: bool,
    pub entrypoint: Entrypoint,
    pub descriptor: Option<String>,
    pub config: serde_json::Value,
    pub grants: ExtensionGrantConfig,
    pub tools: Vec<ToolDeclaration>,
    pub hooks: Vec<HookDeclaration>,
    pub context_injectors: Vec<ContextInjectorDeclaration>,
    pub permissions: Permissions,
    pub protocol_version: Option<String>,
    pub description: Option<String>,
    pub input_schema: Option<serde_json::Value>,
    pub risk: Option<gestalt_core::tool::RiskLevel>,
    pub read_only: bool,
    pub idempotent: bool,
}
