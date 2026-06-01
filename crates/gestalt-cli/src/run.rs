use std::{collections::HashMap, fs, path::PathBuf, sync::Arc, time::Duration};

use gestalt_context::MinimalContextPipeline;
use gestalt_core::{
    trace::TraceSink, AgentEvent, AgentLoop, ExecutionMode, Message, Session, SessionConfig,
    TokenBudget, ToolContext,
};
use gestalt_models::registry;
use gestalt_policy::{MinimalPolicyEngine, PolicyConfig};
use gestalt_tools::default_registry;
use gestalt_trace::{aggregate_costs, write_cost_report, write_summary, JsonlTraceSink};

use crate::{approval::CliApprovalProvider, config::EffectiveConfig, output::render_event};

pub async fn run_prompt(
    config: &EffectiveConfig,
    prompt: &str,
) -> Result<PathBuf, gestalt_core::HarnessError> {
    let provider_name = config.selected_provider()?;
    let provider = registry::get(&provider_name, config.provider_json(&provider_name))?;
    let provider_default_model = provider.default_model().to_string();

    let tools = Arc::new(default_registry()?);
    let pipeline = Arc::new(build_pipeline(config));
    let policy = Arc::new(build_policy(config)?);
    let approval = approval_provider(config.selected_mode()?);
    let max_turns = config.max_turns();
    let loop_ = AgentLoop::new(provider, tools, pipeline, policy, approval, max_turns);

    let model = config.selected_model().unwrap_or(provider_default_model);
    let session_id = format!("session-{}", std::process::id());
    let (sink, run_paths) = JsonlTraceSink::create_run(config.run_log_dir(), &session_id)?;

    let mut session = Session::new(
        session_id.clone(),
        SessionConfig {
            model,
            provider: provider_name.clone(),
            max_tokens: 4096,
            temperature: Some(0.0),
            max_turns,
        },
        TokenBudget {
            model_limit: config.context.max_context_window.unwrap_or(120_000),
            reserved_output: config.context.reserved_output_tokens.unwrap_or(8_000),
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 16,
        },
        ToolContext {
            working_dir: config.workspace_root.clone(),
            workspace_root: Some(config.workspace_root.clone()),
            timeout: Duration::from_secs(config.tools.bash_timeout_secs.unwrap_or(60)),
            allow_network: false,
            environment: HashMap::new(),
            max_output_bytes: config.tools.max_output_tokens.unwrap_or(4_000),
            artifact_dir: Some(run_paths.artifacts.clone()),
            current_tool_call_id: None,
        },
        config.selected_mode()?,
    );
    session.history.push(Message::User {
        content: vec![gestalt_core::ContentBlock::Text {
            text: prompt.to_string(),
        }],
    });
    sink.emit(AgentEvent::UserMessage {
        content: prompt.to_string(),
    })?;

    let result = loop_
        .run(&mut session, |event| {
            let _ = sink.emit(event.clone());
            if let Some(line) = render_event(&event) {
                println!("{line}");
            }
        })
        .await?;
    sink.flush()?;

    write_summary(&run_paths.summary, &result)?;
    let report = aggregate_costs(&run_paths.trace, |model| {
        gestalt_models::ModelCatalog::new().get(model)
    })?;
    write_cost_report(&run_paths.cost, &report)?;
    Ok(run_paths.root)
}

fn build_pipeline(config: &EffectiveConfig) -> MinimalContextPipeline {
    let mut pipeline = MinimalContextPipeline::new("pipeline-v1");
    let workspace_md = config.workspace_file("workspace.md");
    if let Ok(content) = fs::read_to_string(workspace_md) {
        pipeline = pipeline.with_workspace_md(content);
    }
    let memory_md = config.workspace_file("memory.md");
    if let Ok(content) = fs::read_to_string(memory_md) {
        pipeline = pipeline.with_memory_md(content);
    }
    pipeline
}

fn build_policy(
    config: &EffectiveConfig,
) -> Result<MinimalPolicyEngine, gestalt_core::HarnessError> {
    let policies = config.workspace_file("policies.toml");
    let policy = if policies.exists() {
        PolicyConfig::from_file(policies)?
    } else {
        PolicyConfig::default()
    };
    Ok(MinimalPolicyEngine::new(policy))
}

fn approval_provider(mode: ExecutionMode) -> Arc<dyn gestalt_core::ApprovalProvider> {
    match mode {
        ExecutionMode::Yolo => Arc::new(CliApprovalProvider),
        _ => Arc::new(CliApprovalProvider),
    }
}
