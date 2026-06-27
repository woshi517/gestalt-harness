use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gestalt_core::{
    trace::TraceSink, AgentEvent, Message, Session, SessionConfig, TokenBudget, ToolCatalog,
    ToolContext, WorkspaceSnapshotter,
};
use gestalt_tools::default_registry;
use gestalt_trace::{
    aggregate_costs, read_prompt_snapshot,
    resume::ResumeAnalyzer,
    run_manifest::{CompatibilityFingerprint, LifecycleState, RunKind, RunManifest},
    write_cost_report, write_summary, JsonlTraceSink,
};

use crate::{
    config::EffectiveConfig,
    output::{render_event, CliReport},
    runs::{resolve_run_path, summarize_run_dir, RunSummary},
};

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub title: String,
    pub created_at: Option<DateTime<Utc>>,
    pub runs_count: usize,
    pub latest_run_id: String,
    pub latest_run_status: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub total_turns: usize,
    pub estimated_cost_usd: f64,
}

#[derive(Serialize)]
pub struct SessionsListReport {
    pub sessions: Vec<SessionSummary>,
}

impl CliReport for SessionsListReport {
    fn kind(&self) -> &'static str {
        "sessions.list"
    }

    fn render_text(&self) -> String {
        if self.sessions.is_empty() {
            return "No sessions found.".to_string();
        }
        let mut lines = Vec::new();
        lines.push(format!(
            "{:<45} | {:<20} | {:<10} | {:<45} | {:<12} | {:<6} | {:<10}",
            "SESSION ID",
            "CREATED AT",
            "RUNS COUNT",
            "LATEST RUN ID",
            "STATUS",
            "TURNS",
            "EST. COST"
        ));
        lines.push("-".repeat(161));
        for s in &self.sessions {
            let created_at_str = s
                .created_at
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            lines.push(format!(
                "{:<45} | {:<20} | {:<10} | {:<45} | {:<12} | {:<6} | ${:<9.6}",
                s.session_id,
                created_at_str,
                s.runs_count,
                s.latest_run_id,
                s.latest_run_status,
                s.total_turns,
                s.estimated_cost_usd
            ));
        }
        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct SessionInspectReport {
    pub session_id: String,
    pub runs: Vec<RunManifestSummary>,
}

#[derive(Serialize)]
pub struct RunManifestSummary {
    pub run_id: String,
    pub dir_name: String,
    pub parent_run_id: Option<String>,
    pub run_kind: String,
    pub created_at: DateTime<Utc>,
    pub lifecycle_state: String,
    pub turns: usize,
}

impl CliReport for SessionInspectReport {
    fn kind(&self) -> &'static str {
        "sessions.inspect"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![format!("Session ID: {}", self.session_id)];
        if self.runs.is_empty() {
            lines.push("No runs found in this session.".to_string());
            return lines.join("\n");
        }
        lines.push("\nRuns Lineage Graph:".to_string());

        // Simple tree layout: build adj list
        let mut root_runs = Vec::new();
        let mut children: HashMap<String, Vec<&RunManifestSummary>> = HashMap::new();
        for r in &self.runs {
            if let Some(ref parent) = r.parent_run_id {
                children.entry(parent.clone()).or_default().push(r);
            } else {
                root_runs.push(r);
            }
        }

        root_runs.sort_by_key(|r| r.created_at);

        fn print_tree(
            run: &RunManifestSummary,
            children: &HashMap<String, Vec<&RunManifestSummary>>,
            depth: usize,
            lines: &mut Vec<String>,
        ) {
            let indent = "  ".repeat(depth);
            let prefix = if depth == 0 { "● " } else { "└─ " };
            lines.push(format!(
                "{}{}{} [{}] (State: {}, Turns: {}) - {}",
                indent,
                prefix,
                run.run_id,
                run.run_kind,
                run.lifecycle_state,
                run.turns,
                run.created_at.format("%Y-%m-%d %H:%M:%S UTC")
            ));
            if let Some(child_list) = children.get(&run.run_id) {
                let mut sorted_children = child_list.clone();
                sorted_children.sort_by_key(|c| c.created_at);
                for child in sorted_children {
                    print_tree(child, children, depth + 1, lines);
                }
            }
        }

        for root in root_runs {
            print_tree(root, &children, 0, &mut lines);
        }

        lines.join("\n")
    }
}

#[derive(Serialize)]
pub struct SessionHistoryReport {
    pub session_id: String,
    pub timeline: Vec<TimelineItem>,
}

#[derive(Serialize)]
pub struct TimelineItem {
    pub run_id: String,
    pub timestamp: DateTime<Utc>,
    pub event_summary: String,
}

impl CliReport for SessionHistoryReport {
    fn kind(&self) -> &'static str {
        "sessions.history"
    }

    fn render_text(&self) -> String {
        let mut lines = vec![format!("History Timeline for Session: {}", self.session_id)];
        if self.timeline.is_empty() {
            lines.push("No history events found.".to_string());
            return lines.join("\n");
        }
        lines.push("-".repeat(80));
        for item in &self.timeline {
            lines.push(format!(
                "[{}] Run {}: {}",
                item.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
                item.run_id,
                item.event_summary
            ));
        }
        lines.join("\n")
    }
}

/// Lists all sessions by scanning all run.json manifests.
pub fn list_sessions(
    config: &EffectiveConfig,
) -> Result<SessionsListReport, gestalt_core::HarnessError> {
    let run_log_dir = config.run_log_dir();
    let mut sessions_map: HashMap<String, Vec<RunSummary>> = HashMap::new();

    if run_log_dir.exists() {
        if let Ok(entries) = fs::read_dir(run_log_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Ok(summary) = summarize_run_dir(&entry.path()) {
                        sessions_map
                            .entry(summary.session_id.clone())
                            .or_default()
                            .push(summary);
                    }
                }
            }
        }
    }

    let mut sessions = Vec::new();
    for (session_id, mut runs) in sessions_map {
        runs.sort_by_key(|r| r.start_time);

        let oldest = runs.first();
        let latest = runs.last().unwrap(); // Safe as vec is not empty

        let total_turns: usize = runs.iter().map(|r| r.turns.unwrap_or(0)).sum();
        let total_cost: f64 = runs
            .iter()
            .map(|r| r.estimated_cost_usd.unwrap_or(0.0))
            .sum();

        let title = if let Some(first_run) = oldest {
            if let Ok(run_dir) = resolve_run_path(config, &first_run.run_id) {
                let trace_path = run_dir.join("trace.jsonl");
                if trace_path.exists() {
                    if let Ok(envelopes) = gestalt_trace::read_trace(&trace_path) {
                        let mut first_msg = None;
                        for env in envelopes {
                            if let gestalt_core::event::AgentEvent::UserMessage { content } =
                                env.event
                            {
                                let trimmed = content.trim();
                                if !trimmed.is_empty() {
                                    let mut t = trimmed.chars().take(30).collect::<String>();
                                    if trimmed.chars().count() > 30 {
                                        t.push_str("...");
                                    }
                                    first_msg = Some(t);
                                    break;
                                }
                            }
                        }
                        first_msg
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        let title = title.unwrap_or_else(|| session_id.clone());

        sessions.push(SessionSummary {
            session_id,
            title,
            created_at: oldest.and_then(|r| r.start_time),
            runs_count: runs.len(),
            latest_run_id: latest.run_id.clone(),
            latest_run_status: latest.apparent_status.clone(),
            provider: latest.provider.clone(),
            model: latest.model.clone(),
            total_turns,
            estimated_cost_usd: total_cost,
        });
    }

    sessions.sort_by_key(|s| s.created_at);

    Ok(SessionsListReport { sessions })
}

/// Inspects a session lineage tree.
pub fn inspect_session(
    config: &EffectiveConfig,
    session_id: &str,
) -> Result<SessionInspectReport, gestalt_core::HarnessError> {
    let run_log_dir = config.run_log_dir();
    let mut runs = Vec::new();

    if run_log_dir.exists() {
        if let Ok(entries) = fs::read_dir(run_log_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let manifest_path = entry.path().join("run.json");
                    if manifest_path.exists() {
                        if let Ok(manifest) = RunManifest::load_from(&manifest_path) {
                            if manifest.session_id == session_id {
                                let summary = summarize_run_dir(&entry.path())?;
                                runs.push(RunManifestSummary {
                                    run_id: manifest.run_id.clone(),
                                    dir_name: entry.file_name().to_string_lossy().into_owned(),
                                    parent_run_id: manifest.parent_run_id.clone(),
                                    run_kind: format!("{:?}", manifest.run_kind).to_lowercase(),
                                    created_at: manifest.created_at,
                                    lifecycle_state: format!("{:?}", manifest.lifecycle_state)
                                        .to_lowercase(),
                                    turns: summary.turns.unwrap_or(0),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    if runs.is_empty() {
        return Err(gestalt_core::HarnessError::Config(
            gestalt_core::ConfigError::InvalidValue {
                field: "session_id".to_string(),
                reason: format!("No runs found for session ID: {}", session_id),
            },
        ));
    }

    runs.sort_by_key(|r| r.created_at);

    Ok(SessionInspectReport {
        session_id: session_id.to_string(),
        runs,
    })
}

/// Displays the logical chronological timeline of events across a session.
pub fn history_session(
    config: &EffectiveConfig,
    session_id: &str,
) -> Result<SessionHistoryReport, gestalt_core::HarnessError> {
    let run_log_dir = config.run_log_dir();
    let mut runs = Vec::new();

    if run_log_dir.exists() {
        if let Ok(entries) = fs::read_dir(run_log_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let manifest_path = entry.path().join("run.json");
                    if manifest_path.exists() {
                        if let Ok(manifest) = RunManifest::load_from(&manifest_path) {
                            if manifest.session_id == session_id {
                                runs.push((manifest.created_at, entry.path(), manifest.run_id));
                            }
                        }
                    }
                }
            }
        }
    }

    if runs.is_empty() {
        return Err(gestalt_core::HarnessError::Config(
            gestalt_core::ConfigError::InvalidValue {
                field: "session_id".to_string(),
                reason: format!("No runs found for session ID: {}", session_id),
            },
        ));
    }

    runs.sort_by_key(|r| r.0);

    let mut timeline = Vec::new();

    for (_, run_path, run_id) in runs {
        let trace_path = run_path.join("trace.jsonl");
        if trace_path.exists() {
            if let Ok(envelopes) = gestalt_trace::read_trace(&trace_path) {
                for env in envelopes {
                    let event_summary = match env.event {
                        AgentEvent::UserMessage { ref content } => {
                            format!("User: {}", content)
                        }
                        AgentEvent::AssistantMessageCommitted { ref message } => {
                            let text = match message {
                                Message::Assistant { content } => content
                                    .iter()
                                    .filter_map(|block| {
                                        if let gestalt_core::message::ContentBlock::Text { text } =
                                            block
                                        {
                                            Some(text.clone())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" "),
                                _ => String::new(),
                            };
                            if !text.is_empty() {
                                format!("Assistant: {}", text)
                            } else {
                                "Assistant: [actions]".to_string()
                            }
                        }
                        AgentEvent::ToolExecutionStarted { ref tool_name, .. } => {
                            format!("Tool Executed: {}", tool_name)
                        }
                        AgentEvent::ToolResult {
                            ref id, is_error, ..
                        } => {
                            format!(
                                "Tool Result for {}: {}",
                                id,
                                if is_error { "error" } else { "success" }
                            )
                        }
                        AgentEvent::Checkpoint { .. } => "Checkpoint Committed".to_string(),
                        AgentEvent::Interrupted { ref reason } => {
                            format!("Interrupted: {}", reason)
                        }
                        _ => continue,
                    };
                    timeline.push(TimelineItem {
                        run_id: run_id.clone(),
                        timestamp: env.ts,
                        event_summary,
                    });
                }
            }
        }
    }

    Ok(SessionHistoryReport {
        session_id: session_id.to_string(),
        timeline,
    })
}

/// Continued session runner structure.
pub struct ContinuedRunConfig {
    pub session_id: String,
    pub parent_run_id: String,
    pub base_checkpoint: Option<u64>,
    pub run_kind: RunKind,
    pub prompt: Option<String>, // if continue/branch
    pub history: Vec<Message>,
    pub token_budget: TokenBudget,
}

/// Resolves a logical session's latest run head.
fn resolve_session_head(
    config: &EffectiveConfig,
    session_id: &str,
) -> Result<PathBuf, gestalt_core::HarnessError> {
    let run_log_dir = config.run_log_dir();
    let mut matching_runs = Vec::new();

    if run_log_dir.exists() {
        if let Ok(entries) = fs::read_dir(run_log_dir) {
            for entry in entries.flatten() {
                let manifest_path = entry.path().join("run.json");
                if manifest_path.exists() {
                    if let Ok(manifest) = RunManifest::load_from(&manifest_path) {
                        if manifest.session_id == session_id {
                            matching_runs.push((manifest.created_at, entry.path()));
                        }
                    }
                }
            }
        }
    }

    if matching_runs.is_empty() {
        return Err(gestalt_core::HarnessError::Config(
            gestalt_core::ConfigError::InvalidValue {
                field: "session_id".to_string(),
                reason: format!("No runs found for session ID: {}", session_id),
            },
        ));
    }

    matching_runs.sort_by_key(|r| r.0);
    // Return latest run path
    Ok(matching_runs.last().unwrap().1.clone())
}

pub fn calculate_continuation_state(
    parent_model: Option<&gestalt_core::ResolvedModelSnapshot>,
    current_model: &gestalt_core::ResolvedModelSnapshot,
    parent_context_state: gestalt_core::ContextProjectionState,
    parent_budget: gestalt_core::TokenBudget,
    config: &crate::config::EffectiveConfig,
) -> (
    gestalt_core::ContextProjectionState,
    gestalt_core::TokenBudget,
    bool,
) {
    let model_changed = match parent_model {
        Some(pm) => {
            pm.selection != current_model.selection || pm.api_format != current_model.api_format
        }
        None => true,
    };

    let context_state = if model_changed {
        gestalt_core::ContextProjectionState::default()
    } else {
        parent_context_state
    };

    let mut token_budget = parent_budget;
    token_budget.model_limit = config
        .context
        .max_context_window
        .or(Some(current_model.max_context_tokens))
        .unwrap_or(120_000);
    token_budget.reserved_output = config
        .context
        .reserved_output_tokens
        .or(Some(current_model.max_output_tokens.min(8192)))
        .or(config.tools.max_output_tokens.map(|v| v.min(8192)))
        .unwrap_or(4096);

    (context_state, token_budget, model_changed)
}

/// Main entry point for executing subcommands continue, resume, branch.
pub async fn run_session_action(
    config: &EffectiveConfig,
    action: &str, // "continue", "resume", "branch"
    target: &str, // session_id for continue, run_id_or_path for resume/branch
    prompt: Option<String>,
    branch_checkpoint: Option<u64>,
    api_key: Option<String>,
    cancel_token: gestalt_core::cancel::CancelToken,
    approval_override: Option<Arc<dyn gestalt_core::ApprovalProvider>>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<gestalt_core::AgentEvent>>,
) -> Result<PathBuf, gestalt_core::HarnessError> {
    // 1. Resolve parent run path
    let parent_run_path = match action {
        "continue" => resolve_session_head(config, target)?,
        _ => resolve_run_path(config, target)?,
    };

    // 2. Perform preflight analysis
    let snapshotter = gestalt_core::snapshot::GitWorkspaceSnapshotter;
    let current_snapshot = snapshotter.capture(&config.workspace_root).await?;

    let tools = Arc::new(default_registry()?);
    let skill_explicit: Vec<std::path::PathBuf> = config
        .skills
        .explicit_paths
        .iter()
        .map(std::path::PathBuf::from)
        .collect();
    let skill_discovery = crate::runtime::build_skill_discovery(config);
    let discovered_skills = skill_discovery
        .discover_all(&skill_explicit)
        .unwrap_or_default();
    let workspace_cfg = config.context.workspace.clone().unwrap_or_default();
    let memory_cfg = config.context.memory.clone().unwrap_or_default();
    let event_bus = gestalt_runtime::event_bus::RuntimeEventBus::new();
    let workspace_context_snapshot_hash =
        match gestalt_runtime::workspace_context::load_and_snapshot_workspace_context(
            &config.workspace_root,
            None,
            &event_bus,
            &workspace_cfg,
            &memory_cfg,
        )
        .await
        {
            Ok((_, _, snapshot)) => Some(snapshot.compute_hash()),
            Err(_) => None,
        };

    let expected_fingerprint = CompatibilityFingerprint {
        context_pipeline_version: "pipeline-v1".to_string(),
        tool_schema_hash: gestalt_trace::run_manifest::compute_tool_schema_hash(&tools.schemas()),
        policy_fingerprint: serde_json::to_string(&config.policies)
            .map(|content| gestalt_trace::run_manifest::compute_policy_fingerprint(&content))
            .unwrap_or_default(),
        hook_contract_hash: {
            let hook_names = vec![
                "VerificationToolHook".to_string(),
                "EvaluatorHook".to_string(),
            ];
            gestalt_trace::run_manifest::compute_hook_contract_hash(&hook_names)
        },
        execution_mode: format!("{:?}", config.selected_mode()?),
        skill_fingerprint: crate::run::compute_skill_fingerprint(config, &discovered_skills, None),
        workspace_context_snapshot_hash,
    };

    let analysis = ResumeAnalyzer::analyze(
        &parent_run_path,
        Some(&current_snapshot),
        Some(&expected_fingerprint),
    );

    // For branch, we bypass default safety rules if a specific checkpoint seq is requested.
    // However, if we do a normal resume, we check if it is safe to resume.
    // If continue, we check if it is safe to continue.
    match action {
        "resume" => {
            if !analysis.is_safe_to_resume() {
                return Err(gestalt_core::HarnessError::Policy(gestalt_core::PolicyError::Denied(
                    format!("Resume rejected: Run status is {:?}. Workspace drift, ambiguous tool calls, or unfinalized runs cannot be automatically resumed.", analysis.status)
                )));
            }
        }
        "continue" => {
            if !analysis.is_safe_to_continue() {
                return Err(gestalt_core::HarnessError::Policy(gestalt_core::PolicyError::Denied(
                    format!("Continue rejected: Run status is {:?}. Only completed head runs can be continued.", analysis.status)
                )));
            }
        }
        _ => {} // Branch can fork from checkpoints
    }

    let resolved_provider = config.resolve_provider()?;

    // 3. Reconstruct history up to chosen checkpoint
    let (history, context_state, token_budget, last_checkpoint_seq) =
        if let Some(target_seq) = branch_checkpoint {
            let trace_path = parent_run_path.join("trace.jsonl");
            if !trace_path.exists() {
                return Err(gestalt_core::HarnessError::Trace(
                    gestalt_core::TraceError::ReadFailed {
                        reason: "trace.jsonl missing".to_string(),
                    },
                ));
            }
            let envelopes = gestalt_trace::read_trace(&trace_path)
                .map_err(|e| gestalt_core::HarnessError::Trace(e))?;
            let mut target_checkpoint = None;
            for env in &envelopes {
                if matches!(env.event, gestalt_core::AgentEvent::Checkpoint { .. })
                    && env.seq == target_seq
                {
                    target_checkpoint = Some(env);
                    break;
                }
            }
            match target_checkpoint {
                Some(env) => match &env.event {
                    gestalt_core::AgentEvent::Checkpoint {
                        history,
                        context_state,
                        token_budget,
                        ..
                    } => (
                        history.clone(),
                        gestalt_core::ContextProjectionState::clone(context_state),
                        token_budget.clone(),
                        Some(target_seq),
                    ),
                    _ => {
                        return Err(gestalt_core::HarnessError::Trace(
                            gestalt_core::TraceError::ReadFailed {
                                reason: format!("Event seq {} is not a Checkpoint", target_seq),
                            },
                        ))
                    }
                },
                None => {
                    return Err(gestalt_core::HarnessError::Trace(
                        gestalt_core::TraceError::ReadFailed {
                            reason: format!("Checkpoint with sequence {} not found", target_seq),
                        },
                    ))
                }
            }
        } else {
            (
                analysis.history.clone(),
                analysis.context_state.clone(),
                analysis.token_budget.clone(),
                analysis.last_checkpoint_seq,
            )
        };

    let (context_state, token_budget, _model_changed) = calculate_continuation_state(
        analysis.resolved_model.as_ref(),
        &resolved_provider.resolved_model,
        context_state,
        token_budget,
        config,
    );

    let session_id = analysis.session_id.clone();
    let parent_run_id = analysis.run_id.clone();

    // 4. Initialize dependencies and AgentRuntime
    let run_id = format!("run-{}", uuid::Uuid::new_v4());

    let (sink_inner, run_paths) = JsonlTraceSink::create_run(
        config.run_log_dir(),
        &session_id,
        &run_id,
        Some(current_snapshot.clone()),
    )?;
    let sink = Arc::new(sink_inner);

    let runtime = crate::runtime::build_cli_runtime(
        config,
        api_key,
        event_tx.clone(),
        approval_override,
        Some(sink.clone() as Arc<dyn gestalt_core::trace::TraceSink>),
    )
    .await?;

    let mode = config.selected_mode()?;
    let max_turns = config.max_turns();
    let mut meta_map = serde_json::Map::new();
    if let Some(ref thinking) = resolved_provider.resolved_options.thinking {
        meta_map.insert(
            "thinking".to_string(),
            serde_json::to_value(thinking).unwrap_or_default(),
        );
    }
    if let Some(ref adapter_opts) = resolved_provider.resolved_options.adapter_options {
        for (k, v) in adapter_opts {
            meta_map.insert(k.clone(), v.clone());
        }
    }
    let metadata = serde_json::Value::Object(meta_map);

    fn to_core_reasoning_effort(
        value: crate::config::ReasoningEffort,
    ) -> gestalt_core::provider::ReasoningEffort {
        match value {
            crate::config::ReasoningEffort::None => gestalt_core::provider::ReasoningEffort::None,
            crate::config::ReasoningEffort::Low => gestalt_core::provider::ReasoningEffort::Low,
            crate::config::ReasoningEffort::Medium => {
                gestalt_core::provider::ReasoningEffort::Medium
            }
            crate::config::ReasoningEffort::High => gestalt_core::provider::ReasoningEffort::High,
            crate::config::ReasoningEffort::Xhigh => gestalt_core::provider::ReasoningEffort::Xhigh,
        }
    }

    fn to_core_text_verbosity(
        value: crate::config::TextVerbosity,
    ) -> gestalt_core::provider::TextVerbosity {
        match value {
            crate::config::TextVerbosity::None => gestalt_core::provider::TextVerbosity::None,
            crate::config::TextVerbosity::Low => gestalt_core::provider::TextVerbosity::Low,
            crate::config::TextVerbosity::Medium => gestalt_core::provider::TextVerbosity::Medium,
            crate::config::TextVerbosity::High => gestalt_core::provider::TextVerbosity::High,
        }
    }

    let mut session = Session::new(
        session_id.clone(),
        SessionConfig {
            model: resolved_provider.model().to_string(),
            provider: resolved_provider.provider_name().to_string(),
            max_tokens: resolved_provider
                .resolved_options
                .max_output_tokens
                .or_else(|| u32::try_from(resolved_provider.resolved_model.max_output_tokens).ok())
                .unwrap_or(4096),
            temperature: resolved_provider.resolved_options.temperature,
            max_turns,
            top_p: resolved_provider.resolved_options.top_p,
            reasoning_effort: resolved_provider
                .resolved_options
                .reasoning_effort
                .map(to_core_reasoning_effort),
            text_verbosity: resolved_provider
                .resolved_options
                .text_verbosity
                .map(to_core_text_verbosity),
            metadata,
            resolved_model: Some(resolved_provider.resolved_model.clone()),
        },
        token_budget,
        ToolContext {
            working_dir: config.workspace_root.clone(),
            workspace_root: Some(config.workspace_root.clone()),
            timeout: Duration::from_secs(config.tools.bash_timeout_secs.unwrap_or(60)),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: config.tools.max_output_tokens.unwrap_or(4_000),
            artifact_dir: Some(run_paths.artifacts.clone()),
            current_tool_call_id: None,
            ignore_patterns: config.tools.ignore_patterns.clone().unwrap_or_default(),
        },
        mode,
        current_snapshot.clone(),
    );

    // Seed the session with reconstructed history
    session.history = history;
    session.context_state = context_state;

    let parent_cache_key = match (
        &analysis.resolved_model,
        &analysis.prompt_snapshot,
        &analysis.tool_schema_hash,
    ) {
        (Some(model), Some(snapshot), Some(tool_hash)) => {
            Some(gestalt_core::context::ProviderCacheKey {
                provider_id: model.selection.provider_id.clone(),
                api_format: model.api_format,
                model_id: model.selection.model_id.clone(),
                prompt_prefix_hash: snapshot.prefix_hash.clone(),
                tool_schema_hash: tool_hash.clone(),
            })
        }
        _ => None,
    };

    let current_tool_hash = gestalt_trace::run_manifest::compute_tool_schema_hash(&tools.schemas());
    let current_cache_key =
        analysis
            .prompt_snapshot
            .as_ref()
            .map(|snapshot| gestalt_core::context::ProviderCacheKey {
                provider_id: resolved_provider
                    .resolved_model
                    .selection
                    .provider_id
                    .clone(),
                api_format: resolved_provider.resolved_model.api_format,
                model_id: resolved_provider.resolved_model.selection.model_id.clone(),
                prompt_prefix_hash: snapshot.prefix_hash.clone(),
                tool_schema_hash: current_tool_hash,
            });

    let cache_matches = match (&parent_cache_key, &current_cache_key) {
        (Some(p_key), Some(c_key)) => p_key == c_key,
        _ => false,
    };

    let initial_prompt_snapshot_hash = if cache_matches {
        if let Some(prompt_snapshot) = analysis.prompt_snapshot.as_ref() {
            let loaded_event = AgentEvent::PromptSnapshotLoaded {
                snapshot_hash: prompt_snapshot.snapshot_hash.clone(),
                source: gestalt_trace::run_manifest::PROMPT_SNAPSHOT_RELATIVE_PATH.to_string(),
            };
            sink.emit(loaded_event.clone())?;
            if let Some(ref tx) = event_tx {
                let _ = tx.send(loaded_event);
            } else if let Some(line) = render_event(&loaded_event) {
                println!("{line}");
            }
        }
        analysis
            .prompt_snapshot
            .as_ref()
            .map(|snapshot| snapshot.snapshot_hash.clone())
    } else {
        None
    };

    // 5. Setup RunManifest
    let run_kind = match action {
        "continue" => RunKind::Continue,
        "resume" => RunKind::Resume,
        _ => RunKind::Branch,
    };

    let run_manifest_path = run_paths.root.join("run.json");
    let initial_manifest = RunManifest {
        v: 1,
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        parent_run_id: Some(parent_run_id),
        base_checkpoint: last_checkpoint_seq,
        run_kind,
        created_at: Utc::now(),
        lifecycle_state: LifecycleState::Running,
        finalized_at: None,
        failure_kind: None,
        interrupted_phase: None,
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        resolved_model: Some(resolved_provider.resolved_model.clone()),
        compatibility_fingerprint: expected_fingerprint.clone(),
    };
    initial_manifest
        .save_to(&run_manifest_path)
        .map_err(|e| gestalt_core::HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;

    let snapshot_id: String = current_snapshot.content_hash.chars().take(12).collect();
    let snapshot_event = AgentEvent::WorkspaceSnapshotCaptured {
        snapshot_id,
        dirty: current_snapshot.git_dirty.unwrap_or(false),
    };
    sink.emit(snapshot_event.clone())?;
    if let Some(ref tx) = event_tx {
        let _ = tx.send(snapshot_event);
    }

    // If continue or branch, we append the user's prompt as the next turn
    if let Some(ref p) = prompt {
        session.append_message(Message::User {
            content: vec![gestalt_core::ContentBlock::Text { text: p.clone() }],
            metadata: None,
        });
        let user_msg_event = AgentEvent::UserMessage { content: p.clone() };
        sink.emit(user_msg_event.clone())?;
        if let Some(ref tx) = event_tx {
            let _ = tx.send(user_msg_event);
        }
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<gestalt_core::AgentEvent>();
    let event_tx_clone = event_tx.clone();
    let render_task = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Some(ref user_tx) = event_tx_clone {
                let _ = user_tx.send(event.clone());
            } else if let Some(line) = render_event(&event) {
                println!("{line}");
            }
        }
    });

    let loop_result = runtime
        .run_session(
            &mut session,
            &cancel_token,
            Some(tx),
            initial_prompt_snapshot_hash,
        )
        .await;

    // Await rendering task to finish processing all events before completing
    let _ = render_task.await;

    let mut manifest = initial_manifest;
    manifest.finalized_at = Some(Utc::now());

    let final_status = match loop_result {
        Ok(result) => {
            manifest.lifecycle_state = LifecycleState::Completed;
            let _ = write_summary(&run_paths.summary, &result);
            crate::run::flush_trace_sink_with_warning(sink.as_ref(), event_tx.as_ref());
            let _ = write_cost_report_helper(&run_paths.trace, &run_paths.cost);
            Ok(run_paths.root.clone())
        }
        Err(gestalt_runtime::RuntimeError::Harness(
            gestalt_core::error::HarnessError::Cancelled,
        )) => {
            manifest.lifecycle_state = LifecycleState::Interrupted;
            manifest.interrupted_phase = Some("agent_loop".to_string());
            crate::run::flush_trace_sink_with_warning(sink.as_ref(), event_tx.as_ref());

            let mock_run_result = gestalt_core::session::RunResult {
                session_id: session.id.clone(),
                turns: session.history.len() / 2,
                stop_reason: gestalt_core::StopReason::EndTurn,
                total_input_tokens: 0,
                total_output_tokens: 0,
                artifacts: Vec::new(),
                workspace_snapshot_id: None,
            };
            let _ = write_summary(&run_paths.summary, &mock_run_result);
            let _ = write_cost_report_helper(&run_paths.trace, &run_paths.cost);
            Err(gestalt_core::HarnessError::Cancelled)
        }
        Err(err) => {
            manifest.lifecycle_state = LifecycleState::Failed;
            manifest.failure_kind = Some(format!("{:?}", err));
            crate::run::flush_trace_sink_with_warning(sink.as_ref(), event_tx.as_ref());

            let mock_run_result = gestalt_core::session::RunResult {
                session_id: session.id.clone(),
                turns: session.history.len() / 2,
                stop_reason: gestalt_core::StopReason::EndTurn,
                total_input_tokens: 0,
                total_output_tokens: 0,
                artifacts: Vec::new(),
                workspace_snapshot_id: None,
            };
            let _ = write_summary(&run_paths.summary, &mock_run_result);
            let _ = write_cost_report_helper(&run_paths.trace, &run_paths.cost);
            match err {
                gestalt_runtime::RuntimeError::Harness(he) => Err(he),
                other => Err(gestalt_core::HarnessError::Config(
                    gestalt_core::error::ConfigError::InvalidValue {
                        field: "runtime".to_string(),
                        reason: other.to_string(),
                    },
                )),
            }
        }
    };

    let prompt_snapshot_path = run_paths
        .root
        .join(gestalt_trace::run_manifest::PROMPT_SNAPSHOT_RELATIVE_PATH);
    if let Ok(snapshot) = read_prompt_snapshot(&prompt_snapshot_path) {
        manifest.prompt_snapshot_hash = Some(snapshot.snapshot_hash);
        manifest.prompt_snapshot_path =
            Some(gestalt_trace::run_manifest::PROMPT_SNAPSHOT_RELATIVE_PATH.to_string());
    }

    let _ = manifest.save_to(&run_manifest_path);
    final_status
}

fn write_cost_report_helper(
    trace_path: &std::path::Path,
    cost_path: &std::path::Path,
) -> Result<(), gestalt_core::HarnessError> {
    let report = aggregate_costs(trace_path, |model| {
        gestalt_models::ModelCatalog::new().get(model)
    })?;
    write_cost_report(cost_path, &report)?;
    Ok(())
}
