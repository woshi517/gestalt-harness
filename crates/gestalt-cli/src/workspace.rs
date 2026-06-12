use gestalt_core::snapshot::{GitWorkspaceSnapshotter, WorkspaceSnapshotter};
use gestalt_core::{ConfigError, HarnessError};
use std::fs;
use std::path::{Path, PathBuf};

use crate::auth::resolve_auth;
use crate::config::{load_effective_config, CliOverrides};
use crate::output::{
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

pub fn status_workspace(overrides: &CliOverrides) -> Result<WorkspaceStatusReport, HarnessError> {
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
                .clone()
                .or_else(|| Some("confirm".to_string()));

            // Check files presence
            let gestalt_dir = workspace_root.join(".gestalt");
            if !workspace_root.join("gestalt.json").exists()
                && !gestalt_dir.join("config.toml").exists()
                && !gestalt_dir.join("policies.toml").exists()
            {
                warnings.push("gestalt.json is missing".to_string());
            }
            if !config.workspace_file("workspace.md").exists() {
                warnings.push("workspace.md is missing".to_string());
            }
            if !config.workspace_file("memory.md").exists() {
                warnings.push("memory.md is missing".to_string());
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

pub fn doctor_workspace(overrides: &CliOverrides) -> Result<WorkspaceDoctorReport, HarnessError> {
    let config_res = load_effective_config(overrides);
    let mut config_valid = true;
    let mut config_error = None;
    let mut policies_valid = true;
    let mut policies_error = None;
    let mut missing_files = Vec::new();
    let mut auth_summary = std::collections::HashMap::new();

    // Use derived or fallback root
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
            // Create dummy config for downstream fallback where needed
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

    // 4. Provider auth checks
    let providers = crate::providers::list_providers(&config);
    for provider in &providers {
        let status = match resolve_auth(&config, provider) {
            Ok(auth_report) => auth_report.status,
            Err(err) => format!("error: {}", err),
        };
        auth_summary.insert(provider.clone(), status);
    }

    // 5. Writability test (non-mutating/read-only)
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

    Ok(WorkspaceDoctorReport {
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
    })
}
