use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize, Debug, Clone)]
pub struct ConnectReport {
    pub provider: String,
    pub status: String,
    pub profile_created: Option<String>,
    pub keychain_stored: bool,
}

/// An entry in the run log index listing.
#[derive(Serialize, Clone, Debug)]
pub struct RunIndexEntry {
    /// Unique run identifier.
    pub run_id: String,
    /// Absolute filesystem path to the run directory.
    pub path: PathBuf,
    /// Run start timestamp.
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Associated session identifier.
    pub session_id: String,
    /// LLM provider (e.g. "openai").
    pub provider: Option<String>,
    /// LLM model name.
    pub model: Option<String>,
    /// Whether the trace.jsonl file exists.
    pub trace_exists: bool,
    /// Whether the summary.md file exists.
    pub summary_exists: bool,
    /// Whether the cost.json file exists.
    pub cost_exists: bool,
    /// Number of generated workspace artifacts.
    pub artifact_count: usize,
    /// Current apparent status of the run.
    pub apparent_status: String,
    /// Input tokens consumed.
    pub total_input_tokens: Option<usize>,
    /// Output tokens consumed.
    pub total_output_tokens: Option<usize>,
    /// Estimated total cost of the run in USD.
    pub estimated_cost_usd: Option<f64>,
}

/// Report containing a list of run entries.
#[derive(Serialize, Debug, Clone)]
pub struct RunsListReport {
    /// List of indexed runs.
    pub runs: Vec<RunIndexEntry>,
}
