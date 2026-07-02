use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverityV1 {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AppDiagnosticV1 {
    pub severity: DiagnosticSeverityV1,
    pub code: String,
    pub message: String,
    pub correlation_id: Option<String>,
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct AppErrorProjectionV1 {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Option<serde_json::Value>,
}

impl AppErrorProjectionV1 {
    pub fn from_harness_error(error: &gestalt_core::HarnessError) -> Self {
        let retryable = error.is_recoverable();
        match error {
            gestalt_core::HarnessError::Config(cfg_err) => Self {
                code: match cfg_err {
                    gestalt_core::ConfigError::FeatureDisabled { .. } => {
                        "FEATURE_DISABLED".to_string()
                    }
                    gestalt_core::ConfigError::UnsupportedLegacyConfig { .. } => {
                        "UNSUPPORTED_LEGACY_CONFIG".to_string()
                    }
                    gestalt_core::ConfigError::MissingVersion => {
                        "CONFIG_VERSION_MISSING".to_string()
                    }
                    gestalt_core::ConfigError::InvalidVersion => {
                        "CONFIG_VERSION_INVALID".to_string()
                    }
                    gestalt_core::ConfigError::UnsupportedVersion { .. } => {
                        "CONFIG_VERSION_UNSUPPORTED".to_string()
                    }
                    gestalt_core::ConfigError::InvalidValue { field, .. }
                        if field.starts_with("skills.") =>
                    {
                        "SKILL_CONFIGURATION_ERROR".to_string()
                    }
                    gestalt_core::ConfigError::InvalidValue { field, .. }
                        if field.starts_with("extensions.") =>
                    {
                        "EXTENSION_REJECTED".to_string()
                    }
                    _ => "CONFIG_ERROR".to_string(),
                },
                message: cfg_err.to_string(),
                retryable,
                details: match cfg_err {
                    gestalt_core::ConfigError::InvalidValue { field, reason } => {
                        Some(serde_json::json!({"field": field, "reason": reason}))
                    }
                    _ => None,
                },
            },
            gestalt_core::HarnessError::Provider(prov_err) => Self {
                code: match prov_err {
                    gestalt_core::ProviderError::UnknownProvider(_) => "PROVIDER_NOT_FOUND",
                    gestalt_core::ProviderError::AuthFailed { .. } => "AUTH_FAILED",
                    gestalt_core::ProviderError::RateLimit { .. } => "PROVIDER_RATE_LIMIT",
                    gestalt_core::ProviderError::ContextTooLong { .. } => "CONTEXT_TOO_LONG",
                    gestalt_core::ProviderError::InvalidModel { .. } => "MODEL_INVALID",
                    gestalt_core::ProviderError::Timeout => "PROVIDER_TIMEOUT",
                    gestalt_core::ProviderError::StreamInterrupted => "PROVIDER_STREAM_INTERRUPTED",
                    gestalt_core::ProviderError::MalformedToolCall { .. } => {
                        "PROVIDER_MALFORMED_TOOL_CALL"
                    }
                    gestalt_core::ProviderError::UnsupportedCapability { .. } => {
                        "PROVIDER_UNSUPPORTED_CAPABILITY"
                    }
                    gestalt_core::ProviderError::UnexpectedResponse { .. }
                    | gestalt_core::ProviderError::Transport(_) => "PROVIDER_ERROR",
                }
                .to_string(),
                message: prov_err.to_string(),
                retryable,
                details: None,
            },
            gestalt_core::HarnessError::Policy(pol_err) => Self {
                code: match pol_err {
                    gestalt_core::PolicyError::Denied(_) => "POLICY_DENIED",
                    gestalt_core::PolicyError::InvalidPolicy(_) => "POLICY_INVALID",
                    gestalt_core::PolicyError::Io(_) => "POLICY_ERROR",
                }
                .to_string(),
                message: pol_err.to_string(),
                retryable,
                details: None,
            },
            gestalt_core::HarnessError::Context(ctx_err) => Self {
                code: "CONTEXT_ERROR".to_string(),
                message: ctx_err.to_string(),
                retryable,
                details: None,
            },
            gestalt_core::HarnessError::Tool(tool_err) => Self {
                code: match tool_err {
                    gestalt_core::ToolError::NotFound(_) => "TOOL_NOT_FOUND",
                    gestalt_core::ToolError::PathNotAllowed(_)
                    | gestalt_core::ToolError::NetworkDenied(_)
                    | gestalt_core::ToolError::Denied(_) => "TOOL_PERMISSION_DENIED",
                    _ => "TOOL_ERROR",
                }
                .to_string(),
                message: tool_err.to_string(),
                retryable,
                details: None,
            },
            gestalt_core::HarnessError::Trace(trace_err) => Self {
                code: "TRACE_ERROR".to_string(),
                message: trace_err.to_string(),
                retryable,
                details: None,
            },
            gestalt_core::HarnessError::Approval(approval_err) => Self {
                code: match approval_err {
                    gestalt_core::ApprovalError::Rejected(_) => "APPROVAL_REJECTED",
                    gestalt_core::ApprovalError::Io(_) => "APPROVAL_ERROR",
                }
                .to_string(),
                message: approval_err.to_string(),
                retryable,
                details: None,
            },
            gestalt_core::HarnessError::Cancelled => Self {
                code: "CANCELLED".to_string(),
                message: "Execution was cancelled".to_string(),
                retryable,
                details: None,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ServiceReportV1<T> {
    pub value: Option<T>,
    pub diagnostics: Vec<AppDiagnosticV1>,
    pub error: Option<AppErrorProjectionV1>,
    pub correlation_id: Option<String>,
}

impl<T> ServiceReportV1<T> {
    pub fn new(value: T) -> Self {
        Self {
            value: Some(value),
            diagnostics: Vec::new(),
            error: None,
            correlation_id: None,
        }
    }

    pub fn failure(error: AppErrorProjectionV1) -> Self {
        Self {
            value: None,
            diagnostics: Vec::new(),
            error: Some(error),
            correlation_id: None,
        }
    }

    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: Vec<AppDiagnosticV1>) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    #[must_use]
    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }
}

pub trait DiagnosticSinkV1: Send + Sync {
    fn emit(&self, diagnostic: AppDiagnosticV1);
}

impl<F> DiagnosticSinkV1 for F
where
    F: Fn(AppDiagnosticV1) + Send + Sync,
{
    fn emit(&self, diagnostic: AppDiagnosticV1) {
        self(diagnostic);
    }
}

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

/// Report containing metrics of pruned runs.
#[derive(Serialize, Debug, Clone)]
pub struct RunsPruneReport {
    /// List of pruned run identifiers.
    pub pruned_runs: Vec<String>,
    /// Reclaimed disk space in bytes.
    pub reclaimed_bytes: u64,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

/// Report containing metrics of a deleted run.
#[derive(Serialize, Debug, Clone)]
pub struct RunsDeleteReport {
    /// Deleted run identifier.
    pub deleted_run: String,
    /// Reclaimed disk space in bytes.
    pub reclaimed_bytes: u64,
}

#[derive(Serialize, Debug, Clone)]
pub struct ContextExplainReport {
    pub prompt: Option<String>,
    pub run_id: Option<String>,
    pub token_estimate: usize,
    pub packet_hash: String,
    pub pipeline_version: String,
    pub prompt_source: Option<String>,
    pub system_prompt: Option<String>,
    pub sources: Vec<gestalt_runtime::unstable::ContextSourceReportV1>,
    pub omissions: Vec<gestalt_runtime::unstable::ContextOmissionReportV1>,
}

#[derive(Serialize, Debug, Clone)]
pub struct RunsInspectReport {
    /// Unique run identifier.
    pub run_id: String,
    /// Absolute filesystem path to the run directory.
    pub path: PathBuf,
    /// Run start timestamp.
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    /// Associated session identifier.
    pub session_id: String,
    pub parent_run_id: Option<String>,
    pub run_kind: Option<String>,
    pub lifecycle_state: Option<String>,
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
    /// Current apparent status of the run.
    pub apparent_status: String,
    /// Total turns executed.
    pub turns: Option<usize>,
    /// Stop reason description.
    pub stop_reason: Option<String>,
    /// Total input tokens consumed.
    pub total_input_tokens: Option<usize>,
    /// Total output tokens consumed.
    pub total_output_tokens: Option<usize>,
    /// Estimated total cost of the run in USD.
    pub estimated_cost_usd: Option<f64>,
    /// Workspace snapshot identifier.
    pub workspace_snapshot_id: Option<String>,
    /// List of generated workspace artifacts.
    pub artifacts: Vec<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct AuthResolveReport {
    pub provider: String,
    pub source: String,
    pub variable: String,
    pub status: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct AuthDoctorEntry {
    pub variable: String,
    pub status: String,
    pub value: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct AuthDoctorReport {
    pub entries: Vec<AuthDoctorEntry>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ProviderDoctorResult {
    pub provider: String,
    pub auth_variable: String,
    pub auth_status: String,
    pub auth_source: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct ProvidersDoctorReport {
    pub results: Vec<ProviderDoctorResult>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ModelsRefreshReport {
    pub count: usize,
    pub status: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct ProfileInfoEntry {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub active: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct ProfilesListReport {
    pub profiles: Vec<ProfileInfoEntry>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ProfilesInspectReport {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub active: bool,
    pub resolved_provider_kind: String,
    pub resolved_base_url: Option<String>,
    pub resolved_auth_ref: Option<String>,
    pub resolved_api_key_env: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ProfilesUseReport {
    pub name: String,
    pub active: bool,
    pub file_updated: PathBuf,
}

#[derive(Serialize, Debug, Clone)]
pub struct DisconnectReport {
    pub provider: String,
    pub profile_removed: Option<String>,
    pub keychain_cleared: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct WorkspaceDoctorReport {
    pub workspace_root: PathBuf,
    pub config_valid: bool,
    pub config_error: Option<String>,
    pub policies_valid: bool,
    pub policies_error: Option<String>,
    pub missing_files: Vec<String>,
    pub auth_summary: HashMap<String, String>,
    pub run_dir_exists: bool,
    pub run_dir_writable: Option<bool>,
    pub selected_model: Option<String>,
    pub model_valid: bool,
    pub model_error: Option<String>,
    pub memory_writable: Option<bool>,
    pub memory_write_error: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct GlobalDoctorReport {
    pub workspace_doctor: WorkspaceDoctorReport,
    pub live: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct VerifierResultEntry {
    pub name: String,
    pub status: gestalt_core::event::VerificationStatus,
    pub findings: Vec<gestalt_core::event::VerificationFinding>,
    pub report: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ArtifactVerificationResult {
    pub artifact_path: String,
    pub verifiers: Vec<VerifierResultEntry>,
}

#[derive(Serialize, Debug, Clone)]
pub struct VerifyRunReport {
    pub run_id: String,
    pub status: gestalt_core::event::VerificationStatus,
    pub total_checks: usize,
    pub total_failed: usize,
    pub artifacts: Vec<ArtifactVerificationResult>,
}

#[derive(Serialize, Debug, Clone)]
pub struct SkillListEntry {
    pub name: String,
    pub description: String,
    pub trust_level: String,
    pub source: String,
    pub manifest_path: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct SkillsListReport {
    pub skills: Vec<SkillListEntry>,
}

#[derive(Serialize, Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub runs_count: usize,
    pub latest_run_id: String,
    pub latest_run_status: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub total_turns: usize,
    pub estimated_cost_usd: f64,
}

#[derive(Serialize, Debug, Clone)]
pub struct SessionsListReport {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Serialize, Debug, Clone)]
pub struct SessionInspectReport {
    pub session_id: String,
    pub runs: Vec<RunManifestSummary>,
}

#[derive(Serialize, Debug, Clone)]
pub struct RunManifestSummary {
    pub run_id: String,
    pub dir_name: String,
    pub parent_run_id: Option<String>,
    pub run_kind: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub lifecycle_state: String,
    pub turns: usize,
}

#[derive(Serialize, Debug, Clone)]
pub struct SessionHistoryReport {
    pub session_id: String,
    pub timeline: Vec<TimelineItem>,
}

#[derive(Serialize, Debug, Clone)]
pub struct TimelineItem {
    pub run_id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event_summary: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct WorkspaceInitReport {
    pub workspace_root: PathBuf,
    pub created_files: Vec<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct WorkspaceStatusReport {
    pub workspace_root: PathBuf,
    pub config_valid: bool,
    pub active_provider: Option<String>,
    pub active_model: Option<String>,
    pub active_mode: Option<String>,
    pub recent_runs_count: usize,
    pub auth_summary: HashMap<String, String>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct WorkspaceInfoReport {
    pub workspace_root: PathBuf,
    pub config_path: PathBuf,
    pub workspace_md_path: PathBuf,
    pub memory_md_path: PathBuf,
}

#[derive(Serialize, Debug, Clone)]
pub struct WorkspaceSnapshotReport {
    pub snapshot: gestalt_core::snapshot::WorkspaceSnapshot,
}
