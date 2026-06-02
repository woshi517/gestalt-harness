use std::{collections::HashMap, fs, path::PathBuf, sync::Arc, time::Duration};

use gestalt_context::MinimalContextPipeline;
use gestalt_core::{
    trace::TraceSink, AgentEvent, AgentLoop, ExecutionMode, Message, Session, SessionConfig,
    TokenBudget, ToolContext, WorkspaceSnapshotter, ToolCatalog,
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
    let mode = config.selected_mode()?;
    let max_turns = config.max_turns();
    let tool_names: Vec<String> = tools
        .schemas()
        .iter()
        .filter_map(|s| s.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect();
    let pipeline = Arc::new(build_pipeline(config, mode, max_turns, &tool_names)?);
    let policy = Arc::new(build_policy(config)?);
    let approval = approval_provider(config.selected_mode()?);

    let model = config.selected_model().unwrap_or(provider_default_model);
    let session_id = format!("session-{}", std::process::id());

    let snapshotter = gestalt_core::snapshot::GitWorkspaceSnapshotter;
    let snapshot = snapshotter.capture(&config.workspace_root).await?;

    let (sink_inner, run_paths) = JsonlTraceSink::create_run(config.run_log_dir(), &session_id, Some(snapshot.clone()))?;
    let sink = Arc::new(sink_inner);

    let mut verifier_registry = gestalt_verify::VerifierRegistry::new();
    verifier_registry.register(Box::new(gestalt_verify::FileExistsVerifier));
    verifier_registry.register(Box::new(gestalt_verify::NoSecretsVerifier));
    verifier_registry.register(Box::new(gestalt_verify::PatchAppliesVerifier));
    verifier_registry.register(Box::new(gestalt_verify::MarkdownStructureVerifier));
    verifier_registry.register(Box::new(gestalt_verify::CommandVerifier::new(
        "echo 'Command verified'",
    )));

    let verification_hook = Arc::new(gestalt_verify::VerificationToolHook::new(verifier_registry));
    let evaluator = Arc::new(gestalt_trace::evaluator::NoopTraceEvaluator);
    let sink_clone = sink.clone();
    let evaluator_hook = Arc::new(gestalt_trace::evaluator::EvaluatorHook::new(evaluator, None)
        .with_flush_trigger(Arc::new(move || {
            let _ = sink_clone.flush();
        })));
    let mut hooks = gestalt_core::HookRegistry::new();
    hooks.register_tool_hook(verification_hook);
    hooks.register_session_hook(evaluator_hook);

    let loop_ =
        AgentLoop::new(provider, tools, pipeline, policy, approval, max_turns).with_hooks(hooks);

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
        snapshot.clone(),
    );

    let snapshot_id: String = snapshot.content_hash.chars().take(12).collect();
    sink.emit(AgentEvent::WorkspaceSnapshotCaptured {
        snapshot_id,
        dirty: snapshot.git_dirty.unwrap_or(false),
    })?;
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
                &*sink,
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

fn build_pipeline(
    config: &EffectiveConfig,
    mode: ExecutionMode,
    max_turns: usize,
    tools: &[String],
) -> Result<MinimalContextPipeline, gestalt_core::HarnessError> {
    let mut pipeline = MinimalContextPipeline::new("pipeline-v1")
        .with_workspace_root(config.workspace_root.clone())
        .with_mode(format!("{mode:?}"))
        .with_max_turns(max_turns)
        .with_available_tools(tools.to_vec());

    let policies_path = config.workspace_file("policies.toml");
    if policies_path.exists() {
        let toml_content = fs::read_to_string(&policies_path)
            .map_err(|e| gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                field: "policies.toml".to_string(),
                reason: format!("Failed to read policies.toml: {e}"),
            }))?;
        let value = toml::from_str::<toml::Value>(&toml_content)
            .map_err(|e| gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                field: "policies.toml".to_string(),
                reason: format!("Failed to parse policies.toml: {e}"),
            }))?;
        if let Some(prompt) = value.get("prompt") {
            if let Some(over_val) = prompt.get("override") {
                let over = over_val.as_str().ok_or_else(|| {
                    gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                        field: "prompt.override".to_string(),
                        reason: "Expected prompt.override to be a string".to_string(),
                    })
                })?;
                pipeline = pipeline.with_prompt_override(over);
            } else if let Some(file_path_val) = prompt.get("override_file") {
                let file_path = file_path_val.as_str().ok_or_else(|| {
                    gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                        field: "prompt.override_file".to_string(),
                        reason: "Expected prompt.override_file to be a string".to_string(),
                    })
                })?;
                let target_path = config.workspace_root.join(file_path);
                if !target_path.exists() {
                    return Err(gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                        field: "prompt.override_file".to_string(),
                        reason: format!("Override file '{file_path}' does not exist"),
                    }));
                }
                let canonical_root = fs::canonicalize(&config.workspace_root)
                    .map_err(|e| gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                        field: "workspace_root".to_string(),
                        reason: format!("Failed to canonicalize workspace root: {e}"),
                    }))?;
                let canonical_target = fs::canonicalize(&target_path)
                    .map_err(|e| gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                        field: "prompt.override_file".to_string(),
                        reason: format!("Failed to canonicalize override file: {e}"),
                    }))?;
                if !canonical_target.starts_with(&canonical_root) {
                    return Err(gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                        field: "prompt.override_file".to_string(),
                        reason: format!("Override file path '{file_path}' escapes the workspace root"),
                    }));
                }
                let content = fs::read_to_string(&canonical_target)
                    .map_err(|e| gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                        field: "prompt.override_file".to_string(),
                        reason: format!("Failed to read override file '{file_path}': {e}"),
                    }))?;
                pipeline = pipeline.with_prompt_override_file(file_path, content);
            }
        }
    }

    let workspace_md = config.workspace_file("workspace.md");
    if let Ok(content) = fs::read_to_string(workspace_md) {
        pipeline = pipeline.with_workspace_md(content);
    }
    let memory_md = config.workspace_file("memory.md");
    if let Ok(content) = fs::read_to_string(memory_md) {
        pipeline = pipeline.with_memory_md(content);
    }
    Ok(pipeline)
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

    #[test]
    fn test_build_pipeline_override_scenarios() {
        use std::fs;
        use crate::config::{EffectiveConfig, DefaultsConfig, ToolsConfig, ContextConfig, ObserveConfig};
        use std::collections::HashMap;

        let temp_dir = std::env::temp_dir().join(format!("gestalt-cli-test-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let gestalt_dir = temp_dir.join(".gestalt");
        fs::create_dir_all(&gestalt_dir).unwrap();

        let config = EffectiveConfig {
            workspace_root: temp_dir.clone(),
            defaults: DefaultsConfig {
                provider: None,
                model: None,
                mode: None,
                max_turns: None,
            },
            tools: ToolsConfig {
                bash_timeout_secs: None,
                max_output_tokens: None,
                sandbox_type: None,
            },
            context: ContextConfig {
                max_context_window: None,
                reserved_output_tokens: None,
            },
            observe: ObserveConfig {
                run_log_dir: None,
                log_format: None,
            },
            providers: HashMap::new(),
        };

        // Scenario 1: No policies.toml => uses default prompt
        let pipeline = super::build_pipeline(&config, gestalt_core::session::ExecutionMode::Confirm, 3, &[]).unwrap();
        let budget = gestalt_core::context::TokenBudget {
            model_limit: 1000,
            reserved_output: 16,
            used_system: 0,
            used_history: 0,
            used_sources: 0,
            used_tools: 0,
            used_memory: 0,
            minimum_turn_budget: 8,
        };
        use gestalt_core::context::ContextPipeline as _;
        let packet = pipeline.build_packet(&[], &budget);
        assert_eq!(packet.prompt_source.as_deref(), Some("default"));

        // Scenario 2: policies.toml with prompt.override
        let policies_toml = r#"
[prompt]
override = "Manual override text"
"#;
        fs::write(gestalt_dir.join("policies.toml"), policies_toml).unwrap();
        let pipeline = super::build_pipeline(&config, gestalt_core::session::ExecutionMode::Confirm, 3, &[]).unwrap();
        let packet = pipeline.build_packet(&[], &budget);
        assert_eq!(packet.prompt_source.as_deref(), Some("override"));
        
        // Scenario 3: policies.toml with prompt.override_file (valid)
        let policies_toml_file = r#"
[prompt]
override_file = ".gestalt/custom_prompt.md"
"#;
        fs::write(gestalt_dir.join("policies.toml"), policies_toml_file).unwrap();
        fs::write(temp_dir.join(".gestalt/custom_prompt.md"), "File custom text").unwrap();
        let pipeline = super::build_pipeline(&config, gestalt_core::session::ExecutionMode::Confirm, 3, &[]).unwrap();
        let packet = pipeline.build_packet(&[], &budget);
        assert_eq!(packet.prompt_source.as_deref(), Some(".gestalt/custom_prompt.md"));

        // Scenario 4: policies.toml with prompt.override_file (missing)
        let policies_toml_missing = r#"
[prompt]
override_file = ".gestalt/nonexistent_prompt.md"
"#;
        fs::write(gestalt_dir.join("policies.toml"), policies_toml_missing).unwrap();
        let result = super::build_pipeline(&config, gestalt_core::session::ExecutionMode::Confirm, 3, &[]);
        assert!(result.is_err());
        if let Err(gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue { field, reason })) = result {
            assert_eq!(field, "prompt.override_file");
            assert!(reason.contains("does not exist"));
        } else {
            panic!("expected invalid value config error");
        }

        // Scenario 5: policies.toml with prompt.override_file escaping workspace root
        let policies_toml_escape = r#"
[prompt]
override_file = "../../../etc/passwd"
"#;
        fs::write(gestalt_dir.join("policies.toml"), policies_toml_escape).unwrap();
        let result = super::build_pipeline(&config, gestalt_core::session::ExecutionMode::Confirm, 3, &[]);
        assert!(result.is_err());
        if let Err(gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue { field, reason })) = result {
            assert_eq!(field, "prompt.override_file");
            assert!(reason.contains("escapes the workspace root") || reason.contains("does not exist"));
        } else {
            panic!("expected path escape or does not exist config error");
        }

        // Scenario 6: policies.toml with invalid TOML syntax
        let policies_toml_invalid = r#"
[prompt
override = "Manual override text"
"#;
        fs::write(gestalt_dir.join("policies.toml"), policies_toml_invalid).unwrap();
        let result = super::build_pipeline(&config, gestalt_core::session::ExecutionMode::Confirm, 3, &[]);
        assert!(result.is_err());
        if let Err(gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue { field, reason })) = result {
            assert_eq!(field, "policies.toml");
            assert!(reason.contains("Failed to parse policies.toml"));
        } else {
            panic!("expected parse error config error");
        }

        // Scenario 7: policies.toml with prompt.override as an integer (invalid value shape)
        let policies_toml_bad_shape = r#"
[prompt]
override = 12345
"#;
        fs::write(gestalt_dir.join("policies.toml"), policies_toml_bad_shape).unwrap();
        let result = super::build_pipeline(&config, gestalt_core::session::ExecutionMode::Confirm, 3, &[]);
        assert!(result.is_err());
        if let Err(gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue { field, reason })) = result {
            assert_eq!(field, "prompt.override");
            assert!(reason.contains("Expected prompt.override to be a string"));
        } else {
            panic!("expected invalid value shape config error");
        }

        // Clean up
        fs::remove_dir_all(&temp_dir).unwrap();
    }
}
