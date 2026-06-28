#![allow(
    clippy::pedantic,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::too_many_lines
)]

use std::io::{self, Write as _};
use std::sync::Arc;
use tokio::io::AsyncBufReadExt as _;

use gestalt_core::{HarnessError, ToolCatalog, WorkspaceSnapshotter};
use gestalt_tools::default_registry;
use gestalt_trace::resume::ResumeAnalyzer;
use gestalt_trace::run_manifest::{CompatibilityFingerprint, RunManifest};

use crate::slash::{handle_slash_command, SlashOutcome};
use gestalt_app::config::{load_effective_config, CliOverrides, EffectiveConfig};
use gestalt_app::run::run_prompt;
use gestalt_app::sessions::run_session_action;

/// Entry point for running the interactive chat REPL.
pub async fn run_chat(
    overrides: &CliOverrides,
    resume_run_id_or_path: Option<String>,
    api_key: Option<String>,
    cancel_token: gestalt_core::cancel::CancelToken,
) -> Result<(), HarnessError> {
    let mut overrides_clone = overrides.clone();
    let mut config = load_effective_config(&overrides_clone)?;

    let mut session_id = format!("session-{}", uuid::Uuid::new_v4());
    let mut parent_run_id: Option<String> = None;

    if let Some(ref target) = resume_run_id_or_path {
        let parent_run_path = crate::runs::resolve_run_path(&config, target)?;
        let manifest_path = parent_run_path.join("run.json");
        if !manifest_path.exists() {
            return Err(HarnessError::Config(
                gestalt_core::ConfigError::InvalidValue {
                    field: "resume".to_string(),
                    reason: format!("run.json missing from {}", parent_run_path.display()),
                },
            ));
        }

        let manifest = RunManifest::load_from(&manifest_path).map_err(|e| {
            HarnessError::Trace(gestalt_core::TraceError::ReadFailed {
                reason: e.to_string(),
            })
        })?;

        let snapshotter = gestalt_runtime::GitWorkspaceSnapshotter;
        let current_snapshot = snapshotter.capture(&config.workspace_root).await?;
        let tools = Arc::new(default_registry()?);
        let skill_explicit: Vec<std::path::PathBuf> = config
            .skills
            .explicit_paths
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        let skill_discovery = gestalt_app::runtime_factory::build_skill_discovery(&config);
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
            tool_schema_hash: gestalt_trace::run_manifest::compute_tool_schema_hash(
                &tools.schemas(),
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
            skill_fingerprint: gestalt_app::run::compute_skill_fingerprint(
                &config,
                &discovered_skills,
                None,
            ),
            workspace_context_snapshot_hash,
        };

        let analysis = ResumeAnalyzer::analyze(
            &parent_run_path,
            Some(&current_snapshot),
            Some(&expected_fingerprint),
        );

        if !analysis.is_safe_to_continue() && !analysis.is_safe_to_resume() {
            return Err(HarnessError::Policy(gestalt_core::PolicyError::Denied(
                format!("Resume rejected: Run status is {:?}. Workspace drift, ambiguous tool calls, or unfinalized runs cannot be automatically resumed.", analysis.status)
            )));
        }

        session_id = manifest.session_id;
        parent_run_id = Some(manifest.run_id);
        println!(
            "Resumed session {session_id} at run {}",
            parent_run_id.as_ref().unwrap()
        );
    } else {
        println!("Started new session {session_id}");
    }

    let mut reader = tokio::io::BufReader::new(tokio::io::stdin());

    loop {
        if cancel_token.is_cancelled() {
            break;
        }
        print!("gestalt> ");
        if io::stdout().flush().is_err() {
            break;
        }

        let mut input = String::new();
        tokio::select! {
            res = reader.read_line(&mut input) => {
                match res {
                    Ok(0) => break, // EOF
                    Ok(_) => {}
                    Err(e) => return Err(HarnessError::Trace(gestalt_core::TraceError::ReadFailed { reason: e.to_string() })),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!();
                break;
            }
            () = cancel_token.cancelled() => {
                break;
            }
        }

        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('/') {
            match handle_slash_command(
                trimmed,
                &session_id,
                parent_run_id.as_deref(),
                &mut overrides_clone,
                &config,
            )
            .await
            {
                Ok(SlashOutcome::Quit) => break,
                Ok(SlashOutcome::ChangeMode(new_mode)) => {
                    overrides_clone.mode = Some(new_mode);
                    config = load_effective_config(&overrides_clone)?;
                }
                Ok(SlashOutcome::SkillActivated(name)) => {
                    overrides_clone
                        .skills
                        .retain(|skill| skill != &name && skill != &format!("!{name}"));
                    overrides_clone.skills.push(name.clone());
                    config = load_effective_config(&overrides_clone)?;
                    println!("Skill '{name}' activated for this session.");
                }
                Ok(SlashOutcome::SkillDeactivated(name)) => {
                    overrides_clone
                        .skills
                        .retain(|skill| skill != &name && skill != &format!("!{name}"));
                    overrides_clone.skills.push(format!("!{name}"));
                    config = load_effective_config(&overrides_clone)?;
                    println!("Skill '{name}' deactivated for this session.");
                }
                Ok(SlashOutcome::None) => {}
                Err(e) => {
                    eprintln!("error executing command: {e}");
                }
            }
            continue;
        }

        // Execute the user's prompt as a run in the session lineage
        let turn_cancel = gestalt_core::cancel::CancelToken::new();
        let turn_cancel_clone = turn_cancel.clone();

        let cancel_watcher = tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\n[Interrupt] Cancellation requested. Cleaning up turn...");
                turn_cancel_clone.cancel();
            }
        });

        let (event_tx, printer_handle) = {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<gestalt_core::AgentEvent>();
            let handle = tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    if let Some(line) = crate::output::render_event(&event) {
                        println!("{line}");
                    }
                }
            });
            (tx, handle)
        };

        let approval = Some(Arc::new(crate::approval::CliApprovalProvider)
            as Arc<dyn gestalt_core::ApprovalProvider>);
        let interaction = Some(Arc::new(crate::approval::CliInteractionProvider)
            as Arc<dyn gestalt_app::InteractionProvider>);

        let res = if let Some(ref parent) = parent_run_id {
            run_session_action(
                &config,
                "branch",
                parent,
                Some(trimmed.to_string()),
                None,
                api_key.clone(),
                turn_cancel,
                approval,
                Some(event_tx),
                interaction,
            )
            .await
        } else {
            run_prompt(
                &config,
                trimmed,
                api_key.clone(),
                turn_cancel,
                approval,
                Some(event_tx),
                Some(session_id.clone()),
                interaction,
            )
            .await
        };

        cancel_watcher.abort();
        let _ = printer_handle.await;

        match res {
            Ok(run_dir) => {
                let manifest_path = run_dir.join("run.json");
                if let Ok(manifest) = RunManifest::load_from(&manifest_path) {
                    parent_run_id = Some(manifest.run_id);
                    session_id = manifest.session_id;
                }
            }
            Err(HarnessError::Cancelled) => {
                println!("Turn cancelled.");
                if let Some(latest) = find_latest_run_id(&config, &session_id) {
                    parent_run_id = Some(latest);
                }
            }
            Err(err) => {
                eprintln!("error: {err}");
                if let Some(latest) = find_latest_run_id(&config, &session_id) {
                    parent_run_id = Some(latest);
                }
            }
        }
    }

    Ok(())
}

fn find_latest_run_id(config: &EffectiveConfig, session_id: &str) -> Option<String> {
    let run_log_dir = config.run_log_dir();
    let mut latest_run: Option<(std::time::SystemTime, String)> = None;
    if run_log_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(run_log_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let manifest_path = entry.path().join("run.json");
                    if manifest_path.exists() {
                        if let Ok(manifest) = RunManifest::load_from(&manifest_path) {
                            if manifest.session_id == session_id {
                                if let Ok(metadata) = entry.metadata() {
                                    if let Ok(modified) = metadata.modified() {
                                        if latest_run.is_none()
                                            || modified > latest_run.as_ref().unwrap().0
                                        {
                                            latest_run = Some((modified, manifest.run_id));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    latest_run.map(|(_, id)| id)
}
