use std::{fs, path::PathBuf, sync::Arc};

use gestalt_context::MinimalContextPipeline;
use gestalt_core::{
    trace::TraceSink, ExecutionMode, WorkspaceSnapshotter,
};
use gestalt_policy::{MinimalPolicyEngine, PolicyConfig};
use gestalt_trace::{aggregate_costs, write_cost_report, write_summary, JsonlTraceSink};

use crate::{approval::CliApprovalProvider, config::EffectiveConfig, output::render_event};

pub async fn run_prompt(
    config: &EffectiveConfig,
    prompt: &str,
    api_key: Option<String>,
    cancel_token: gestalt_core::cancel::CancelToken,
    approval_override: Option<Arc<dyn gestalt_core::ApprovalProvider>>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<gestalt_core::AgentEvent>>,
    session_id_override: Option<String>,
) -> Result<PathBuf, gestalt_core::HarnessError> {
    let session_id =
        session_id_override.unwrap_or_else(|| format!("session-{}", uuid::Uuid::new_v4()));
    let run_id = format!("run-{}", uuid::Uuid::new_v4());

    let snapshotter = gestalt_core::snapshot::GitWorkspaceSnapshotter;
    let snapshot = snapshotter.capture(&config.workspace_root).await?;

    let (sink_inner, run_paths) = JsonlTraceSink::create_run(
        config.run_log_dir(),
        &session_id,
        &run_id,
        Some(snapshot.clone()),
    )?;
    let sink = Arc::new(sink_inner);

    let runtime = crate::runtime::build_cli_runtime(
        config,
        api_key,
        event_tx.clone(),
        approval_override,
        Some(sink.clone() as Arc<dyn gestalt_core::trace::TraceSink>),
    )?;

    // Initial manifest setup and save
    let run_manifest_path = run_paths.root.join("run.json");
    let initial_manifest = gestalt_trace::run_manifest::RunManifest {
        v: 1,
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        parent_run_id: None,
        base_checkpoint: None,
        run_kind: gestalt_trace::run_manifest::RunKind::New,
        created_at: chrono::Utc::now(),
        lifecycle_state: gestalt_trace::run_manifest::LifecycleState::Running,
        finalized_at: None,
        failure_kind: None,
        interrupted_phase: None,
        compatibility_fingerprint: gestalt_trace::run_manifest::CompatibilityFingerprint {
            context_pipeline_version: "pipeline-v1".to_string(),
            tool_schema_hash: gestalt_trace::run_manifest::compute_tool_schema_hash(
                &runtime.tools.schemas(),
            ),
            policy_fingerprint: {
                let policies_path = config.workspace_file("policies.toml");
                let content = std::fs::read_to_string(&policies_path).unwrap_or_default();
                gestalt_trace::run_manifest::compute_policy_fingerprint(&content)
            },
            hook_contract_hash: {
                let hook_names = vec![
                    "VerificationToolHook".to_string(),
                    "EvaluatorHook".to_string(),
                ];
                gestalt_trace::run_manifest::compute_hook_contract_hash(&hook_names)
            },
            execution_mode: format!("{:?}", config.selected_mode()?),
        },
    };
    initial_manifest
        .save_to(&run_manifest_path)
        .map_err(|e| gestalt_core::HarnessError::Trace(gestalt_core::TraceError::WriteFailed(e)))?;

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

    let input = gestalt_runtime::UserInput {
        prompt: prompt.to_string(),
        session_id: Some(session_id.clone()),
        cancel_token: cancel_token.clone(),
        event_tx: Some(tx),
        artifact_dir: Some(run_paths.artifacts.clone()),
    };

    let loop_result = runtime.run_prompt(input).await;

    // Await rendering task to finish processing all events before completing
    let _ = render_task.await;

    let mut manifest = initial_manifest;
    manifest.finalized_at = Some(chrono::Utc::now());

    let final_status = match loop_result {
        Ok(result) => {
            manifest.lifecycle_state = gestalt_trace::run_manifest::LifecycleState::Completed;
            let _ = write_summary(&run_paths.summary, &result);
            let _ = sink.flush();
            let _ = write_cost_report_helper(&run_paths.trace, &run_paths.cost);
            Ok(run_paths.root.clone())
        }
        Err(gestalt_runtime::RuntimeError::Harness(gestalt_core::error::HarnessError::Cancelled)) => {
            manifest.lifecycle_state = gestalt_trace::run_manifest::LifecycleState::Interrupted;
            manifest.interrupted_phase = Some("agent_loop".to_string());
            let _ = sink.flush();

            let mock_run_result = gestalt_core::session::RunResult {
                session_id,
                turns: 0,
                stop_reason: gestalt_core::event::StopReason::EndTurn,
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
            manifest.lifecycle_state = gestalt_trace::run_manifest::LifecycleState::Failed;
            manifest.failure_kind = Some(format!("{:?}", err));
            let _ = sink.flush();

            let mock_run_result = gestalt_core::session::RunResult {
                session_id,
                turns: 0,
                stop_reason: gestalt_core::event::StopReason::EndTurn,
                total_input_tokens: 0,
                total_output_tokens: 0,
                artifacts: Vec::new(),
                workspace_snapshot_id: None,
            };
            let _ = write_summary(&run_paths.summary, &mock_run_result);
            let _ = write_cost_report_helper(&run_paths.trace, &run_paths.cost);
            match err {
                gestalt_runtime::RuntimeError::Harness(he) => Err(he),
                other => Err(gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                    field: "runtime".to_string(),
                    reason: other.to_string(),
                })),
            }
        }
    };

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

pub fn build_pipeline(
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
        let toml_content = fs::read_to_string(&policies_path).map_err(|e| {
            gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                field: "policies.toml".to_string(),
                reason: format!("Failed to read policies.toml: {e}"),
            })
        })?;
        let value = toml::from_str::<toml::Value>(&toml_content).map_err(|e| {
            gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                field: "policies.toml".to_string(),
                reason: format!("Failed to parse policies.toml: {e}"),
            })
        })?;
        if let Some(prompt) = value.get("prompt") {
            if let Some(over_val) = prompt.get("override") {
                let over = over_val.as_str().ok_or_else(|| {
                    gestalt_core::HarnessError::Config(
                        gestalt_core::error::ConfigError::InvalidValue {
                            field: "prompt.override".to_string(),
                            reason: "Expected prompt.override to be a string".to_string(),
                        },
                    )
                })?;
                pipeline = pipeline.with_prompt_override(over);
            } else if let Some(file_path_val) = prompt.get("override_file") {
                let file_path = file_path_val.as_str().ok_or_else(|| {
                    gestalt_core::HarnessError::Config(
                        gestalt_core::error::ConfigError::InvalidValue {
                            field: "prompt.override_file".to_string(),
                            reason: "Expected prompt.override_file to be a string".to_string(),
                        },
                    )
                })?;
                let target_path = config.workspace_root.join(file_path);
                if !target_path.exists() {
                    return Err(gestalt_core::HarnessError::Config(
                        gestalt_core::error::ConfigError::InvalidValue {
                            field: "prompt.override_file".to_string(),
                            reason: format!("Override file '{file_path}' does not exist"),
                        },
                    ));
                }
                let canonical_root = fs::canonicalize(&config.workspace_root).map_err(|e| {
                    gestalt_core::HarnessError::Config(
                        gestalt_core::error::ConfigError::InvalidValue {
                            field: "workspace_root".to_string(),
                            reason: format!("Failed to canonicalize workspace root: {e}"),
                        },
                    )
                })?;
                let canonical_target = fs::canonicalize(&target_path).map_err(|e| {
                    gestalt_core::HarnessError::Config(
                        gestalt_core::error::ConfigError::InvalidValue {
                            field: "prompt.override_file".to_string(),
                            reason: format!("Failed to canonicalize override file: {e}"),
                        },
                    )
                })?;
                if !canonical_target.starts_with(&canonical_root) {
                    return Err(gestalt_core::HarnessError::Config(
                        gestalt_core::error::ConfigError::InvalidValue {
                            field: "prompt.override_file".to_string(),
                            reason: format!(
                                "Override file path '{file_path}' escapes the workspace root"
                            ),
                        },
                    ));
                }
                let content = fs::read_to_string(&canonical_target).map_err(|e| {
                    gestalt_core::HarnessError::Config(
                        gestalt_core::error::ConfigError::InvalidValue {
                            field: "prompt.override_file".to_string(),
                            reason: format!("Failed to read override file '{file_path}': {e}"),
                        },
                    )
                })?;
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

pub(crate) fn build_policy(
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

pub(crate) fn approval_provider(mode: ExecutionMode) -> Arc<dyn gestalt_core::ApprovalProvider> {
    match mode {
        ExecutionMode::Yolo => Arc::new(CliApprovalProvider),
        _ => Arc::new(CliApprovalProvider),
    }
}



#[allow(dead_code)]
pub(crate) fn emit_trace_event<S: TraceSink>(
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
        use crate::config::{
            ContextConfig, DefaultsConfig, EffectiveConfig, ObserveConfig, ToolsConfig,
        };
        use std::collections::HashMap;
        use std::fs;

        let temp_dir = std::env::temp_dir().join(format!(
            "gestalt-cli-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let gestalt_dir = temp_dir.join(".gestalt");
        fs::create_dir_all(&gestalt_dir).unwrap();

        let config = EffectiveConfig {
            workspace_root: temp_dir.clone(),
            defaults: DefaultsConfig {
                provider: None,
                model: None,
                mode: None,
                max_turns: None,
                profile: None,
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
            profiles: HashMap::new(),
            provider_override: None,
            model_override: None,
            tui: crate::config::TuiConfig::default(),
        };

        // Scenario 1: No policies.toml => uses default prompt
        let pipeline = super::build_pipeline(
            &config,
            gestalt_core::session::ExecutionMode::Confirm,
            3,
            &[],
        )
        .unwrap();
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
        let pipeline = super::build_pipeline(
            &config,
            gestalt_core::session::ExecutionMode::Confirm,
            3,
            &[],
        )
        .unwrap();
        let packet = pipeline.build_packet(&[], &budget);
        assert_eq!(packet.prompt_source.as_deref(), Some("override"));

        // Scenario 3: policies.toml with prompt.override_file (valid)
        let policies_toml_file = r#"
[prompt]
override_file = ".gestalt/custom_prompt.md"
"#;
        fs::write(gestalt_dir.join("policies.toml"), policies_toml_file).unwrap();
        fs::write(
            temp_dir.join(".gestalt/custom_prompt.md"),
            "File custom text",
        )
        .unwrap();
        let pipeline = super::build_pipeline(
            &config,
            gestalt_core::session::ExecutionMode::Confirm,
            3,
            &[],
        )
        .unwrap();
        let packet = pipeline.build_packet(&[], &budget);
        assert_eq!(
            packet.prompt_source.as_deref(),
            Some(".gestalt/custom_prompt.md")
        );

        // Scenario 4: policies.toml with prompt.override_file (missing)
        let policies_toml_missing = r#"
[prompt]
override_file = ".gestalt/nonexistent_prompt.md"
"#;
        fs::write(gestalt_dir.join("policies.toml"), policies_toml_missing).unwrap();
        let result = super::build_pipeline(
            &config,
            gestalt_core::session::ExecutionMode::Confirm,
            3,
            &[],
        );
        assert!(result.is_err());
        if let Err(gestalt_core::HarnessError::Config(
            gestalt_core::error::ConfigError::InvalidValue { field, reason },
        )) = result
        {
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
        let result = super::build_pipeline(
            &config,
            gestalt_core::session::ExecutionMode::Confirm,
            3,
            &[],
        );
        assert!(result.is_err());
        if let Err(gestalt_core::HarnessError::Config(
            gestalt_core::error::ConfigError::InvalidValue { field, reason },
        )) = result
        {
            assert_eq!(field, "prompt.override_file");
            assert!(
                reason.contains("escapes the workspace root") || reason.contains("does not exist")
            );
        } else {
            panic!("expected path escape or does not exist config error");
        }

        // Scenario 6: policies.toml with invalid TOML syntax
        let policies_toml_invalid = r#"
[prompt
override = "Manual override text"
"#;
        fs::write(gestalt_dir.join("policies.toml"), policies_toml_invalid).unwrap();
        let result = super::build_pipeline(
            &config,
            gestalt_core::session::ExecutionMode::Confirm,
            3,
            &[],
        );
        assert!(result.is_err());
        if let Err(gestalt_core::HarnessError::Config(
            gestalt_core::error::ConfigError::InvalidValue { field, reason },
        )) = result
        {
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
        let result = super::build_pipeline(
            &config,
            gestalt_core::session::ExecutionMode::Confirm,
            3,
            &[],
        );
        assert!(result.is_err());
        if let Err(gestalt_core::HarnessError::Config(
            gestalt_core::error::ConfigError::InvalidValue { field, reason },
        )) = result
        {
            assert_eq!(field, "prompt.override");
            assert!(reason.contains("Expected prompt.override to be a string"));
        } else {
            panic!("expected invalid value shape config error");
        }

        // Clean up
        fs::remove_dir_all(&temp_dir).unwrap();
    }
}
