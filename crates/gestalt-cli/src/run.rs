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

    let mut verifier_registry = gestalt_verify::VerifierRegistry::new();
    verifier_registry.register(Box::new(gestalt_verify::FileExistsVerifier));
    verifier_registry.register(Box::new(gestalt_verify::NoSecretsVerifier));
    verifier_registry.register(Box::new(gestalt_verify::PatchAppliesVerifier));
    verifier_registry.register(Box::new(gestalt_verify::MarkdownStructureVerifier));
    verifier_registry.register(Box::new(gestalt_verify::CommandVerifier::new(
        "echo 'Command verified'",
    )));

    let verification_hook = Arc::new(gestalt_verify::VerificationToolHook::new(verifier_registry));
    let mut hooks = gestalt_core::HookRegistry::new();
    hooks.register_tool_hook(verification_hook);

    let loop_ =
        AgentLoop::new(provider, tools, pipeline, policy, approval, max_turns).with_hooks(hooks);

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

    let mut trace_error_count = 0;
    let max_trace_errors = 3;

    let result = loop_
        .run(&mut session, |event| {
            emit_trace_event(
                &sink,
                event.clone(),
                &mut trace_error_count,
                max_trace_errors,
            )?;
            if let Some(line) = render_event(&event) {
                println!("{line}");
            }
            Ok(())
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

fn emit_trace_event<S: TraceSink>(
    sink: &S,
    event: gestalt_core::AgentEvent,
    trace_error_count: &mut usize,
    max_trace_errors: usize,
) -> Result<(), gestalt_core::HarnessError> {
    if let Err(err) = sink.emit(event) {
        *trace_error_count += 1;
        eprintln!("Trace emit error: {err}");
        if *trace_error_count >= max_trace_errors {
            return Err(gestalt_core::HarnessError::Trace(err));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gestalt_core::{error::TraceError, trace::TraceSink, AgentEvent};

    use super::emit_trace_event;

    #[derive(Default)]
    struct FailingTraceSink {
        emit_calls: Mutex<usize>,
    }

    impl TraceSink for FailingTraceSink {
        fn emit(&self, _event: AgentEvent) -> Result<(), TraceError> {
            {
                let mut calls = self.emit_calls.lock().expect("lock");
                *calls += 1;
            }
            Err(TraceError::WriteFailed(std::io::Error::other("trace boom")))
        }

        fn flush(&self) -> Result<(), TraceError> {
            Ok(())
        }
    }

    #[test]
    fn trace_events_abort_after_threshold() {
        let sink = FailingTraceSink::default();
        let mut trace_error_count = 0;

        let first = emit_trace_event(
            &sink,
            AgentEvent::UserMessage {
                content: "one".to_string(),
            },
            &mut trace_error_count,
            3,
        );
        assert!(first.is_ok());
        assert_eq!(trace_error_count, 1);

        let second = emit_trace_event(
            &sink,
            AgentEvent::UserMessage {
                content: "two".to_string(),
            },
            &mut trace_error_count,
            1,
        );
        assert!(second.is_err());
        assert_eq!(trace_error_count, 2);
    }
}
