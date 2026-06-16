use gestalt_core::{model::ModelInfo, AgentEvent};
use gestalt_trace::CostReport;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

#[allow(clippy::format_push_string)]
pub fn render_event(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::UserMessage { content } => Some(format!("user> {content}")),
        AgentEvent::ContextBuilt {
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
        AgentEvent::PromptSnapshotCreated {
            snapshot_hash,
            prefix_hash,
            created_turn,
        } => Some(format!(
            "snapshot-created> {} prefix={} turn={created_turn}",
            &snapshot_hash[..8.min(snapshot_hash.len())],
            &prefix_hash[..8.min(prefix_hash.len())]
        )),
        AgentEvent::PromptSnapshotLoaded {
            snapshot_hash,
            source,
        } => Some(format!(
            "snapshot-loaded> {} source={source}",
            &snapshot_hash[..8.min(snapshot_hash.len())]
        )),
        AgentEvent::PromptSnapshotReused {
            snapshot_hash,
            prefix_hash,
        } => Some(format!(
            "snapshot-reused> {} prefix={}",
            &snapshot_hash[..8.min(snapshot_hash.len())],
            &prefix_hash[..8.min(prefix_hash.len())]
        )),
        AgentEvent::PromptCachePlanGenerated {
            snapshot_hash,
            prefix_hash,
            prefix_message_count,
        } => Some(format!(
            "cache-plan> {} prefix={} messages={prefix_message_count}",
            &snapshot_hash[..8.min(snapshot_hash.len())],
            &prefix_hash[..8.min(prefix_hash.len())]
        )),
        AgentEvent::EphemeralContextInjected {
            source,
            token_estimate,
        } => Some(format!("ephemeral> {source} ({token_estimate} tokens)")),
        AgentEvent::ModelRequest {
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
        AgentEvent::Text { delta } => Some(format!("assistant> {delta}")),
        AgentEvent::Thinking { delta } => Some(format!("thinking> {delta}")),
        AgentEvent::ToolCallStreamed { .. } => None,
        AgentEvent::ToolCallProposed { id, name, input } => {
            Some(format!("tool> {name}#{id} {input}"))
        }
        AgentEvent::PolicyDecision {
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
        AgentEvent::ApprovalDecision {
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
        AgentEvent::ToolResult {
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
        AgentEvent::ArtifactCreated {
            path,
            size_bytes,
            mime_type,
            hash,
        } => Some(format!(
            "artifact-created> {path} size={size_bytes} mime={mime_type} hash={}",
            &hash[..8.min(hash.len())]
        )),
        AgentEvent::PolicyViolation {
            tool_call_id,
            tool_name,
            reason,
        } => Some(format!(
            "policy-violation> {tool_call_id} tool={tool_name} reason={reason}"
        )),
        AgentEvent::MemoryProposal { diff } => Some(format!("memory> {diff}")),
        AgentEvent::VerificationResult { report, .. } => report.clone(),
        AgentEvent::Usage {
            input_tokens,
            output_tokens,
        } => Some(format!("usage> in={input_tokens} out={output_tokens}")),
        AgentEvent::Stop { reason } => Some(format!("stop> {reason:?}")),
        AgentEvent::Error {
            message,
            recoverable,
        } => Some(format!("error> recoverable={recoverable} {message}")),
        AgentEvent::WorkspaceSnapshotCaptured { snapshot_id, dirty } => {
            Some(format!("snapshot> id={snapshot_id} dirty={dirty}"))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Serialize)]
pub struct JsonEnvelope<T> {
    pub schema_version: u32,
    pub kind: String,
    pub data: T,
}

#[derive(Serialize)]
pub struct CliErrorPayload {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

pub trait CliReport: Serialize {
    fn kind(&self) -> &'static str;
    fn render_text(&self) -> String;
}

#[derive(Serialize)]
pub struct RunReport {
    pub run_dir: PathBuf,
}

impl CliReport for RunReport {
    fn kind(&self) -> &'static str {
        "run"
    }
    fn render_text(&self) -> String {
        format!("run_dir={}", self.run_dir.display())
    }
}

#[derive(Serialize)]
pub struct ReplayReport {
    pub rendered: String,
}

impl CliReport for ReplayReport {
    fn kind(&self) -> &'static str {
        "replay"
    }
    fn render_text(&self) -> String {
        self.rendered.clone()
    }
}

#[derive(Serialize)]
pub struct CostReportWrapper(pub CostReport);

impl CliReport for CostReportWrapper {
    fn kind(&self) -> &'static str {
        "cost"
    }
    fn render_text(&self) -> String {
        crate::cost::render_cost(&self.0)
    }
}

#[derive(Serialize)]
pub struct ConfigValidateReport {
    pub workspace_root: PathBuf,
}

impl CliReport for ConfigValidateReport {
    fn kind(&self) -> &'static str {
        "config.validate"
    }
    fn render_text(&self) -> String {
        format!("valid workspace={}", self.workspace_root.display())
    }
}

#[derive(Serialize)]
pub struct AuthResolveReport {
    pub provider: String,
    pub source: String,
    pub variable: String,
    pub status: String,
}

impl CliReport for AuthResolveReport {
    fn kind(&self) -> &'static str {
        "auth.resolve"
    }
    fn render_text(&self) -> String {
        format!(
            "provider={} source={} variable={} status={}",
            self.provider, self.source, self.variable, self.status
        )
    }
}

#[derive(Serialize)]
pub struct AuthDoctorEntry {
    pub variable: String,
    pub status: String,
    pub value: String,
}

#[derive(Serialize)]
pub struct AuthDoctorReport {
    pub entries: Vec<AuthDoctorEntry>,
}

impl CliReport for AuthDoctorReport {
    fn kind(&self) -> &'static str {
        "auth.doctor"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!(
                "{:<30} | {:<10} | {:<15}",
                "Environment Variable", "Status", "Value"
            ),
            "-".repeat(61),
        ];
        for entry in &self.entries {
            lines.push(format!(
                "{:<30} | {:<10} | {:<15}",
                entry.variable, entry.status, entry.value
            ));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct ProvidersListReport {
    pub providers: Vec<String>,
}

impl CliReport for ProvidersListReport {
    fn kind(&self) -> &'static str {
        "providers.list"
    }
    fn render_text(&self) -> String {
        self.providers.join("\n")
    }
}

#[derive(Serialize)]
pub struct ProvidersInspectReport {
    pub provider: String,
    pub config: Value,
}

impl CliReport for ProvidersInspectReport {
    fn kind(&self) -> &'static str {
        "providers.inspect"
    }
    fn render_text(&self) -> String {
        serde_json::to_string_pretty(&self.config).unwrap_or_default()
    }
}

#[derive(Serialize)]
pub struct ProviderDoctorResult {
    pub provider: String,
    pub auth_variable: String,
    pub auth_status: String,
    pub auth_source: String,
}

#[derive(Serialize)]
pub struct ProvidersDoctorReport {
    pub results: Vec<ProviderDoctorResult>,
}

impl CliReport for ProvidersDoctorReport {
    fn kind(&self) -> &'static str {
        "providers.doctor"
    }
    fn render_text(&self) -> String {
        self.results
            .iter()
            .map(|r| {
                format!(
                    "provider={}\nprovider={} source={} variable={} status={}",
                    r.provider, r.provider, r.auth_source, r.auth_variable, r.auth_status
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Serialize)]
pub struct ModelsListReport {
    pub models: Vec<ModelInfo>,
}

impl CliReport for ModelsListReport {
    fn kind(&self) -> &'static str {
        "models.list"
    }
    fn render_text(&self) -> String {
        let mut lines = vec![
            format!(
                "{:<40} | {:<12} | {:<12} | {:<6} | {:<6} | {:<8} | {:<6}",
                "Qualified ID", "Input $/M", "Output $/M", "Vision", "Tools", "Thinking", "Cache"
            ),
            "-".repeat(110),
        ];
        for m in &self.models {
            let input_cost = m
                .input_cost_per_million
                .map_or("N/A".to_string(), |c| format!("${:.2}", c));
            let output_cost = m
                .output_cost_per_million
                .map_or("N/A".to_string(), |c| format!("${:.2}", c));
            lines.push(format!(
                "{:<40} | {:<12} | {:<12} | {:<6} | {:<6} | {:<8} | {:<6}",
                m.qualified_id,
                input_cost,
                output_cost,
                if m.supports_vision { "yes" } else { "no" },
                if m.supports_tools { "yes" } else { "no" },
                if m.supports_thinking { "yes" } else { "no" },
                if m.supports_prompt_caching {
                    "yes"
                } else {
                    "no"
                }
            ));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct ModelsInspectReport {
    pub model: ModelInfo,
}

impl CliReport for ModelsInspectReport {
    fn kind(&self) -> &'static str {
        "models.inspect"
    }
    fn render_text(&self) -> String {
        serde_json::to_string_pretty(&self.model).unwrap_or_default()
    }
}

#[derive(Serialize)]
pub struct ModelsRefreshReport {
    pub count: usize,
    pub status: String,
}

impl CliReport for ModelsRefreshReport {
    fn kind(&self) -> &'static str {
        "models.refresh"
    }
    fn render_text(&self) -> String {
        match self.status.as_str() {
            "offline" => format!(
                "built-in catalog available: {} models (offline)",
                self.count
            ),
            "live requested" => format!("live refresh requested: {} models (offline)", self.count),
            "live performed" => format!("refreshed live catalog: {} models", self.count),
            "unsupported" => format!("live refresh unsupported: {} models (offline)", self.count),
            _ => format!("built-in catalog available: {} models", self.count),
        }
    }
}

#[derive(Serialize)]
pub struct ModelsSelectReport {
    pub qualified_id: String,
    pub display_name: String,
}

impl CliReport for ModelsSelectReport {
    fn kind(&self) -> &'static str {
        "models.select"
    }
    fn render_text(&self) -> String {
        format!("selected {} ({})", self.qualified_id, self.display_name)
    }
}

#[derive(Serialize)]
pub struct WorkspaceInitReport {
    pub workspace_root: PathBuf,
    pub created_files: Vec<String>,
}

impl CliReport for WorkspaceInitReport {
    fn kind(&self) -> &'static str {
        "workspace.init"
    }
    fn render_text(&self) -> String {
        format!(
            "initialized workspace={}\ncreated files:\n{}",
            self.workspace_root.display(),
            self.created_files
                .iter()
                .map(|f| format!("  - {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

#[derive(Serialize)]
pub struct WorkspaceStatusReport {
    pub workspace_root: PathBuf,
    pub config_valid: bool,
    pub active_provider: Option<String>,
    pub active_model: Option<String>,
    pub active_mode: Option<String>,
    pub recent_runs_count: usize,
    pub auth_summary: std::collections::HashMap<String, String>,
    pub warnings: Vec<String>,
}

impl CliReport for WorkspaceStatusReport {
    fn kind(&self) -> &'static str {
        "workspace.status"
    }
    fn render_text(&self) -> String {
        let mut lines = vec![
            format!("workspace_root={}", self.workspace_root.display()),
            format!("config_valid={}", self.config_valid),
        ];
        if let Some(ref p) = self.active_provider {
            lines.push(format!("active_provider={p}"));
        }
        if let Some(ref m) = self.active_model {
            lines.push(format!("active_model={m}"));
        }
        if let Some(ref mode) = self.active_mode {
            lines.push(format!("active_mode={mode}"));
        }
        lines.push(format!("recent_runs_count={}", self.recent_runs_count));

        let mut auths: Vec<_> = self.auth_summary.iter().collect();
        auths.sort_by_key(|(k, _)| *k);
        for (provider, status) in auths {
            lines.push(format!("auth.{provider}={status}"));
        }

        if !self.warnings.is_empty() {
            lines.push("warnings:".to_string());
            for warning in &self.warnings {
                lines.push(format!("  - {warning}"));
            }
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct WorkspaceInfoReport {
    pub workspace_root: PathBuf,
    pub config_path: PathBuf,
    pub workspace_md_path: PathBuf,
    pub memory_md_path: PathBuf,
}

impl CliReport for WorkspaceInfoReport {
    fn kind(&self) -> &'static str {
        "workspace.info"
    }
    fn render_text(&self) -> String {
        format!(
            "workspace_root={}\nconfig_path={}\nworkspace_md_path={}\nmemory_md_path={}",
            self.workspace_root.display(),
            self.config_path.display(),
            self.workspace_md_path.display(),
            self.memory_md_path.display()
        )
    }
}

#[derive(Serialize)]
pub struct WorkspaceSnapshotReport {
    pub snapshot: gestalt_core::snapshot::WorkspaceSnapshot,
}

impl CliReport for WorkspaceSnapshotReport {
    fn kind(&self) -> &'static str {
        "workspace.snapshot"
    }
    fn render_text(&self) -> String {
        format!(
            "workspace_root={}\ngit_sha={}\ngit_dirty={}\nuntracked_count={}\ncontent_hash={}\ncaptured_at={}",
            self.snapshot.workspace_root.display(),
            self.snapshot.git_sha.as_deref().unwrap_or("none"),
            self.snapshot.git_dirty.unwrap_or(false),
            self.snapshot.untracked_count.unwrap_or(0),
            self.snapshot.content_hash,
            self.snapshot.captured_at
        )
    }
}

#[derive(Serialize)]
pub struct WorkspaceDoctorReport {
    pub workspace_root: PathBuf,
    pub config_valid: bool,
    pub config_error: Option<String>,
    pub policies_valid: bool,
    pub policies_error: Option<String>,
    pub missing_files: Vec<String>,
    pub auth_summary: std::collections::HashMap<String, String>,
    pub run_dir_exists: bool,
    pub run_dir_writable: Option<bool>,
    pub selected_model: Option<String>,
    pub model_valid: bool,
    pub model_error: Option<String>,
    pub memory_writable: Option<bool>,
    pub memory_write_error: Option<String>,
}

#[derive(Serialize)]
pub struct GlobalDoctorReport {
    pub workspace_doctor: WorkspaceDoctorReport,
    pub live: bool,
}

impl CliReport for GlobalDoctorReport {
    fn kind(&self) -> &'static str {
        "doctor"
    }

    fn render_text(&self) -> String {
        let mut passes = Vec::new();
        let mut warnings = Vec::new();
        let mut failures = Vec::new();

        // 1. Config Check
        if self.workspace_doctor.config_valid {
            passes.push(format!(
                "Configuration: valid (root: {})",
                self.workspace_doctor.workspace_root.display()
            ));
        } else {
            failures.push(format!(
                "Configuration: invalid. Details: {}",
                self.workspace_doctor
                    .config_error
                    .as_deref()
                    .unwrap_or("unknown error")
            ));
        }

        // 2. Policies Check
        if self.workspace_doctor.policies_valid {
            passes.push("Policies: syntax valid".to_string());
        } else {
            failures.push(format!(
                "Policies: invalid syntax. Details: {}",
                self.workspace_doctor
                    .policies_error
                    .as_deref()
                    .unwrap_or("unknown error")
            ));
        }

        // 2b. Selected Model Check
        if let Some(ref model) = self.workspace_doctor.selected_model {
            if self.workspace_doctor.model_valid {
                passes.push(format!("Selected model '{model}': exists in catalog"));
            } else {
                failures.push(format!(
                    "Selected model '{model}': {}",
                    self.workspace_doctor
                        .model_error
                        .as_deref()
                        .unwrap_or("not found in catalog")
                ));
            }
        }

        // 3. Workspace Files Check
        if self.workspace_doctor.missing_files.is_empty() {
            passes.push("Workspace files: all required files (gestalt.json, workspace.md, memory.md) present".to_string());
        } else {
            warnings.push(format!(
                "Workspace files: missing files: {}",
                self.workspace_doctor.missing_files.join(", ")
            ));
        }

        // 4. Writability Check
        if self.workspace_doctor.run_dir_exists {
            match self.workspace_doctor.run_dir_writable {
                Some(true) => passes.push("Runs directory: exists and writable".to_string()),
                Some(false) => failures.push("Runs directory: exists but NOT writable".to_string()),
                None => warnings
                    .push("Runs directory: exists but writability status is unknown".to_string()),
            }
        } else {
            warnings.push(
                "Runs directory: does not exist yet (will be created on first run)".to_string(),
            );
        }

        // 5. Auth / Providers Check
        let mut auths: Vec<_> = self.workspace_doctor.auth_summary.iter().collect();
        auths.sort_by_key(|(k, _)| *k);
        for (provider, status) in auths {
            if status == "present" || status == "ready" {
                passes.push(format!(
                    "Provider '{provider}': credentials status is {status}"
                ));
            } else if status == "missing" {
                warnings.push(format!("Provider '{provider}': credentials missing"));
            } else {
                failures.push(format!("Provider '{provider}': probe failed with {status}"));
            }
        }

        let mut output = Vec::new();
        output.push("=== GESTALT DIAGNOSTICS ===".to_string());

        if !failures.is_empty() {
            output.push(String::new());
            output.push("FAILURES:".to_string());
            for f in failures {
                output.push(format!("  [FAIL] {f}"));
            }
        }

        if !warnings.is_empty() {
            output.push(String::new());
            output.push("WARNINGS:".to_string());
            for w in warnings {
                output.push(format!("  [WARN] {w}"));
            }
        }

        if !passes.is_empty() {
            output.push(String::new());
            output.push("PASS:".to_string());
            for p in passes {
                output.push(format!("  [PASS] {p}"));
            }
        }

        output.join("\n")
    }
}

impl CliReport for WorkspaceDoctorReport {
    fn kind(&self) -> &'static str {
        "workspace.doctor"
    }
    fn render_text(&self) -> String {
        let mut lines = vec![
            format!("workspace_root={}", self.workspace_root.display()),
            format!("config_valid={}", self.config_valid),
        ];
        if let Some(ref err) = self.config_error {
            lines.push(format!("config_error={err}"));
        }
        lines.push(format!("policies_valid={}", self.policies_valid));
        if let Some(ref err) = self.policies_error {
            lines.push(format!("policies_error={err}"));
        }
        if !self.missing_files.is_empty() {
            lines.push(format!("missing_files={}", self.missing_files.join(", ")));
        }
        if let Some(ref model) = self.selected_model {
            lines.push(format!("selected_model={model}"));
        }
        lines.push(format!("model_valid={}", self.model_valid));
        if let Some(ref err) = self.model_error {
            lines.push(format!("model_error={err}"));
        }

        let mut auths: Vec<_> = self.auth_summary.iter().collect();
        auths.sort_by_key(|(k, _)| *k);
        for (provider, status) in auths {
            lines.push(format!("auth.{provider}={status}"));
        }
        lines.push(format!("run_dir_exists={}", self.run_dir_exists));
        let writable_str = match self.run_dir_writable {
            Some(true) => "true",
            Some(false) => "false",
            None => "unknown",
        };
        lines.push(format!("run_dir_writable={}", writable_str));
        if let Some(writable) = self.memory_writable {
            lines.push(format!("memory_writable={}", writable));
        }
        if let Some(ref err) = self.memory_write_error {
            lines.push(format!("memory_write_error={}", err));
        }
        lines.join("\n")
    }
}

/// An entry in the run log index listing.
#[derive(Serialize, Clone)]
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
#[derive(Serialize)]
pub struct RunsListReport {
    /// List of indexed runs.
    pub runs: Vec<RunIndexEntry>,
}

impl CliReport for RunsListReport {
    fn kind(&self) -> &'static str {
        "runs.list"
    }
    fn render_text(&self) -> String {
        if self.runs.is_empty() {
            return "No runs found.".to_string();
        }
        let mut lines = Vec::new();
        lines.push(format!(
            "{:<30} | {:<20} | {:<30} | {:<12} | {:<15} | {:<10}",
            "RUN ID", "START TIME", "PROVIDER/MODEL", "STATUS", "TOKENS (IN/OUT)", "COST"
        ));
        lines.push("-".repeat(129));
        for r in &self.runs {
            let start_time_str = r
                .start_time
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let prov_mod = match (&r.provider, &r.model) {
                (Some(p), Some(m)) => format!("{p}/{m}"),
                (Some(p), None) => p.to_string(),
                (None, Some(m)) => m.to_string(),
                _ => "unknown".to_string(),
            };
            let tokens_str = match (r.total_input_tokens, r.total_output_tokens) {
                (Some(i), Some(o)) => format!("{i}/{o}"),
                _ => "unknown".to_string(),
            };
            let cost_str = r
                .estimated_cost_usd
                .map(|c| format!("${c:.6}"))
                .unwrap_or_else(|| "unknown".to_string());
            lines.push(format!(
                "{:<30} | {:<20} | {:<30} | {:<12} | {:<15} | {:<10}",
                r.run_id, start_time_str, prov_mod, r.apparent_status, tokens_str, cost_str
            ));
        }
        lines.join("\n")
    }
}

/// Detailed run inspection report.
#[derive(Serialize)]
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

impl CliReport for RunsInspectReport {
    fn kind(&self) -> &'static str {
        "runs.inspect"
    }
    fn render_text(&self) -> String {
        let start_time_str = self
            .start_time
            .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let prov_mod = match (&self.provider, &self.model) {
            (Some(p), Some(m)) => format!("{p}/{m}"),
            (Some(p), None) => p.to_string(),
            (None, Some(m)) => m.to_string(),
            _ => "unknown".to_string(),
        };
        let tokens_str = match (self.total_input_tokens, self.total_output_tokens) {
            (Some(i), Some(o)) => format!("{i} in / {o} out"),
            _ => "unknown".to_string(),
        };
        let cost_str = self
            .estimated_cost_usd
            .map(|c| format!("${c:.6}"))
            .unwrap_or_else(|| "unknown".to_string());

        let mut lines = vec![
            format!("Run ID: {}", self.run_id),
            format!("Path: {}", self.path.display()),
            format!("Start Time: {start_time_str}"),
            format!("Session ID: {}", self.session_id),
            format!(
                "Parent Run ID: {}",
                self.parent_run_id.as_deref().unwrap_or("none")
            ),
            format!("Run Kind: {}", self.run_kind.as_deref().unwrap_or("none")),
            format!(
                "Lifecycle State: {}",
                self.lifecycle_state.as_deref().unwrap_or("none")
            ),
            format!("Status: {}", self.apparent_status),
            format!("Provider/Model: {prov_mod}"),
            format!(
                "Turns: {}",
                self.turns
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            ),
            format!(
                "Stop Reason: {}",
                self.stop_reason.as_deref().unwrap_or("unknown")
            ),
            format!("Tokens: {tokens_str}"),
            format!("Cost: {cost_str}"),
            format!(
                "Workspace Snapshot ID: {}",
                self.workspace_snapshot_id.as_deref().unwrap_or("none")
            ),
            format!("Artifacts: {} artifacts", self.artifacts.len()),
        ];
        for a in &self.artifacts {
            lines.push(format!("  - {a}"));
        }
        lines.join("\n")
    }
}

/// Report containing metrics of pruned runs.
#[derive(Serialize)]
pub struct RunsPruneReport {
    /// List of pruned run identifiers.
    pub pruned_runs: Vec<String>,
    /// Reclaimed disk space in bytes.
    pub reclaimed_bytes: u64,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

impl CliReport for RunsPruneReport {
    fn kind(&self) -> &'static str {
        "runs.prune"
    }
    fn render_text(&self) -> String {
        let prefix = if self.dry_run {
            "Would prune"
        } else {
            "Pruned"
        };
        if self.pruned_runs.is_empty() {
            return "No runs found to prune.".to_string();
        }
        let size_mb = self.reclaimed_bytes as f64 / 1_048_576.0;
        let mut lines = vec![format!(
            "{prefix} {} runs (reclaiming {size_mb:.2} MB):",
            self.pruned_runs.len()
        )];
        for r in &self.pruned_runs {
            lines.push(format!("  - {r}"));
        }
        lines.join("\n")
    }
}

/// Report containing metrics of a deleted run.
#[derive(Serialize)]
pub struct RunsDeleteReport {
    /// Deleted run identifier.
    pub deleted_run: String,
    /// Reclaimed disk space in bytes.
    pub reclaimed_bytes: u64,
}

impl CliReport for RunsDeleteReport {
    fn kind(&self) -> &'static str {
        "runs.delete"
    }
    fn render_text(&self) -> String {
        let size_mb = self.reclaimed_bytes as f64 / 1_048_576.0;
        format!(
            "Deleted run {} (reclaimed {size_mb:.2} MB).",
            self.deleted_run
        )
    }
}

/// Export format for run trace files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Markdown,
    Jsonl,
    Sharegpt,
}

/// Detailed trace inspection report.
#[derive(Debug, Clone, Serialize)]
pub struct TraceInspectReport {
    pub run_id: String,
    pub path: PathBuf,
    pub total_events: usize,
    pub event_types: std::collections::HashMap<String, usize>,
    pub turns: usize,
    pub tool_calls: usize,
    pub policy_decisions: usize,
    pub policy_outcomes: PolicyOutcomesSummary,
    pub verification_results: usize,
    pub verification_status: Option<gestalt_core::event::VerificationStatus>,
    pub artifacts: Vec<String>,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub estimated_cost_usd: Option<f64>,
    pub prompt_snapshots_created: usize,
    pub prompt_snapshots_loaded: usize,
    pub prompt_snapshots_reused: usize,
    pub prompt_cache_plans: usize,
    pub ephemeral_context_injections: usize,
    pub redacted: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct PolicyOutcomesSummary {
    pub allowed: usize,
    pub confirmed: usize,
    pub denied: usize,
}

impl CliReport for TraceInspectReport {
    fn kind(&self) -> &'static str {
        "trace.inspect"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!("Run ID: {}", self.run_id),
            format!("Path: {}", self.path.display()),
            format!("Total Events: {}", self.total_events),
            format!("Turns: {}", self.turns),
            format!("Tool Calls: {}", self.tool_calls),
            format!("Policy Decisions: {}", self.policy_decisions),
            format!("  Allowed: {}", self.policy_outcomes.allowed),
            format!("  Confirmed: {}", self.policy_outcomes.confirmed),
            format!("  Denied: {}", self.policy_outcomes.denied),
            format!("Verification Results: {}", self.verification_results),
            format!(
                "  Status: {}",
                self.verification_status
                    .map(|s| format!("{s:?}"))
                    .unwrap_or_else(|| "none".to_string())
            ),
            format!(
                "Tokens: {} in / {} out",
                self.total_input_tokens, self.total_output_tokens
            ),
            format!(
                "Cost: {}",
                self.estimated_cost_usd
                    .map(|c| format!("${c:.6}"))
                    .unwrap_or_else(|| "unknown".to_string())
            ),
            format!(
                "Snapshot/Cache: created={} loaded={} reused={} cache_plans={} ephemeral_injections={}",
                self.prompt_snapshots_created,
                self.prompt_snapshots_loaded,
                self.prompt_snapshots_reused,
                self.prompt_cache_plans,
                self.ephemeral_context_injections
            ),
            format!("Redacted: {}", self.redacted),
            format!("Artifacts: {} artifacts", self.artifacts.len()),
        ];
        for a in &self.artifacts {
            lines.push(format!("  - {a}"));
        }
        if !self.warnings.is_empty() {
            lines.push("Warnings:".to_string());
            for w in &self.warnings {
                lines.push(format!("  - {w}"));
            }
        }
        lines.join("\n")
    }
}

/// Trace file validation report.
#[derive(Debug, Clone, Serialize)]
pub struct TraceValidateReport {
    pub run_id: String,
    pub path: PathBuf,
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl CliReport for TraceValidateReport {
    fn kind(&self) -> &'static str {
        "trace.validate"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!("Run ID: {}", self.run_id),
            format!("Path: {}", self.path.display()),
            format!("Valid: {}", self.valid),
        ];
        if !self.errors.is_empty() {
            lines.push("Errors:".to_string());
            for e in &self.errors {
                lines.push(format!("  - {e}"));
            }
        }
        if !self.warnings.is_empty() {
            lines.push("Warnings:".to_string());
            for w in &self.warnings {
                lines.push(format!("  - {w}"));
            }
        }
        lines.join("\n")
    }
}

/// Trace analysis report for tool-calling metrics.
#[derive(Debug, Clone, Serialize)]
pub struct TraceAnalyzeReport {
    pub path: PathBuf,
    pub tools_metrics: gestalt_trace::ToolMetricsReport,
}

impl CliReport for TraceAnalyzeReport {
    fn kind(&self) -> &'static str {
        "trace.analyze"
    }

    fn render_text(&self) -> String {
        let m = &self.tools_metrics;
        let cost_str = match m.estimated_cost_usd {
            Some(c) => format!("${:.6}", c),
            None => "N/A (pricing missing)".to_string(),
        };
        let lines: Vec<String> = vec![
            format!("Analysis Path: {}", self.path.display()),
            "Tool Metrics Summary:".to_string(),
            format!(
                "  Total Proposed Calls:          {}",
                m.total_proposed_calls
            ),
            format!(
                "  Total Validation Failures:     {}",
                m.total_validation_failures
            ),
            format!(
                "  Invalid Tool Call Rate:        {:.2}%",
                m.invalid_tool_call_rate * 100.0
            ),
            format!(
                "  Total Policy Decisions:        {}",
                m.total_policy_decisions
            ),
            format!(
                "  Total Policy Denials:          {}",
                m.total_policy_denials
            ),
            format!(
                "  Policy Denied Rate:            {:.2}%",
                m.policy_denied_rate * 100.0
            ),
            format!("  Total Tool Results:            {}", m.total_tool_results),
            format!(
                "  Total Truncated Results:       {}",
                m.total_truncated_results
            ),
            format!(
                "  Truncation Rate:               {:.2}%",
                m.truncation_rate * 100.0
            ),
            format!(
                "  Total Executed Calls:          {}",
                m.total_executed_calls
            ),
            format!(
                "  First-call Success Count:      {}",
                m.first_call_success_count
            ),
            format!(
                "  First-call Success Rate:       {:.2}%",
                m.first_call_success_rate * 100.0
            ),
            format!("  Total Input Tokens:            {}", m.total_input_tokens),
            format!("  Total Output Tokens:           {}", m.total_output_tokens),
            format!("  Estimated Cost:                {}", cost_str),
            format!(
                "  Total Turns with Tool Catalog:  {}",
                m.total_turns_with_tool_selection
            ),
            format!(
                "  Exposed Tool Catalog Size/Turn: {:.2}",
                m.tool_exposure_count_per_turn
            ),
        ];
        lines.join("\n")
    }
}

/// Export format wrapper for printing export outputs.
#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub format: String,
    pub content: String,
}

impl CliReport for ExportReport {
    fn kind(&self) -> &'static str {
        "export"
    }

    fn render_text(&self) -> String {
        self.content.clone()
    }
}

/// Post-run verification execution result entry.
#[derive(Debug, Clone, Serialize)]
pub struct VerifierResultEntry {
    pub name: String,
    pub status: gestalt_core::event::VerificationStatus,
    pub findings: Vec<gestalt_core::event::VerificationFinding>,
    pub report: Option<String>,
}

/// Artifact verification summary.
#[derive(Debug, Clone, Serialize)]
pub struct ArtifactVerificationResult {
    pub artifact_path: String,
    pub verifiers: Vec<VerifierResultEntry>,
}

/// Aggregated verify run report.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyRunReport {
    pub run_id: String,
    pub status: gestalt_core::event::VerificationStatus,
    pub total_checks: usize,
    pub total_failed: usize,
    pub artifacts: Vec<ArtifactVerificationResult>,
}

impl CliReport for VerifyRunReport {
    fn kind(&self) -> &'static str {
        "verify.run"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!("Run ID: {}", self.run_id),
            format!("Overall Status: {:?}", self.status),
            format!("Total Checks: {}", self.total_checks),
            format!("Total Failed: {}", self.total_failed),
            "Artifacts:".to_string(),
        ];
        for a in &self.artifacts {
            lines.push(format!("  - Artifact: {}", a.artifact_path));
            for v in &a.verifiers {
                lines.push(format!("    * Verifier: {} [{:?}]", v.name, v.status));
                for f in &v.findings {
                    lines.push(format!(
                        "      - [{:?}] {}{}",
                        f.severity,
                        f.message,
                        f.location
                            .as_ref()
                            .map(|loc| format!(" at {loc}"))
                            .unwrap_or_default()
                    ));
                }
                if let Some(r) = &v.report {
                    lines.push(format!("      Report: {r}"));
                }
            }
        }
        lines.join("\n")
    }
}

fn redact_effective_config(mut config: crate::config::EffectiveConfig) -> crate::config::EffectiveConfig {
    for prov in config.providers.values_mut() {
        if let Some(ref mut headers) = prov.headers {
            for (k, v) in headers.iter_mut() {
                let lower_k = k.to_lowercase();
                if lower_k.contains("auth")
                    || lower_k.contains("key")
                    || lower_k.contains("token")
                    || lower_k.contains("secret")
                    || lower_k.contains("credential")
                    || lower_k.contains("sig")
                {
                    *v = "[REDACTED]".to_string();
                }
            }
        }
        if prov.auth_ref.is_some() {
            prov.auth_ref = Some("[REDACTED]".to_string());
        }
    }
    config
}

fn redact_explain_map(
    mut map: std::collections::HashMap<String, crate::config::ConfigSourceInfo>
) -> std::collections::HashMap<String, crate::config::ConfigSourceInfo> {
    for (k, info) in &mut map {
        let lower_k = k.to_lowercase();
        if (lower_k.contains("auth_ref") || lower_k.contains("api_key") || lower_k.contains("headers"))
            && !lower_k.contains("api_key_env") {
                match &mut info.value {
                    Value::String(s) => {
                        *s = "[REDACTED]".to_string();
                    }
                    other => {
                        *other = Value::String("[REDACTED]".to_string());
                    }
                }
            }
    }
    map
}

#[derive(Serialize)]
pub struct ConfigShowReport {
    pub config: crate::config::EffectiveConfig,
    pub source: bool,
    pub explain_map: Option<std::collections::HashMap<String, crate::config::ConfigSourceInfo>>,
}

impl CliReport for ConfigShowReport {
    fn kind(&self) -> &'static str {
        "config.show"
    }

    fn render_text(&self) -> String {
        if self.source {
            if let Some(ref map) = self.explain_map {
                let redacted_map = redact_explain_map(map.clone());
                let mut lines = vec![
                    format!(
                        "{:<35} | {:<25} | {:<25}",
                        "Configuration Key", "Value", "Source"
                    ),
                    "-".repeat(91),
                ];
                let mut keys: Vec<&String> = redacted_map.keys().collect();
                keys.sort();
                for k in keys {
                    if let Some(info) = redacted_map.get(k) {
                        let val_str = match &info.value {
                            Value::Null => "null".to_string(),
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        lines.push(format!("{:<35} | {:<25} | {:<25}", k, val_str, info.source));
                    }
                }
                lines.join("\n")
            } else {
                "No source information available".to_string()
            }
        } else {
            let redacted_config = redact_effective_config(self.config.clone());
            toml::to_string(&redacted_config).unwrap_or_else(|_| "Serialization error".to_string())
        }
    }
}

#[derive(Serialize)]
pub struct ConfigExplainReport {
    pub explain_map: std::collections::HashMap<String, crate::config::ConfigSourceInfo>,
}

impl CliReport for ConfigExplainReport {
    fn kind(&self) -> &'static str {
        "config.explain"
    }

    fn render_text(&self) -> String {
        let redacted_map = redact_explain_map(self.explain_map.clone());
        let mut lines = vec![
            "Precedence Order: CLI Override > Env Var > Workspace Config File > Global Config File > Default".to_string(),
            String::new(),
        ];
        let mut keys: Vec<&String> = redacted_map.keys().collect();
        keys.sort();
        for k in keys {
            if let Some(info) = redacted_map.get(k) {
                let val_str = match &info.value {
                    Value::Null => "null".to_string(),
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                lines.push(format!("{} = {} (Active: {})", k, val_str, info.source));
            }
        }
        lines.join("\n")
    }
}
#[derive(Serialize)]
pub struct ConfigPathsReport {
    pub global_path: std::path::PathBuf,
    pub global_exists: bool,
    pub legacy_global_path: std::path::PathBuf,
    pub legacy_global_exists: bool,
    pub workspace_path: std::path::PathBuf,
    pub workspace_exists: bool,
    pub legacy_workspace_path: std::path::PathBuf,
    pub legacy_workspace_exists: bool,
    pub legacy_policies_path: std::path::PathBuf,
    pub legacy_policies_exists: bool,
    pub ambiguities: Vec<String>,
}

impl CliReport for ConfigPathsReport {
    fn kind(&self) -> &'static str {
        "config.paths"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            "Config Paths and Discovery:".to_string(),
            String::new(),
            format!("  Global JSON Config:        {} (exists: {})", self.global_path.display(), self.global_exists),
            format!("  Global Legacy TOML:        {} (exists: {}) [DEPRECATED]", self.legacy_global_path.display(), self.legacy_global_exists),
            format!("  Workspace JSON Config:     {} (exists: {})", self.workspace_path.display(), self.workspace_exists),
            format!("  Workspace Legacy TOML:     {} (exists: {}) [DEPRECATED]", self.legacy_workspace_path.display(), self.legacy_workspace_exists),
            format!("  Workspace Legacy Policies: {} (exists: {}) [DEPRECATED]", self.legacy_policies_path.display(), self.legacy_policies_exists),
            String::new(),
        ];
        if self.ambiguities.is_empty() {
            lines.push("  No ambiguity issues detected.".to_string());
        } else {
            lines.push("  Ambiguity Warnings:".to_string());
            for amb in &self.ambiguities {
                lines.push(format!("    - [WARNING] {amb}"));
            }
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct PolicyValidateReport {
    pub path: PathBuf,
    pub valid: bool,
    pub error: Option<String>,
}

impl CliReport for PolicyValidateReport {
    fn kind(&self) -> &'static str {
        "policy.validate"
    }

    fn render_text(&self) -> String {
        if self.valid {
            format!("valid policy path={}", self.path.display())
        } else {
            format!(
                "invalid policy path={} error={}",
                self.path.display(),
                self.error.as_deref().unwrap_or("unknown error")
            )
        }
    }
}

#[derive(Serialize)]
pub struct PolicyExplainReport {
    pub tool: String,
    pub input: Value,
    pub mode: String,
    pub risk: gestalt_core::tool::RiskLevel,
    pub decision: gestalt_core::policy::PolicyDecision,
}

impl CliReport for PolicyExplainReport {
    fn kind(&self) -> &'static str {
        "policy.explain"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!(
                "Policy Explain for tool '{}' with input {}",
                self.tool, self.input
            ),
            format!("Execution Mode: {}", self.mode),
            format!("Classified Risk: {:?}", self.risk),
            format!("Outcome: {:?}", self.decision.status),
        ];
        if !self.decision.policy_source.is_empty() {
            lines.push(format!("Policy Source: {}", self.decision.policy_source));
        }
        if let Some(ref reason) = self.decision.reason {
            lines.push(format!("Reason: {}", reason));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct PolicyTestReport {
    pub tool: String,
    pub input: Value,
    pub mode: String,
    pub risk: gestalt_core::tool::RiskLevel,
    pub decision: gestalt_core::policy::PolicyDecision,
}

impl CliReport for PolicyTestReport {
    fn kind(&self) -> &'static str {
        "policy.test"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!(
                "Policy Test for tool '{}' with input {}",
                self.tool, self.input
            ),
            format!("Execution Mode: {}", self.mode),
            format!("Classified Risk: {:?}", self.risk),
            format!("Outcome: {:?}", self.decision.status),
        ];
        if !self.decision.policy_source.is_empty() {
            lines.push(format!("Policy Source: {}", self.decision.policy_source));
        }
        if let Some(ref reason) = self.decision.reason {
            lines.push(format!("Reason: {}", reason));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
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

impl CliReport for ContextExplainReport {
    fn kind(&self) -> &'static str {
        "context.explain"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!("Pipeline Version: {}", self.pipeline_version),
            format!("Token Estimate:   {}", self.token_estimate),
            format!("Packet Hash:      {}", self.packet_hash),
        ];
        if self.prompt.is_some() {
            lines.push(format!(
                "Prompt Source:    {}",
                self.prompt_source.as_deref().unwrap_or("none")
            ));
        } else if let Some(ref run_id) = self.run_id {
            lines.push(format!("Run ID:           {}", run_id));
        }

        lines.push(String::new());
        lines.push("Context Sources:".to_string());
        lines.push(format!(
            "{:<15} | {:<30} | {:<10} | {:<15} | {:<8}",
            "Kind", "Path/Label", "Trust", "Token Est.", "Included"
        ));
        lines.push("-".repeat(91));
        for s in &self.sources {
            lines.push(format!(
                "{:<15} | {:<30} | {:<10} | {:<15} | {:<8}",
                s.kind, s.path_or_label, s.trust, s.token_estimate, s.included
            ));
        }

        if !self.omissions.is_empty() {
            lines.push(String::new());
            lines.push("Context Omissions (Budget Exhausted):".to_string());
            lines.push(format!(
                "{:<15} | {:<30} | {:<10} | {:<15} | {:<20}",
                "Kind", "Path/Label", "Trust", "Token Est.", "Reason"
            ));
            lines.push("-".repeat(98));
            for o in &self.omissions {
                lines.push(format!(
                    "{:<15} | {:<30} | {:<10} | {:<15} | {:<20}",
                    o.kind, o.path_or_label, o.trust, o.token_estimate, o.reason
                ));
            }
        }

        if let Some(ref sys) = self.system_prompt {
            lines.push(String::new());
            lines.push("System Prompt:".to_string());
            lines.push("-".repeat(91));
            lines.push(sys.clone());
            lines.push("-".repeat(91));
        }

        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct ToolInfoEntry {
    pub name: String,
    pub description: String,
    pub risk_type: String,
}

#[derive(Serialize)]
pub struct ToolsListReport {
    pub tools: Vec<ToolInfoEntry>,
}

impl CliReport for ToolsListReport {
    fn kind(&self) -> &'static str {
        "tools.list"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!(
                "{:<15} | {:<20} | {:<50}",
                "Tool Name", "Risk Classification", "Description"
            ),
            "-".repeat(91),
        ];
        for t in &self.tools {
            lines.push(format!(
                "{:<15} | {:<20} | {:<50}",
                t.name, t.risk_type, t.description
            ));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct ToolsInspectReport {
    pub name: String,
    pub schema: Value,
    pub risk: gestalt_core::tool::RiskLevel,
    pub annotations: gestalt_core::tool_descriptor::ToolAnnotations,
}

impl CliReport for ToolsInspectReport {
    fn kind(&self) -> &'static str {
        "tools.inspect"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!("Tool Name:   {}", self.name),
            format!("Risk Level:  {:?}", self.risk),
        ];
        if !self.annotations.annotations.is_empty() {
            lines.push("Annotations:".to_string());
            for ann in &self.annotations.annotations {
                lines.push(format!("  - {}: {} ({:?})", ann.key, ann.value, ann.source));
            }
        }
        lines.push("Schema:".to_string());
        if let Ok(schema_str) = serde_json::to_string_pretty(&self.schema) {
            for line in schema_str.lines() {
                lines.push(format!("  {}", line));
            }
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct ToolsClassifyReport {
    pub command: String,
    pub risk: gestalt_core::tool::RiskLevel,
}

impl CliReport for ToolsClassifyReport {
    fn kind(&self) -> &'static str {
        "tools.classify"
    }

    fn render_text(&self) -> String {
        let lines = vec![
            format!("Command:  {}", self.command),
            format!("Risk:     {:?}", self.risk),
        ];
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct ConnectReport {
    pub provider: String,
    pub status: String,
    pub profile_created: Option<String>,
    pub keychain_stored: bool,
}

impl CliReport for ConnectReport {
    fn kind(&self) -> &'static str {
        "connect"
    }
    fn render_text(&self) -> String {
        format!(
            "Connected to provider '{}'. status={}\nprofile_created={}\nkeychain_stored={}",
            self.provider,
            self.status,
            self.profile_created.as_deref().unwrap_or("none"),
            self.keychain_stored
        )
    }
}

#[derive(Serialize)]
pub struct ProfileInfoEntry {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub active: bool,
}

#[derive(Serialize)]
pub struct ProfilesListReport {
    pub profiles: Vec<ProfileInfoEntry>,
}

impl CliReport for ProfilesListReport {
    fn kind(&self) -> &'static str {
        "profiles.list"
    }
    fn render_text(&self) -> String {
        let mut lines = vec![
            format!(
                "{:<15} | {:<20} | {:<30} | {:<8}",
                "Profile", "Provider", "Model", "Active"
            ),
            "-".repeat(81),
        ];
        for p in &self.profiles {
            let active_str = if p.active { "yes" } else { "no" };
            lines.push(format!(
                "{:<15} | {:<20} | {:<30} | {:<8}",
                p.name, p.provider, p.model, active_str
            ));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
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

impl CliReport for ProfilesInspectReport {
    fn kind(&self) -> &'static str {
        "profiles.inspect"
    }
    fn render_text(&self) -> String {
        let active_str = if self.active { "yes" } else { "no" };
        let mut lines = vec![
            format!("Profile:      {}", self.name),
            format!("Provider:     {}", self.provider),
            format!("Model:        {}", self.model),
            format!("Active:       {}", active_str),
            format!("Adapter Kind: {}", self.resolved_provider_kind),
        ];
        if let Some(ref url) = self.resolved_base_url {
            lines.push(format!("Base URL:     {}", url));
        }
        if let Some(ref auth) = self.resolved_auth_ref {
            lines.push(format!("Auth Ref:     {}", auth));
        }
        if let Some(ref env) = self.resolved_api_key_env {
            lines.push(format!("API Key Env:  {}", env));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct ProfilesUseReport {
    pub name: String,
    pub active: bool,
    pub file_updated: PathBuf,
}

impl CliReport for ProfilesUseReport {
    fn kind(&self) -> &'static str {
        "profiles.use"
    }
    fn render_text(&self) -> String {
        format!(
            "Profile '{}' is now active. Updated config at {}",
            self.name,
            self.file_updated.display()
        )
    }
}

#[derive(Serialize)]
pub struct DisconnectReport {
    pub provider: String,
    pub profile_removed: Option<String>,
    pub keychain_cleared: bool,
}

impl CliReport for DisconnectReport {
    fn kind(&self) -> &'static str {
        "disconnect"
    }
    fn render_text(&self) -> String {
        format!(
            "Disconnected provider '{}'. profile_removed={}\nkeychain_cleared={}",
            self.provider,
            self.profile_removed.as_deref().unwrap_or("none"),
            self.keychain_cleared
        )
    }
}

#[derive(Serialize)]
pub struct ModelsSearchReport {
    pub models: Vec<gestalt_core::model::ModelInfo>,
}

impl CliReport for ModelsSearchReport {
    fn kind(&self) -> &'static str {
        "models.search"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!(
                "{:<30} | {:<25} | {:<10} | {:<10} | {:<10} | {:<10}",
                "Model ID", "Display Name", "Context", "Max Out", "Tools", "Vision"
            ),
            "-".repeat(110),
        ];
        for m in &self.models {
            lines.push(format!(
                "{:<30} | {:<25} | {:<10} | {:<10} | {:<10} | {:<10}",
                m.qualified_id,
                m.display_name,
                m.max_context_tokens,
                m.max_output_tokens,
                if m.supports_tools { "yes" } else { "no" },
                if m.supports_vision { "yes" } else { "no" }
            ));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct RuntimeInspectReport {
    pub inspect: gestalt_runtime::RuntimeInspect,
}

impl CliReport for RuntimeInspectReport {
    fn kind(&self) -> &'static str {
        "runtime.inspect"
    }

    fn render_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Resolved Gestalt Agent Runtime Shape".to_string());
        lines.push("====================================".to_string());
        lines.push(format!(
            "Provider Connection: {}",
            self.inspect.provider_name
        ));
        lines.push(format!(
            "Provider Model:      {}",
            self.inspect.provider_model
        ));
        lines.push(format!(
            "Execution Mode:      {}",
            self.inspect.execution_mode
        ));
        lines.push(format!("Max Turns Limit:     {}", self.inspect.max_turns));
        lines.push(format!(
            "Context Version:     {}",
            self.inspect.context_pipeline_version
        ));
        lines.push(format!(
            "Workspace Root:      {}",
            self.inspect.workspace_root
        ));

        if let Some(ref source) = self.inspect.policy_source_path {
            lines.push(format!("Policy Source:       {}", source));
        }
        if let Some(ref fp) = self.inspect.policy_fingerprint {
            lines.push(format!("Policy Fingerprint:  {}", fp));
        }

        if let Some(ref sink) = self.inspect.trace_sink_kind {
            lines.push(format!("Trace Sink Kind:     {}", sink));
        }

        if let Some(ref fp) = self.inspect.effective_config_fingerprint {
            lines.push(format!("Effective Config FP: {}", fp));
        }
        if let Some(ref fp) = self.inspect.variant_fingerprint {
            lines.push(format!("Model Variant FP:    {}", fp));
        }
        if let Some(ref fp) = self.inspect.negotiated_protocol_fingerprint {
            lines.push(format!("Negotiated Proto FP: {}", fp));
        }

        lines.push(String::new());
        lines.push(format!(
            "Enabled CLI Features: {:?}",
            self.inspect.enabled_cli_features
        ));

        lines.push(String::new());
        lines.push(format!(
            "Extensions Installed ({}):",
            self.inspect.extensions.len()
        ));
        if self.inspect.extensions.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            for ext in &self.inspect.extensions {
                lines.push(format!("  - {}", ext));
            }
        }

        lines.push(String::new());
        lines.push(format!(
            "Active Verification Rules ({}):",
            self.inspect.verifiers.len()
        ));
        if self.inspect.verifiers.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            for v in &self.inspect.verifiers {
                lines.push(format!("  - {}", v));
            }
        }

        lines.push(String::new());
        lines.push(format!(
            "Registered Composition Hooks ({}):",
            self.inspect.hooks.len()
        ));
        lines.push(format!(
            "Hook Contract Hash:  {}",
            self.inspect.hook_contract_hash
        ));
        if self.inspect.hooks.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            for h in &self.inspect.hooks {
                lines.push(format!("  - {}", h));
            }
        }

        lines.push(String::new());
        lines.push(format!(
            "Available Tools ({}) - Schema Hash {}:",
            self.inspect.tools.len(),
            self.inspect.tool_schema_hash
        ));
        if self.inspect.tools.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            let mut sorted_tools = self.inspect.tools.clone();
            sorted_tools.sort_by(|a, b| a.name.cmp(&b.name));
            for t in &sorted_tools {
                lines.push(format!("  - {:<25} (hash: {})", t.name, t.schema_hash));
            }
        }

        lines.push(String::new());
        lines.push(format!(
            "MCP Servers ({}) - Discovery Threshold: {:?}",
            self.inspect.mcp_servers.len(),
            self.inspect.mcp_discovery_threshold
        ));
        if self.inspect.mcp_servers.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            for server in &self.inspect.mcp_servers {
                let status = format!("{:?}", server.connection_state).to_uppercase();
                let trust = server.trust_level.as_deref().unwrap_or("untrusted");
                let fresh = if server.cache_fresh { "fresh" } else { "stale" };
                let mode = if server.discovery_mode { "progressive discovery" } else { "direct exposure" };
                lines.push(format!(
                    "  - {}: Status: {} | Tools: {} ({}) | Trust: {} | Mode: {}",
                    server.server_id,
                    status,
                    server.tool_count,
                    fresh,
                    trust,
                    mode
                ));
                if let Some(ref err) = server.last_error {
                    lines.push(format!("    Error: {}", err));
                }
            }
        }

        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct ExtensionsListReport {
    pub extensions: Vec<gestalt_runtime::DiscoveredExtension>,
}

impl CliReport for ExtensionsListReport {
    fn kind(&self) -> &'static str {
        "extensions.list"
    }

    fn render_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Discovered Gestalt Extensions".to_string());
        lines.push("=============================".to_string());
        if self.extensions.is_empty() {
            lines.push("No extensions found.".to_string());
        } else {
            for ext in &self.extensions {
                let status = if ext.enabled { "ENABLED" } else { "DISABLED" };
                lines.push(format!(
                    "- {} (v{}) [{}] - {}",
                    ext.manifest.id,
                    ext.manifest.version,
                    status,
                    ext.manifest_path.to_string_lossy()
                ));
            }
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct ExtensionInspectReport {
    pub manifest: gestalt_runtime::ExtensionManifest,
}

impl CliReport for ExtensionInspectReport {
    fn kind(&self) -> &'static str {
        "extensions.inspect"
    }

    fn render_text(&self) -> String {
        serde_json::to_string_pretty(&self.manifest).unwrap_or_default()
    }
}

#[derive(Serialize)]
pub struct ExtensionActionReport {
    pub action: String,
    pub extension_id: String,
    pub success: bool,
    pub message: String,
}

impl CliReport for ExtensionActionReport {
    fn kind(&self) -> &'static str {
        "extensions.action"
    }

    fn render_text(&self) -> String {
        format!(
            "{}: {} (success: {})",
            self.action, self.message, self.success
        )
    }
}

#[derive(Serialize)]
pub struct RuntimeEventsReport {
    pub events: Vec<gestalt_runtime::RuntimeEvent>,
}

impl CliReport for RuntimeEventsReport {
    fn kind(&self) -> &'static str {
        "runtime.events"
    }

    fn render_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Runtime Events Log".to_string());
        lines.push("==================".to_string());
        for evt in &self.events {
            lines.push(format!("{:?}", evt));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct RuntimeDoctorReport {
    pub checks: Vec<String>,
}

impl CliReport for RuntimeDoctorReport {
    fn kind(&self) -> &'static str {
        "runtime.doctor"
    }

    fn render_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push("Runtime Diagnostics (Doctor)".to_string());
        lines.push("===========================".to_string());
        for check in &self.checks {
            lines.push(check.clone());
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct SkillsListReport {
    pub skills: Vec<SkillListEntry>,
}

#[derive(Serialize)]
pub struct SkillListEntry {
    pub name: String,
    pub description: String,
    pub trust_level: String,
    pub source: String,
    pub manifest_path: String,
}

impl CliReport for SkillsListReport {
    fn kind(&self) -> &'static str {
        "skills.list"
    }

    fn render_text(&self) -> String {
        if self.skills.is_empty() {
            return "No skills discovered.".to_string();
        }
        let mut lines = vec![
            format!(
                "{:<20} | {:<12} | {:<15} | {:<30}",
                "Name", "Trust", "Source", "Description"
            ),
            "-".repeat(85),
        ];
        for s in &self.skills {
            lines.push(format!(
                "{:<20} | {:<12} | {:<15} | {:<30}",
                s.name,
                s.trust_level,
                s.source,
                s.description.chars().take(30).collect::<String>()
            ));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct SkillInspectReport {
    pub name: String,
    pub description: String,
    pub skill_root: String,
    pub manifest_path: String,
    pub manifest_hash: String,
    pub trust_level: String,
    pub source: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Option<String>,
}

impl CliReport for SkillInspectReport {
    fn kind(&self) -> &'static str {
        "skills.inspect"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![
            format!("Name:          {}", self.name),
            format!("Description:   {}", self.description),
            format!("Trust Level:   {}", self.trust_level),
            format!("Source:        {}", self.source),
            format!("Skill Root:    {}", self.skill_root),
            format!("Manifest Path: {}", self.manifest_path),
            format!("Manifest Hash: {}", self.manifest_hash),
        ];
        if let Some(ref license) = self.license {
            lines.push(format!("License:       {}", license));
        }
        if let Some(ref compat) = self.compatibility {
            lines.push(format!("Compatibility: {}", compat));
        }
        if let Some(ref tools) = self.allowed_tools {
            lines.push(format!("Allowed Tools: {}", tools));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct SkillActionReport {
    pub action: String,
    pub skill_name: String,
    pub success: bool,
    pub message: String,
}

impl CliReport for SkillActionReport {
    fn kind(&self) -> &'static str {
        "skills.action"
    }

    fn render_text(&self) -> String {
        self.message.clone()
    }
}
