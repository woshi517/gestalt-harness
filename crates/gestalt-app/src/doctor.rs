use gestalt_core::error::{ConfigError, HarnessError};
use std::{fs, path::PathBuf};

use crate::auth::resolve_auth;
use crate::config::{load_effective_config, CliOverrides};
use crate::providers::probe_provider;
use crate::reports::{GlobalDoctorReport, WorkspaceDoctorReport};

pub async fn diagnose_workspace(
    overrides: &CliOverrides,
    live: bool,
) -> Result<GlobalDoctorReport, HarnessError> {
    let config_res = match load_effective_config(overrides) {
        Err(err @ HarnessError::Config(ConfigError::UnsupportedLegacyConfig { .. })) => {
            return Err(err);
        }
        result => result,
    };
    let mut config_valid = true;
    let mut config_error = None;
    let policies_valid = true;
    let policies_error = None;
    let mut missing_files = Vec::new();
    let mut auth_summary = std::collections::HashMap::new();

    let workspace_root = overrides
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // 1. Config syntax check
    let config = match config_res {
        Ok(cfg) => cfg,
        Err(err) => {
            config_valid = false;
            config_error = Some(err.to_string());
            crate::config::EffectiveConfig {
                workspace_root: workspace_root.clone(),
                config_path: workspace_root.join("gestalt.json"),
                defaults: crate::config::DefaultsConfig::default(),
                tools: crate::config::ToolsConfig::default(),
                context: crate::config::ContextConfig::default(),
                observe: crate::config::ObserveConfig::default(),
                providers: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(),
                prompt: crate::config::PromptConfig::default(),
                policies: crate::config::PoliciesConfig::default(),
                provider_override: None,
                model_override: None,
                tui: crate::config::TuiConfig::default(),
                extensions: Default::default(),
                skills: Default::default(),
                mcp: None,
                context_window_override: None,
            }
        }
    };

    // 2. Required files check
    if !workspace_root.join("gestalt.json").exists() {
        missing_files.push("gestalt.json".to_string());
    }

    let policy = std::sync::Arc::new(crate::run::build_policy(&config));
    let loader = gestalt_runtime::workspace_context::WorkspaceContextLoader::new(
        workspace_root.clone(),
        Some(policy as std::sync::Arc<dyn gestalt_core::policy::PolicyEngine>),
    );

    let ws_cfg = config.context.workspace.clone().unwrap_or_default();
    let ws_enabled = ws_cfg.enabled.unwrap_or(true);
    if ws_enabled {
        match loader.load_workspace_instructions(&ws_cfg).await {
            Ok(_) => {}
            Err(gestalt_runtime::workspace_context::WorkspaceContextError::RequiredMissing {
                path,
                ..
            }) => {
                let display_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                missing_files.push(display_name);
            }
            Err(err) => {
                config_valid = false;
                config_error = Some(format!("Workspace instructions error: {}", err));
            }
        }
    }

    let mem_cfg = config.context.memory.clone().unwrap_or_default();
    let mem_enabled = mem_cfg.enabled.unwrap_or(true);
    if mem_enabled {
        match loader.load_memory(&mem_cfg).await {
            Ok(_) => {}
            Err(gestalt_runtime::workspace_context::WorkspaceContextError::RequiredMissing {
                path,
                ..
            }) => {
                let display_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                missing_files.push(display_name);
            }
            Err(err) => {
                config_valid = false;
                config_error = Some(format!("Memory error: {}", err));
            }
        }
    }

    // 3. Provider auth/live checks
    let providers = crate::providers::list_providers(&config);
    for provider in &providers {
        let status = match resolve_auth(&config, provider) {
            Ok(auth_report) => {
                let mut status = auth_report.status;
                if live && status == "present" {
                    match probe_provider(&config, provider).await {
                        Ok(_) => {
                            status = "ready".to_string();
                        }
                        Err(err) => {
                            status = format!("error: {}", err);
                        }
                    }
                }
                status
            }
            Err(err) => {
                format!("error: {}", err)
            }
        };
        auth_summary.insert(provider.clone(), status);
    }

    // 4. Writability test
    let run_log_dir = config.run_log_dir();
    let run_dir_exists = run_log_dir.exists();
    let run_dir_writable = if run_dir_exists {
        if let Ok(metadata) = fs::metadata(&run_log_dir) {
            Some(!metadata.permissions().readonly())
        } else {
            Some(false)
        }
    } else {
        None
    };

    // Memory writability check
    let mut memory_writable = None;
    let mut memory_write_error = None;
    if mem_enabled
        && mem_cfg
            .write_mode
            .unwrap_or(crate::config::MemoryWriteMode::Proposal)
            == crate::config::MemoryWriteMode::Proposal
    {
        let mem_path = mem_cfg
            .path
            .clone()
            .unwrap_or_else(|| PathBuf::from(".gestalt/memory.md"));
        let resolved_mem = if mem_path.is_absolute() {
            mem_path
        } else {
            workspace_root.join(mem_path)
        };
        let path_to_check = if resolved_mem.exists() {
            resolved_mem.clone()
        } else if let Some(parent) = resolved_mem.parent() {
            parent.to_path_buf()
        } else {
            resolved_mem.clone()
        };

        if path_to_check.exists() {
            match fs::metadata(&path_to_check) {
                Ok(metadata) => {
                    let is_writable = !metadata.permissions().readonly();
                    memory_writable = Some(is_writable);
                    if !is_writable {
                        memory_write_error = Some("Destination path is read-only".to_string());
                    }
                }
                Err(err) => {
                    memory_writable = Some(false);
                    memory_write_error = Some(format!("Failed to read metadata: {}", err));
                }
            }
        } else {
            memory_writable = Some(false);
            memory_write_error = Some("Parent directory does not exist".to_string());
        }
    }

    // 5. Selected model check
    let selected_model = config.selected_model();
    let mut model_valid = true;
    let mut model_error = None;
    if let Some(ref model_id) = selected_model {
        if gestalt_runtime::ModelCatalog::new().get(model_id).is_none() {
            model_valid = false;
            model_error = Some(format!("selected model '{model_id}' is not in the catalog"));
        }
    }

    let ws_report = WorkspaceDoctorReport {
        workspace_root,
        config_valid,
        config_error,
        policies_valid,
        policies_error,
        missing_files,
        auth_summary,
        run_dir_exists,
        run_dir_writable,
        selected_model,
        model_valid,
        model_error,
        memory_writable,
        memory_write_error,
    };

    Ok(GlobalDoctorReport {
        workspace_doctor: ws_report,
        live,
    })
}
