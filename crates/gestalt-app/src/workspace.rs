use gestalt_core::snapshot::{GitWorkspaceSnapshotter, WorkspaceSnapshotter};
use gestalt_core::{ConfigError, HarnessError};
use std::fs;
use std::path::{Path, PathBuf};

use crate::auth::resolve_auth;
use crate::config::{load_effective_config, CliOverrides};
use crate::reports::{
    WorkspaceDoctorReport, WorkspaceInfoReport, WorkspaceInitReport, WorkspaceSnapshotReport,
    WorkspaceStatusReport,
};

#[allow(dead_code)]
const DEFAULT_CONFIG: &str = r#"[defaults]
profile = "default"
mode = "confirm"
max_turns = 50

[profiles.default]
provider = "openrouter"
model = "openrouter/free"

[tools]
bash_timeout_secs = 60
max_output_tokens = 4000
sandbox_type = "none"

[context]
max_context_window = 120000
reserved_output_tokens = 8000

[observe]
run_log_dir = ".gestalt/runs"
log_format = "jsonl"
"#;

#[allow(dead_code)]
const DEFAULT_POLICIES: &str = r#"[paths]
allow_read  = [".", "sources/", "docs/", "src/"]
allow_write = ["docs/", ".gestalt/"]
deny_write  = [".git/", "secrets/", ".env", "*.key"]

[tools.bash]
default      = "confirm"
yolo_allow   = ["ls", "cat", "grep", "rg", "find"]
always_deny  = ["dd", "mkfs", "fdisk"]

[network]
default = "confirm"
"#;

const DEFAULT_WORKSPACE_MD: &str = r"# Workspace

Describe the purpose, architecture, and technology stack of this workspace here.
";

const DEFAULT_MEMORY_MD: &str = r"# Memory

## Facts

- Describe learnings, state, and facts here.
";

pub fn init_workspace(root: &Path, force: bool) -> Result<WorkspaceInitReport, HarnessError> {
    let gestalt_dir = root.join(".gestalt");
    let config_path = root.join("gestalt.json");
    let workspace_md = gestalt_dir.join("workspace.md");
    let memory_md = gestalt_dir.join("memory.md");

    // Pre-flight check
    let mut existing = Vec::new();
    if config_path.exists() {
        existing.push("gestalt.json");
    }
    if root.join(".gestalt/config.toml").exists() {
        existing.push(".gestalt/config.toml");
    }
    if root.join(".gestalt/policies.toml").exists() {
        existing.push(".gestalt/policies.toml");
    }
    if workspace_md.exists() {
        existing.push(".gestalt/workspace.md");
    }
    if memory_md.exists() {
        existing.push(".gestalt/memory.md");
    }

    if !existing.is_empty() && !force {
        return Err(HarnessError::Config(ConfigError::InvalidValue {
            field: ".gestalt".to_string(),
            reason: format!(
                "workspace files already exist: {}. Use --force to overwrite.",
                existing.join(", ")
            ),
        }));
    }

    // Scaffold
    fs::create_dir_all(&gestalt_dir).map_err(|err| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: ".gestalt".to_string(),
            reason: err.to_string(),
        })
    })?;

    crate::config::write_workspace_config_file(
        &config_path,
        &crate::config::default_workspace_config(),
    )?;

    fs::write(&workspace_md, DEFAULT_WORKSPACE_MD).map_err(|err| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: ".gestalt/workspace.md".to_string(),
            reason: err.to_string(),
        })
    })?;

    fs::write(&memory_md, DEFAULT_MEMORY_MD).map_err(|err| {
        HarnessError::Config(ConfigError::InvalidValue {
            field: ".gestalt/memory.md".to_string(),
            reason: err.to_string(),
        })
    })?;

    Ok(WorkspaceInitReport {
        workspace_root: root.to_path_buf(),
        created_files: vec![
            "gestalt.json".to_string(),
            ".gestalt/workspace.md".to_string(),
            ".gestalt/memory.md".to_string(),
        ],
    })
}

pub async fn status_workspace(
    overrides: &CliOverrides,
) -> Result<WorkspaceStatusReport, HarnessError> {
    let config_res = load_effective_config(overrides);
    let mut warnings = Vec::new();
    let mut auth_summary = std::collections::HashMap::new();

    match config_res {
        Ok(config) => {
            let workspace_root = config.workspace_root.clone();
            let active_provider = config.selected_provider().ok();
            let active_model = config.selected_model();
            let active_mode = config
                .defaults
                .mode
                .map(|m| match m {
                    gestalt_core::ExecutionMode::Confirm => "confirm".to_string(),
                    gestalt_core::ExecutionMode::Yolo => "yolo".to_string(),
                    gestalt_core::ExecutionMode::Human => "human".to_string(),
                    gestalt_core::ExecutionMode::DryRun => "dry_run".to_string(),
                    gestalt_core::ExecutionMode::Replay => "replay".to_string(),
                })
                .or_else(|| Some("confirm".to_string()));

            // Check files presence
            let gestalt_dir = workspace_root.join(".gestalt");
            if !workspace_root.join("gestalt.json").exists()
                && !gestalt_dir.join("config.toml").exists()
                && !gestalt_dir.join("policies.toml").exists()
            {
                warnings.push("gestalt.json is missing".to_string());
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
                    Err(gestalt_runtime::workspace_context::WorkspaceContextError::RequiredMissing { path, .. }) => {
                        warnings.push(format!("workspace instructions file '{}' is missing", path.display()));
                    }
                    Err(err) => {
                        warnings.push(format!("workspace instructions file error: {}", err));
                    }
                }
            }

            let mem_cfg = config.context.memory.clone().unwrap_or_default();
            let mem_enabled = mem_cfg.enabled.unwrap_or(true);
            if mem_enabled {
                match loader.load_memory(&mem_cfg).await {
                    Ok(_) => {}
                    Err(gestalt_runtime::workspace_context::WorkspaceContextError::RequiredMissing { path, .. }) => {
                        warnings.push(format!("memory file '{}' is missing", path.display()));
                    }
                    Err(err) => {
                        warnings.push(format!("memory file error: {}", err));
                    }
                }
            }

            if config.context.workspace_file.is_some() {
                warnings.push("context.workspace_file is deprecated. Please migrate to context.workspace.path".to_string());
            }
            if config.context.memory_file.is_some() {
                warnings.push(
                    "context.memory_file is deprecated. Please migrate to context.memory.path"
                        .to_string(),
                );
            }

            // Count runs
            let run_log_dir = config.run_log_dir();
            let mut recent_runs_count = 0;
            if let Ok(entries) = fs::read_dir(&run_log_dir) {
                for entry in entries.flatten() {
                    if entry.path().join("trace.jsonl").exists() {
                        recent_runs_count += 1;
                    }
                }
            }

            // Auth summary
            let providers = crate::providers::list_providers(&config);
            for provider in &providers {
                if let Ok(auth_report) = resolve_auth(&config, provider) {
                    auth_summary.insert(provider.clone(), auth_report.status.clone());
                    if auth_report.status == "missing" {
                        if let Some(ref active_p) = active_provider {
                            if active_p == provider {
                                warnings.push(format!(
                                    "API key for active provider '{}' is missing from env ({})",
                                    provider, auth_report.variable
                                ));
                            }
                        }
                    }
                }
            }

            Ok(WorkspaceStatusReport {
                workspace_root,
                config_valid: true,
                active_provider,
                active_model,
                active_mode,
                recent_runs_count,
                auth_summary,
                warnings,
            })
        }
        Err(err) => {
            // Load root path as best fallback
            let workspace_root = overrides
                .workspace
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            warnings.push(format!("Config is invalid: {err}"));
            Ok(WorkspaceStatusReport {
                workspace_root,
                config_valid: false,
                active_provider: None,
                active_model: None,
                active_mode: None,
                recent_runs_count: 0,
                auth_summary,
                warnings,
            })
        }
    }
}

pub fn info_workspace(overrides: &CliOverrides) -> Result<WorkspaceInfoReport, HarnessError> {
    let config = load_effective_config(overrides)?;
    let workspace_root = config.workspace_root.clone();
    Ok(WorkspaceInfoReport {
        workspace_root,
        config_path: config.config_file(),
        workspace_md_path: config.workspace_markdown_path(),
        memory_md_path: config.memory_markdown_path(),
    })
}

pub async fn snapshot_workspace(
    overrides: &CliOverrides,
) -> Result<WorkspaceSnapshotReport, HarnessError> {
    let config = load_effective_config(overrides)?;
    let snapshotter = GitWorkspaceSnapshotter;
    let snapshot = snapshotter.capture(&config.workspace_root).await?;
    Ok(WorkspaceSnapshotReport { snapshot })
}

pub async fn doctor_workspace(
    overrides: &CliOverrides,
) -> Result<WorkspaceDoctorReport, HarnessError> {
    let global_report = crate::doctor::diagnose_workspace(overrides, false).await?;
    Ok(global_report.workspace_doctor)
}
