use gestalt_cli::config::CliOverrides;
use gestalt_cli::workspace::{
    doctor_workspace, info_workspace, init_workspace, snapshot_workspace, status_workspace,
};
use std::fs;
use std::path::PathBuf;

fn create_temp_workspace() -> PathBuf {
    let temp =
        std::env::temp_dir().join(format!("gestalt-test-workspace-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[test]
fn test_init_workspace_scaffolding() {
    let temp_root = create_temp_workspace();

    // 1. Initial run succeeds
    let report = init_workspace(&temp_root, false).unwrap();
    assert_eq!(report.workspace_root, temp_root);
    assert_eq!(report.created_files.len(), 4);

    let gestalt_dir = temp_root.join(".gestalt");
    assert!(gestalt_dir.join("config.toml").exists());
    assert!(gestalt_dir.join("policies.toml").exists());
    assert!(gestalt_dir.join("workspace.md").exists());
    assert!(gestalt_dir.join("memory.md").exists());

    // 2. Second run without force fails
    let err_res = init_workspace(&temp_root, false);
    assert!(err_res.is_err());
    let err_msg = err_res.err().unwrap().to_string();
    assert!(err_msg.contains("workspace files already exist"));

    // 3. Second run with force succeeds
    let report_force = init_workspace(&temp_root, true).unwrap();
    assert_eq!(report_force.created_files.len(), 4);

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_status_workspace() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let report = status_workspace(&overrides).unwrap();
    assert!(report.config_valid);
    assert_eq!(report.active_provider.as_deref(), Some("anthropic"));
    assert_eq!(
        report.active_model.as_deref(),
        Some("claude-3-5-sonnet-20241022")
    );
    assert_eq!(report.active_mode.as_deref(), Some("confirm"));
    assert_eq!(report.recent_runs_count, 0);

    // Mock a run by creating trace.jsonl
    let runs_dir = temp_root.join(".gestalt/runs/run-1");
    fs::create_dir_all(&runs_dir).unwrap();
    fs::write(runs_dir.join("trace.jsonl"), "{}").unwrap();

    let report2 = status_workspace(&overrides).unwrap();
    assert_eq!(report2.recent_runs_count, 1);

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_info_workspace() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let report = info_workspace(&overrides).unwrap();
    assert_eq!(report.workspace_root, temp_root);
    assert_eq!(report.config_path, temp_root.join(".gestalt/config.toml"));
    assert_eq!(
        report.policies_path,
        temp_root.join(".gestalt/policies.toml")
    );
    assert_eq!(
        report.workspace_md_path,
        temp_root.join(".gestalt/workspace.md")
    );
    assert_eq!(report.memory_md_path, temp_root.join(".gestalt/memory.md"));

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_snapshot_workspace() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let report = snapshot_workspace(&overrides).await.unwrap();
    assert_eq!(report.snapshot.workspace_root, temp_root);
    // Since temp_root is not a git repo, git metadata should be empty/None
    assert!(report.snapshot.git_sha.is_none());

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_doctor_workspace() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    // Prove doctor_workspace does not create .gestalt/runs/ when absent
    let report = doctor_workspace(&overrides).unwrap();
    assert!(report.config_valid);
    assert!(report.policies_valid);
    assert!(report.missing_files.is_empty());
    assert!(!report.run_dir_exists);
    assert_eq!(report.run_dir_writable, None);
    assert!(!temp_root.join(".gestalt/runs").exists());

    // Creating .gestalt/runs should update status to exists/writable
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();
    let report_with_runs = doctor_workspace(&overrides).unwrap();
    assert!(report_with_runs.run_dir_exists);
    assert_eq!(report_with_runs.run_dir_writable, Some(true));

    // Test with missing memory.md
    fs::remove_file(temp_root.join(".gestalt/memory.md")).unwrap();
    let report_missing = doctor_workspace(&overrides).unwrap();
    assert_eq!(report_missing.missing_files, vec!["memory.md".to_string()]);

    // Test with malformed policies.toml
    fs::write(temp_root.join(".gestalt/policies.toml"), "invalid = [toml").unwrap();
    let report_malformed = doctor_workspace(&overrides).unwrap();
    assert!(!report_malformed.policies_valid);
    assert!(report_malformed.policies_error.is_some());

    // Test with malformed config.toml (invalid-config branch in status and doctor)
    fs::write(temp_root.join(".gestalt/config.toml"), "invalid = [toml").unwrap();
    let report_invalid_config = doctor_workspace(&overrides).unwrap();
    assert!(!report_invalid_config.config_valid);
    assert!(report_invalid_config.config_error.is_some());

    let status_invalid_config = status_workspace(&overrides).unwrap();
    assert!(!status_invalid_config.config_valid);
    assert!(!status_invalid_config.warnings.is_empty());

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_enhanced_cli_operations() {
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let config = gestalt_cli::config::load_effective_config(&overrides).unwrap();

    // U5.4.1 tools list
    let list_report = gestalt_cli::tools::list_tools(&overrides).unwrap();
    assert!(!list_report.tools.is_empty());
    assert!(list_report.tools.iter().any(|t| t.name == "read"));

    // U5.4.2 tools inspect
    let inspect_report = gestalt_cli::tools::inspect_tool(&overrides, "read").unwrap();
    assert_eq!(inspect_report.name, "read");
    assert!(inspect_report.schema.get("name").is_some());

    // U5.4.3 tools classify bash
    let classify_report = gestalt_cli::tools::classify_bash(&overrides, &["rm".to_string(), "-rf".to_string(), "/".to_string()]).unwrap();
    assert_eq!(classify_report.command, "rm -rf /");
    assert_eq!(classify_report.risk, gestalt_core::tool::RiskLevel::Critical);

    // U6.1 auth doctor
    let auth_report = gestalt_cli::auth::auth_doctor(&config).unwrap();
    assert!(auth_report.entries.iter().any(|e| e.variable == "ANTHROPIC_API_KEY"));

    // U6.4 Global Doctor (diagnose_workspace)
    let diag = gestalt_cli::doctor::diagnose_workspace(&overrides, false).await.unwrap();
    assert!(diag.workspace_doctor.config_valid);
    assert!(diag.workspace_doctor.policies_valid);
    assert!(!diag.live);

    let _ = fs::remove_dir_all(&temp_root);
}

