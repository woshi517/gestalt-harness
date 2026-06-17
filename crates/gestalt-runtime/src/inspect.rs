use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInspect {
    pub provider_name: String,
    pub provider_model: String,
    pub execution_mode: String,
    pub max_turns: usize,
    pub context_pipeline_version: String,
    pub tools: Vec<ToolInspectInfo>,
    pub tool_schema_hash: String,
    pub policy_fingerprint: Option<String>,
    pub policy_source_path: Option<String>,
    pub hooks: Vec<String>,
    pub hook_contract_hash: String,
    pub verifiers: Vec<String>,
    pub extensions: Vec<String>,
    pub context_injectors: Vec<String>,
    pub trace_sink_kind: Option<String>,
    pub trace_run_dir: Option<String>,
    pub workspace_root: String,
    pub enabled_cli_features: Vec<String>,
    pub discovered_skills: Vec<SkillInspectInfo>,
    pub active_skills: Vec<String>,
    pub skill_fingerprint: Option<String>,
    pub mcp_servers: Vec<gestalt_mcp::McpServerState>,
    pub mcp_discovery_threshold: Option<usize>,
    #[serde(default)]
    pub effective_config_fingerprint: Option<String>,
    #[serde(default)]
    pub variant_fingerprint: Option<String>,
    #[serde(default)]
    pub negotiated_protocol_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInspectInfo {
    pub name: String,
    pub manifest_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInspectInfo {
    pub name: String,
    pub schema_hash: String,
    #[serde(default)]
    pub backend: Option<String>,
}

pub fn compute_hook_contract_hash(hook_names: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted = hook_names.to_vec();
    sorted.sort();
    let mut hasher = Sha256::new();
    for name in sorted {
        hasher.update(name.as_bytes());
        hasher.update(b":");
    }
    format!("{:x}", hasher.finalize())
}

pub fn compute_policy_fingerprint(policies_content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(policies_content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn compute_variant_fingerprint(
    model: &str,
    provider: &str,
    max_tokens: u32,
    temperature: Option<f32>,
    top_p: Option<f32>,
    reasoning_effort: Option<&gestalt_core::provider::ReasoningEffort>,
    text_verbosity: Option<&gestalt_core::provider::TextVerbosity>,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(model.as_bytes());
    hasher.update(b"|");
    hasher.update(provider.as_bytes());
    hasher.update(b"|");
    hasher.update(max_tokens.to_string().as_bytes());
    hasher.update(b"|");
    if let Some(t) = temperature {
        hasher.update(t.to_string().as_bytes());
    }
    hasher.update(b"|");
    if let Some(p) = top_p {
        hasher.update(p.to_string().as_bytes());
    }
    hasher.update(b"|");
    if let Some(re) = reasoning_effort {
        hasher.update(format!("{:?}", re).as_bytes());
    }
    hasher.update(b"|");
    if let Some(tv) = text_verbosity {
        hasher.update(format!("{:?}", tv).as_bytes());
    }
    format!("{:x}", hasher.finalize())
}
