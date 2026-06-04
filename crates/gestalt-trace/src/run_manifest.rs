use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::fs::File;
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    New,
    Continue,
    Resume,
    Branch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Running,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompatibilityFingerprint {
    pub context_pipeline_version: String,
    pub tool_schema_hash: String,
    pub policy_fingerprint: String,
    pub hook_contract_hash: String,
    pub execution_mode: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunManifest {
    pub v: u32,
    pub session_id: String,
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub base_checkpoint: Option<u64>,
    pub run_kind: RunKind,
    pub created_at: DateTime<Utc>,
    pub lifecycle_state: LifecycleState,
    pub finalized_at: Option<DateTime<Utc>>,
    pub failure_kind: Option<String>,
    pub interrupted_phase: Option<String>,
    pub compatibility_fingerprint: CompatibilityFingerprint,
}

impl RunManifest {
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        let mut file = File::create(path)?;
        file.write_all(content.as_bytes())?;
        Ok(())
    }

    pub fn load_from(path: &Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let manifest = serde_json::from_str(&content)?;
        Ok(manifest)
    }
}

pub fn compute_tool_schema_hash(schemas: &[serde_json::Value]) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    let mut sorted_schemas = schemas.to_vec();
    sorted_schemas.sort_by_key(|s| s.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string());
    for s in &sorted_schemas {
        hasher.update(s.to_string().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub fn compute_policy_fingerprint(policies_content: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(policies_content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn compute_hook_contract_hash(hook_names: &[String]) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    let mut sorted = hook_names.to_vec();
    sorted.sort();
    for name in &sorted {
        hasher.update(name.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}
