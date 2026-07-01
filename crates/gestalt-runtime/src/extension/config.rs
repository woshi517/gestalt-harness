use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

fn default_extension_instance_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInstanceConfig {
    pub package: String,
    #[serde(default = "default_extension_instance_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub components: BTreeMap<String, bool>,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub grants: ExtensionGrantConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionGrantConfig {
    #[serde(default)]
    pub workspace_read: bool,
    #[serde(default)]
    pub workspace_write: bool,
    #[serde(default)]
    pub shell: bool,
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default)]
    pub allowed_paths: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionsConfig {
    #[serde(default)]
    pub instances: BTreeMap<String, ExtensionInstanceConfig>,
}
