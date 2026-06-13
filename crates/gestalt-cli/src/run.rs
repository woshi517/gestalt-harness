use std::{fs, path::PathBuf, sync::Arc};

use gestalt_context::MinimalContextPipeline;
use gestalt_core::{
    trace::TraceSink, AgentEvent, ExecutionMode, PromptAssemblyStrategy, WorkspaceSnapshotter,
};
use gestalt_policy::MinimalPolicyEngine;
use gestalt_trace::{
    aggregate_costs, read_prompt_snapshot, write_cost_report, write_summary, JsonlTraceSink,
};

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
    )
    .await?;

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
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        compatibility_fingerprint: gestalt_trace::run_manifest::CompatibilityFingerprint {
            context_pipeline_version: "pipeline-v1".to_string(),
            tool_schema_hash: gestalt_trace::run_manifest::compute_tool_schema_hash(
                &runtime.tools.schemas(),
            ),
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
            skill_fingerprint: compute_skill_fingerprint(
                config,
                &runtime.config.discovered_skills,
                Some(prompt),
            ),
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
            flush_trace_sink_with_warning(sink.as_ref(), event_tx.as_ref());
            let _ = write_cost_report_helper(&run_paths.trace, &run_paths.cost);
            Ok(run_paths.root.clone())
        }
        Err(gestalt_runtime::RuntimeError::Harness(
            gestalt_core::error::HarnessError::Cancelled,
        )) => {
            manifest.lifecycle_state = gestalt_trace::run_manifest::LifecycleState::Interrupted;
            manifest.interrupted_phase = Some("agent_loop".to_string());
            flush_trace_sink_with_warning(sink.as_ref(), event_tx.as_ref());

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
            flush_trace_sink_with_warning(sink.as_ref(), event_tx.as_ref());

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
                other => Err(gestalt_core::HarnessError::Config(
                    gestalt_core::error::ConfigError::InvalidValue {
                        field: "runtime".to_string(),
                        reason: other.to_string(),
                    },
                )),
            }
        }
    };

    manifest.compatibility_fingerprint.skill_fingerprint = runtime.inspect().skill_fingerprint;

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

    let assembly_strategy = config
        .prompt
        .assembly_strategy
        .unwrap_or(PromptAssemblyStrategy::Snapshot);
    pipeline = pipeline.with_prompt_assembly_strategy(assembly_strategy);

    if let Some(prompt_cfg) = &config.prompt.r#override {
        pipeline = pipeline.with_prompt_override(prompt_cfg);
    } else if let Some(file_path) = &config.prompt.override_file {
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
            gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                field: "workspace_root".to_string(),
                reason: format!("Failed to canonicalize workspace root: {e}"),
            })
        })?;
        let canonical_target = fs::canonicalize(&target_path).map_err(|e| {
            gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                field: "prompt.override_file".to_string(),
                reason: format!("Failed to canonicalize override file: {e}"),
            })
        })?;
        if !canonical_target.starts_with(&canonical_root) {
            return Err(gestalt_core::HarnessError::Config(
                gestalt_core::error::ConfigError::InvalidValue {
                    field: "prompt.override_file".to_string(),
                    reason: format!("Override file path '{file_path}' escapes the workspace root"),
                },
            ));
        }
        let content = fs::read_to_string(&canonical_target).map_err(|e| {
            gestalt_core::HarnessError::Config(gestalt_core::error::ConfigError::InvalidValue {
                field: "prompt.override_file".to_string(),
                reason: format!("Failed to read override file '{file_path}': {e}"),
            })
        })?;
        pipeline = pipeline.with_prompt_override_file(file_path, content);
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

pub(crate) fn build_policy(config: &EffectiveConfig) -> MinimalPolicyEngine {
    let policy = config.policies.to_policy_config();
    MinimalPolicyEngine::new(policy)
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
    event: AgentEvent,
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

pub(crate) fn flush_trace_sink_with_warning(
    sink: &dyn TraceSink,
    event_tx: Option<&tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
) {
    if let Err(err) = sink.flush() {
        let event = AgentEvent::Error {
            message: format!("trace flush failed: {err}"),
            recoverable: true,
        };

        if let Some(tx) = event_tx {
            if tx.send(event.clone()).is_ok() {
                return;
            }
        }

        if let Some(line) = render_event(&event) {
            eprintln!("{line}");
        } else {
            eprintln!("trace flush failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gestalt_core::{error::TraceError, trace::TraceSink, AgentEvent, PromptAssemblyStrategy};

    use super::{emit_trace_event, flush_trace_sink_with_warning};

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

    #[derive(Default)]
    struct FlushFailingTraceSink;

    impl TraceSink for FlushFailingTraceSink {
        fn emit(&self, _event: AgentEvent) -> Result<(), TraceError> {
            Ok(())
        }

        fn flush(&self) -> Result<(), TraceError> {
            Err(TraceError::WriteFailed(std::io::Error::other(
                "trace flush boom",
            )))
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
    fn trace_flush_failure_is_forwarded_as_error_event() {
        let sink = FlushFailingTraceSink;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        flush_trace_sink_with_warning(&sink, Some(&tx));

        let event = rx.try_recv().expect("expected flush failure event");
        match event {
            AgentEvent::Error {
                message,
                recoverable,
            } => {
                assert!(recoverable);
                assert!(message.contains("trace flush failed"));
            }
            other => panic!("expected error event, got {other:?}"),
        }
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
            config_path: temp_dir.join("gestalt.json"),
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
                ignore_patterns: None,
            },
            context: ContextConfig {
                max_context_window: None,
                reserved_output_tokens: None,
                workspace_file: None,
                memory_file: None,
            },
            observe: ObserveConfig {
                run_log_dir: None,
                log_format: None,
            },
            providers: HashMap::new(),
            profiles: HashMap::new(),
            prompt: crate::config::PromptConfig::default(),
            policies: crate::config::PoliciesConfig::default(),
            provider_override: None,
            model_override: None,
            tui: crate::config::TuiConfig::default(),
            extensions: Default::default(),
            skills: Default::default(),
            mcp: None,
        };

        // Scenario 1: No prompt override => default prompt source
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
        assert_eq!(
            packet.prompt_assembly_strategy,
            PromptAssemblyStrategy::Snapshot
        );
        assert!(packet.cache_plan.is_some());

        // Scenario 2: inline prompt.override wins
        let mut config = config.clone();
        config.prompt.r#override = Some("Manual override text".to_string());
        let pipeline = super::build_pipeline(
            &config,
            gestalt_core::session::ExecutionMode::Confirm,
            3,
            &[],
        )
        .unwrap();
        let packet = pipeline.build_packet(&[], &budget);
        assert_eq!(packet.prompt_source.as_deref(), Some("override"));

        // Scenario 3: prompt.override_file (valid)
        let mut config = config.clone();
        config.prompt = crate::config::PromptConfig {
            r#override: None,
            override_file: Some(".gestalt/custom_prompt.md".to_string()),
            assembly_strategy: None,
        };
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

        // Scenario 4: prompt.override_file (missing)
        let mut config = config.clone();
        config.prompt = crate::config::PromptConfig {
            r#override: None,
            override_file: Some(".gestalt/nonexistent_prompt.md".to_string()),
            assembly_strategy: None,
        };
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

        // Scenario 5: prompt.override_file escaping workspace root
        let mut config = config.clone();
        config.prompt = crate::config::PromptConfig {
            r#override: None,
            override_file: Some("../../../etc/passwd".to_string()),
            assembly_strategy: None,
        };
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

        // Clean up
        fs::remove_dir_all(&temp_dir).unwrap();
    }
}

pub fn compute_skill_fingerprint(
    config: &EffectiveConfig,
    discovered: &[gestalt_skills::SkillDescriptor],
    current_task: Option<&str>,
) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();

    let active_descriptors: Vec<&gestalt_skills::SkillDescriptor> = if let Some(task) = current_task
    {
        let index = gestalt_skills::SkillIndex::new(discovered.to_vec());
        let state = gestalt_skills::activation::ActivationState::new(config.skills.active.clone());
        let resolved = gestalt_skills::ActivationEngine::resolve(&index, &state, Some(task));
        resolved
            .iter()
            .filter_map(|name| discovered.iter().find(|skill| &skill.name == name))
            .collect()
    } else {
        discovered
            .iter()
            .filter(|skill| config.skills.active.iter().any(|name| name == &skill.name))
            .collect()
    };

    if active_descriptors.is_empty() && config.skills.explicit_paths.is_empty() {
        return None;
    }

    // Hash every active skill by name AND its actual manifest content hash so
    // that editing an active skill's SKILL.md invalidates the fingerprint
    // and forces an explicit resume decision.
    let mut active_pairs: Vec<(&str, &str)> = active_descriptors
        .into_iter()
        .map(|s| (s.name.as_str(), s.manifest_hash.as_str()))
        .collect();
    active_pairs.sort_by(|a, b| a.0.cmp(b.0));
    for (name, manifest_hash) in &active_pairs {
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        hasher.update(manifest_hash.as_bytes());
        hasher.update(b"\0");
    }

    // Also fold in explicit paths that are not in the discovered set (shouldn't
    // normally happen, but catches the case where a path was provided but the
    // skill could not be loaded).
    let discovered_paths: std::collections::HashSet<String> = discovered
        .iter()
        .map(|s| s.manifest_path.to_string_lossy().into_owned())
        .collect();
    let mut explicit_paths: Vec<&str> = config
        .skills
        .explicit_paths
        .iter()
        .map(String::as_str)
        .filter(|p| !discovered_paths.contains(*p))
        .collect();
    explicit_paths.sort_unstable();
    for path in &explicit_paths {
        hasher.update(path.as_bytes());
    }

    Some(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod fingerprint_tests {
    use super::compute_skill_fingerprint;
    use crate::config::{
        ContextConfig, DefaultsConfig, EffectiveConfig, ObserveConfig, SkillsConfig, ToolsConfig,
    };
    use gestalt_skills::{SkillDescriptor, SkillSource, SkillTrustLevel};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_descriptor(name: &str, manifest_hash: &str) -> SkillDescriptor {
        SkillDescriptor {
            name: name.to_string(),
            description: format!("Description for {name}"),
            skill_root: PathBuf::from("/tmp"),
            manifest_path: PathBuf::from("/tmp/SKILL.md"),
            manifest_hash: manifest_hash.to_string(),
            trust_level: SkillTrustLevel::Workspace,
            source: SkillSource::WorkspaceLocal,
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: None,
        }
    }

    fn empty_config() -> EffectiveConfig {
        use crate::config::{ExtensionsConfig, PoliciesConfig, PromptConfig, TuiConfig};
        use std::collections::HashMap;
        EffectiveConfig {
            workspace_root: PathBuf::from("/tmp"),
            config_path: PathBuf::from("/tmp/gestalt.json"),
            defaults: DefaultsConfig::default(),
            tools: ToolsConfig::default(),
            context: ContextConfig::default(),
            observe: ObserveConfig::default(),
            providers: HashMap::new(),
            profiles: HashMap::new(),
            prompt: PromptConfig::default(),
            policies: PoliciesConfig::default(),
            provider_override: None,
            model_override: None,
            tui: TuiConfig::default(),
            extensions: ExtensionsConfig::default(),
            skills: SkillsConfig::default(),
            mcp: None,
        }
    }

    #[test]
    fn fingerprint_none_when_no_skills() {
        let config = empty_config();
        let fingerprint = compute_skill_fingerprint(&config, &[], None);
        assert!(fingerprint.is_none());
    }

    #[test]
    fn fingerprint_changes_when_manifest_content_changes() {
        // Build a config with two active skills, and a discovered set whose
        // manifest hashes differ from each other.
        let mut config = empty_config();
        config.skills.active = vec!["pdf".to_string(), "search".to_string()];
        let discovered = vec![
            make_descriptor("pdf", "hash-pdf-v1"),
            make_descriptor("search", "hash-search-v1"),
        ];
        let f1 =
            compute_skill_fingerprint(&config, &discovered, None).expect("fingerprint present");

        // Mutate the manifest hash of "pdf" (simulating an in-place edit of
        // SKILL.md) and re-compute. The fingerprint MUST change because the
        // plan requires replay-safety on manifest content, not just names.
        let discovered_v2 = vec![
            make_descriptor("pdf", "hash-pdf-v2"),
            make_descriptor("search", "hash-search-v1"),
        ];
        let f2 =
            compute_skill_fingerprint(&config, &discovered_v2, None).expect("fingerprint present");
        assert_ne!(
            f1, f2,
            "skill_fingerprint must change when an active skill's manifest content changes"
        );
    }

    #[test]
    fn fingerprint_stable_when_manifest_unchanged() {
        let mut config = empty_config();
        config.skills.active = vec!["pdf".to_string()];
        let discovered = vec![make_descriptor("pdf", "hash-pdf-v1")];
        let f1 =
            compute_skill_fingerprint(&config, &discovered, None).expect("fingerprint present");
        let f2 =
            compute_skill_fingerprint(&config, &discovered, None).expect("fingerprint present");
        assert_eq!(
            f1, f2,
            "fingerprint must be deterministic for the same inputs"
        );
    }

    #[test]
    fn fingerprint_changes_when_active_set_changes() {
        let mut config = empty_config();
        config.skills.active = vec!["pdf".to_string()];
        let discovered = vec![make_descriptor("pdf", "hash-pdf-v1")];
        let f1 =
            compute_skill_fingerprint(&config, &discovered, None).expect("fingerprint present");

        config.skills.active = vec!["pdf".to_string(), "search".to_string()];
        let discovered = vec![
            make_descriptor("pdf", "hash-pdf-v1"),
            make_descriptor("search", "hash-search-v1"),
        ];
        let f2 =
            compute_skill_fingerprint(&config, &discovered, None).expect("fingerprint present");
        assert_ne!(
            f1, f2,
            "fingerprint must change when the active set changes"
        );
    }

    #[test]
    fn fingerprint_folds_in_unmatched_explicit_paths() {
        // If a user provided an explicit path that didn't end up in the
        // discovered set (e.g. a broken skill), the path itself should still
        // be reflected in the fingerprint so the user can detect drift.
        let mut config = empty_config();
        config.skills.explicit_paths = vec!["/path/to/broken-skill".to_string()];
        let f1 = compute_skill_fingerprint(&config, &[], None).expect("fingerprint present");
        let mut config2 = config.clone();
        config2.skills.explicit_paths = vec!["/path/to/different-skill".to_string()];
        let f2 = compute_skill_fingerprint(&config2, &[], None).expect("fingerprint present");
        assert_ne!(f1, f2);
    }

    #[test]
    fn fingerprint_includes_trigger_activated_skill() {
        let config = empty_config();
        let discovered = vec![make_descriptor("pdf-processing", "Process PDF documents.")];
        assert!(compute_skill_fingerprint(&config, &discovered, None).is_none());
        let fingerprint =
            compute_skill_fingerprint(&config, &discovered, Some("Please process this PDF"))
                .expect("fingerprint present");
        assert!(!fingerprint.is_empty());
    }
}
