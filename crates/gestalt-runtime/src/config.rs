use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use gestalt_core::session::ExecutionMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub workspace_root: PathBuf,
    pub execution_mode: ExecutionMode,
    pub max_turns: usize,
    pub model: String,
    pub provider: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
    pub max_context_window: Option<usize>,
    pub reserved_output_tokens: Option<usize>,
    pub bash_timeout_secs: Option<u64>,
    pub max_output_tokens: Option<usize>,
    pub allow_network: bool,
    pub environment: HashMap<String, String>,
    pub enabled_cli_features: Vec<String>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            execution_mode: ExecutionMode::Confirm,
            max_turns: 10,
            model: String::new(),
            provider: String::new(),
            max_tokens: 4096,
            temperature: Some(0.0),
            max_context_window: None,
            reserved_output_tokens: None,
            bash_timeout_secs: None,
            max_output_tokens: None,
            allow_network: false,
            environment: HashMap::new(),
            enabled_cli_features: Vec::new(),
        }
    }
}
