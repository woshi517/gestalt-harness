use gestalt_core::error::HarnessError;
use std::{fs, path::PathBuf};

use crate::auth::resolve_auth;
use crate::config::{load_effective_config, CliOverrides};
use crate::output::{GlobalDoctorReport, WorkspaceDoctorReport};
use crate::providers::probe_provider;

pub async fn diagnose_workspace(
    overrides: &CliOverrides,
    live: bool,
) -> Result<GlobalDoctorReport, HarnessError> {
    let config_res = load_effective_config(overrides);
    let mut config_valid = true;
    let mut config_error = None;
    let mut policies_valid = true;
    let mut policies_error = None;
    let mut missing_files = Vec::new();
    let mut auth_summary = std::collections::HashMap::new();

    let workspace_root = overrides
        .workspace
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let gestalt_dir = workspace_root.join(".gestalt");

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
            }
        }
    };

    // 2. Policies syntax check
    let policies_path = gestalt_dir.join("policies.toml");
    if policies_path.exists() {
        if let Err(err) = gestalt_policy::PolicyConfig::from_file(&policies_path) {
            policies_valid = false;
            policies_error = Some(err.to_string());
        }
    }

    // 3. Required files check
    if !workspace_root.join("gestalt.json").exists()
        && !gestalt_dir.join("config.toml").exists()
        && !policies_path.exists()
    {
        missing_files.push("gestalt.json".to_string());
    }
    if !config.workspace_file("workspace.md").exists() {
        missing_files.push("workspace.md".to_string());
    }
    if !config.workspace_file("memory.md").exists() {
        missing_files.push("memory.md".to_string());
    }

    // 4. Provider auth/live checks
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

    // 5. Writability test
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

    // 6. Selected model check
    let selected_model = config.selected_model();
    let mut model_valid = true;
    let mut model_error = None;
    if let Some(ref model_id) = selected_model {
        if gestalt_models::ModelCatalog::new().get(model_id).is_none() {
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
    };

    Ok(GlobalDoctorReport {
        workspace_doctor: ws_report,
        live,
    })
}
