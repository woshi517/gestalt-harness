use serde::Serialize;
use std::path::PathBuf;
use std::collections::HashMap;

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
    pub sources: Vec<gestalt_core::context::ContextSourceRef>,
    pub omissions: Vec<gestalt_core::context::ContextOmission>,
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

pub fn render_event(event: &gestalt_core::AgentEvent) -> Option<String> {
    match event {
        gestalt_core::AgentEvent::UserMessage { content } => Some(format!("user> {content}")),
        gestalt_core::AgentEvent::ContextBuilt {
            token_estimate,
            packet_hash,
            ..
        } => {
            let mut extra = String::new();
            if let Some(h) = packet_hash {
                extra.push_str(&format!(" packet_hash={}", &h[..8.min(h.len())]));
            }
            Some(format!("context> {token_estimate} tokens{extra}"))
        }
        gestalt_core::AgentEvent::PromptSnapshotCreated {
            snapshot_hash,
            prefix_hash,
            created_turn,
        } => Some(format!(
            "snapshot-created> {} prefix={} turn={created_turn}",
            &snapshot_hash[..8.min(snapshot_hash.len())],
            &prefix_hash[..8.min(prefix_hash.len())]
        )),
        gestalt_core::AgentEvent::PromptSnapshotLoaded {
            snapshot_hash,
            source,
        } => Some(format!(
            "snapshot-loaded> {} source={source}",
            &snapshot_hash[..8.min(snapshot_hash.len())]
        )),
        gestalt_core::AgentEvent::PromptSnapshotReused {
            snapshot_hash,
            prefix_hash,
        } => Some(format!(
            "snapshot-reused> {} prefix={}",
            &snapshot_hash[..8.min(snapshot_hash.len())],
            &prefix_hash[..8.min(prefix_hash.len())]
        )),
        gestalt_core::AgentEvent::PromptCachePlanGenerated {
            snapshot_hash,
            prefix_hash,
            prefix_message_count,
        } => Some(format!(
            "cache-plan> {} prefix={} messages={prefix_message_count}",
            &snapshot_hash[..8.min(snapshot_hash.len())],
            &prefix_hash[..8.min(prefix_hash.len())]
        )),
        gestalt_core::AgentEvent::EphemeralContextInjected {
            source,
            token_estimate,
        } => Some(format!("ephemeral> {source} ({token_estimate} tokens)")),
        gestalt_core::AgentEvent::ModelRequest {
            provider,
            model,
            packet_hash,
            temperature,
            max_tokens,
            provider_request_hash,
            ..
        } => {
            let mut extra = String::new();
            if let Some(h) = packet_hash {
                extra.push_str(&format!(" packet_hash={}", &h[..8.min(h.len())]));
            }
            if let Some(t) = temperature {
                extra.push_str(&format!(" temp={t}"));
            }
            if let Some(m) = max_tokens {
                extra.push_str(&format!(" max_tokens={m}"));
            }
            if let Some(h) = provider_request_hash {
                extra.push_str(&format!(" request_hash={}", &h[..8.min(h.len())]));
            }
            Some(format!("model> {provider}/{model}{extra}"))
        }
        gestalt_core::AgentEvent::Text { delta } => Some(format!("assistant> {delta}")),
        gestalt_core::AgentEvent::Thinking { delta } => Some(format!("thinking> {delta}")),
        gestalt_core::AgentEvent::ToolCallStreamed { .. } => None,
        gestalt_core::AgentEvent::ToolCallProposed { id, name, input } => {
            Some(format!("tool> {name}#{id} {input}"))
        }
        gestalt_core::AgentEvent::PolicyDecision {
            tool_call_id,
            tool_name,
            input_hash,
            risk,
            mode,
            matched_rule,
            decision,
            reason,
            policy_source,
        } => {
            let mut extra = String::new();
            if let Some(name) = tool_name {
                extra.push_str(&format!(" tool={name}"));
            }
            if let Some(level) = risk {
                extra.push_str(&format!(" risk={level:?}"));
            }
            if let Some(m) = mode {
                extra.push_str(&format!(" mode={m:?}"));
            }
            if let Some(rule) = matched_rule {
                extra.push_str(&format!(" rule={rule}"));
            }
            if let Some(hash) = input_hash {
                extra.push_str(&format!(" input={}", &hash[..8.min(hash.len())]));
            }
            Some(format!(
                "policy> {tool_call_id} {decision:?} source={policy_source}{extra} {}",
                reason.clone().unwrap_or_default()
            ))
        }
        gestalt_core::AgentEvent::ApprovalDecision {
            tool_call_id,
            decision,
            original_input_hash,
            edited_input_hash,
            grant_terms,
        } => {
            let short = |s: &str| s.chars().take(8).collect::<String>();
            let grant = grant_terms
                .as_ref()
                .map(|g| format!(" grant={}#{}", g.tool_name, short(&g.input_hash)))
                .unwrap_or_default();
            let edited = edited_input_hash
                .as_ref()
                .map(|h| format!(" edited={}", short(h)))
                .unwrap_or_default();
            Some(format!(
                "approval> {tool_call_id} {decision:?} orig={}{}{}",
                short(original_input_hash),
                edited,
                grant
            ))
        }
        gestalt_core::AgentEvent::ToolResult {
            id,
            output,
            is_error,
            truncated,
            tool_name,
            working_dir,
            duration_ms,
            output_hash,
            artifact_refs,
            policy_source,
            failure,
        } => {
            let mut extra = String::new();
            if let Some(name) = tool_name {
                extra.push_str(&format!(" name={name}"));
            }
            if let Some(dir) = working_dir {
                extra.push_str(&format!(" dir={dir}"));
            }
            if let Some(ms) = duration_ms {
                extra.push_str(&format!(" duration={ms}ms"));
            }
            if let Some(h) = output_hash {
                extra.push_str(&format!(" hash={}", &h[..8.min(h.len())]));
            }
            if let Some(refs) = artifact_refs {
                if !refs.is_empty() {
                    extra.push_str(&format!(" artifacts={}", refs.join(",")));
                }
            }
            if let Some(src) = policy_source {
                extra.push_str(&format!(" policy_source={src}"));
            }
            if let Some(failure) = failure {
                extra.push_str(&format!(" failure={}", failure.kind));
                if let Some(guidance) = &failure.repair_guidance {
                    extra.push_str(&format!(
                        " repair={}",
                        guidance.chars().take(60).collect::<String>()
                    ));
                }
            }
            Some(format!(
                "tool-result> {id} error={is_error} truncated={truncated}{extra} {output}"
            ))
        }
        gestalt_core::AgentEvent::ArtifactCreated {
            path,
            size_bytes,
            mime_type,
            hash,
        } => Some(format!(
            "artifact-created> {path} size={size_bytes} mime={mime_type} hash={}",
            &hash[..8.min(hash.len())]
        )),
        gestalt_core::AgentEvent::PolicyViolation {
            tool_call_id,
            tool_name,
            reason,
        } => Some(format!(
            "policy-violation> {tool_call_id} tool={tool_name} reason={reason}"
        )),
        gestalt_core::AgentEvent::MemoryProposal { diff } => Some(format!("memory> {diff}")),
        gestalt_core::AgentEvent::VerificationResult { report, .. } => report.clone(),
        gestalt_core::AgentEvent::Usage {
            input_tokens,
            output_tokens,
        } => Some(format!("usage> in={input_tokens} out={output_tokens}")),
        gestalt_core::AgentEvent::Stop { reason } => Some(format!("stop> {reason:?}")),
        gestalt_core::AgentEvent::Error {
            message,
            recoverable,
        } => Some(format!("error> recoverable={recoverable} {message}")),
        gestalt_core::AgentEvent::WorkspaceSnapshotCaptured { snapshot_id, dirty } => {
            Some(format!("snapshot> id={snapshot_id} dirty={dirty}"))
        }
        _ => None,
    }
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

