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
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
                    "provider={}\nprovider={} source=env variable={} status={}",
                    r.provider, r.provider, r.auth_variable, r.auth_status
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
        self.models
            .iter()
            .map(|m| m.qualified_id.clone())
            .collect::<Vec<_>>()
            .join("\n")
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
}

impl CliReport for ModelsRefreshReport {
    fn kind(&self) -> &'static str {
        "models.refresh"
    }
    fn render_text(&self) -> String {
        format!("built-in catalog available: {} models", self.count)
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
    pub policies_path: PathBuf,
    pub workspace_md_path: PathBuf,
    pub memory_md_path: PathBuf,
}

impl CliReport for WorkspaceInfoReport {
    fn kind(&self) -> &'static str {
        "workspace.info"
    }
    fn render_text(&self) -> String {
        format!(
            "workspace_root={}\nconfig_path={}\npolicies_path={}\nworkspace_md_path={}\nmemory_md_path={}",
            self.workspace_root.display(),
            self.config_path.display(),
            self.policies_path.display(),
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
    pub run_dir_writable: bool,
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

        let mut auths: Vec<_> = self.auth_summary.iter().collect();
        auths.sort_by_key(|(k, _)| *k);
        for (provider, status) in auths {
            lines.push(format!("auth.{provider}={status}"));
        }
        lines.push(format!("run_dir_writable={}", self.run_dir_writable));
        lines.join("\n")
    }
}
