use gestalt_app::config::CliOverrides;
use gestalt_app::workspace::{
    doctor_workspace, info_workspace, init_workspace, snapshot_workspace, status_workspace,
};
use gestalt_cli::policy::validate_policy;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

struct EnvVarGuard {
    key: &'static str,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        let original = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(ref val) = self.original {
            std::env::set_var(self.key, val);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn create_temp_workspace() -> PathBuf {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let temp =
        std::env::temp_dir().join(format!("gestalt-test-workspace-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[test]
fn test_init_workspace_scaffolding() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let temp_root = create_temp_workspace();

    // 1. Initial run succeeds
    let report = init_workspace(&temp_root, false).unwrap();
    assert_eq!(report.workspace_root, temp_root);
    assert_eq!(report.created_files.len(), 3);

    let gestalt_dir = temp_root.join(".gestalt");
    assert!(temp_root.join("gestalt.json").exists());
    assert!(gestalt_dir.join("workspace.md").exists());
    assert!(gestalt_dir.join("memory.md").exists());

    let config_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(temp_root.join("gestalt.json")).unwrap()).unwrap();
    assert_eq!(
        config_json["policies"]["paths"]["allow_write"],
        serde_json::json!(["docs/", ".gestalt/"])
    );

    // 2. Second run without force fails
    let err_res = init_workspace(&temp_root, false);
    assert!(err_res.is_err());
    let err_msg = err_res.err().unwrap().to_string();
    assert!(err_msg.contains("workspace files already exist"));

    // 3. Second run with force succeeds
    let report_force = init_workspace(&temp_root, true).unwrap();
    assert_eq!(report_force.created_files.len(), 3);

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_status_workspace() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let report = status_workspace(&overrides).await.unwrap();
    assert!(report.config_valid);
    assert_eq!(report.active_provider.as_deref(), Some("openrouter"));
    assert_eq!(report.active_model.as_deref(), Some("openrouter/free"));
    assert_eq!(report.active_mode.as_deref(), Some("confirm"));
    assert_eq!(report.recent_runs_count, 0);

    // Mock a run by creating trace.jsonl
    let runs_dir = temp_root.join(".gestalt/runs/run-1");
    fs::create_dir_all(&runs_dir).unwrap();
    fs::write(runs_dir.join("trace.jsonl"), "{}").unwrap();

    let report2 = status_workspace(&overrides).await.unwrap();
    assert_eq!(report2.recent_runs_count, 1);

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_info_workspace() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let report = info_workspace(&overrides).unwrap();
    assert_eq!(report.workspace_root, temp_root);
    assert_eq!(report.config_path, temp_root.join("gestalt.json"));
    assert_eq!(
        report.workspace_md_path,
        temp_root.join(".gestalt/workspace.md")
    );
    assert_eq!(report.memory_md_path, temp_root.join(".gestalt/memory.md"));

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_global_only_config_reports_global_json_path() {
    let _guard = ENV_MUTEX.lock().unwrap();

    let temp_root = create_temp_workspace();
    let global_dir = temp_root.join("global");
    fs::create_dir_all(&global_dir).unwrap();
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &global_dir);

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let info = info_workspace(&overrides).unwrap();
    let global_config_path = global_dir.join("gestalt/gestalt.json");
    assert_eq!(info.config_path, global_config_path);

    let policy_report = validate_policy(&overrides).unwrap();
    assert_eq!(policy_report.path, global_config_path);
    assert!(policy_report.valid);

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_legacy_workspace_reports_legacy_config_path() {
    let _guard = ENV_MUTEX.lock().unwrap();

    let temp_root = create_temp_workspace();
    let gestalt_dir = temp_root.join(".gestalt");
    fs::create_dir_all(&gestalt_dir).unwrap();
    fs::write(
        gestalt_dir.join("config.toml"),
        r#"
[defaults]
profile = "openai"
"#,
    )
    .unwrap();

    let global_dir = temp_root.join("global");
    fs::create_dir_all(&global_dir).unwrap();
    let _xdg_guard = EnvVarGuard::set("XDG_CONFIG_HOME", &global_dir);

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let info = info_workspace(&overrides).unwrap();
    assert_eq!(info.config_path, gestalt_dir.join("config.toml"));

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_snapshot_workspace() {
    let _guard = ENV_MUTEX.lock().unwrap();
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

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_doctor_workspace() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    // Prove doctor_workspace does not create .gestalt/runs/ when absent
    let report = doctor_workspace(&overrides).await.unwrap();
    assert!(report.config_valid);
    assert!(report.policies_valid);
    assert!(report.missing_files.is_empty());
    assert!(!report.run_dir_exists);
    assert_eq!(report.run_dir_writable, None);
    assert!(!temp_root.join(".gestalt/runs").exists());

    // Creating .gestalt/runs should update status to exists/writable
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();
    let report_with_runs = doctor_workspace(&overrides).await.unwrap();
    assert!(report_with_runs.run_dir_exists);
    assert_eq!(report_with_runs.run_dir_writable, Some(true));

    // Test with missing memory.md
    let gestalt_json_path = temp_root.join("gestalt.json");
    let mut config_val: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&gestalt_json_path).unwrap()).unwrap();
    config_val["context"]["memory"] = serde_json::json!({
        "required": true
    });
    fs::write(
        &gestalt_json_path,
        serde_json::to_string_pretty(&config_val).unwrap(),
    )
    .unwrap();

    fs::remove_file(temp_root.join(".gestalt/memory.md")).unwrap();
    let report_missing = doctor_workspace(&overrides).await.unwrap();
    assert_eq!(report_missing.missing_files, vec!["memory.md".to_string()]);

    // Test with malformed policies.toml
    fs::write(temp_root.join(".gestalt/policies.toml"), "invalid = [toml").unwrap();
    let report_malformed = doctor_workspace(&overrides).await.unwrap();
    assert!(!report_malformed.policies_valid);
    assert!(report_malformed.policies_error.is_some());

    // Test with malformed gestalt.json (invalid-config branch in status and doctor)
    fs::write(temp_root.join("gestalt.json"), "invalid = [json").unwrap();
    let report_invalid_config = doctor_workspace(&overrides).await.unwrap();
    assert!(!report_invalid_config.config_valid);
    assert!(report_invalid_config.config_error.is_some());

    let status_invalid_config = status_workspace(&overrides).await.unwrap();
    assert!(!status_invalid_config.config_valid);
    assert!(!status_invalid_config.warnings.is_empty());

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_enhanced_cli_operations() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let temp_root = create_temp_workspace();
    init_workspace(&temp_root, false).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let config = gestalt_app::config::load_effective_config(&overrides).unwrap();

    // U5.4.1 tools list
    let list_report = gestalt_cli::tools::list_tools(&overrides).unwrap();
    assert!(!list_report.tools.is_empty());
    assert!(list_report.tools.iter().any(|t| t.name == "read"));

    // U5.4.2 tools inspect
    let inspect_report = gestalt_cli::tools::inspect_tool(&overrides, "read").unwrap();
    assert_eq!(inspect_report.name, "read");
    assert!(inspect_report.schema.get("name").is_some());

    // U5.4.3 tools classify bash
    let classify_report = gestalt_cli::tools::classify_bash(
        &overrides,
        &["rm".to_string(), "-rf".to_string(), "/".to_string()],
    )
    .unwrap();
    assert_eq!(classify_report.command, "rm -rf /");
    assert_eq!(
        classify_report.risk,
        gestalt_core::tool::RiskLevel::Critical
    );

    // U6.1 auth doctor
    let auth_report = gestalt_app::auth::auth_doctor(&config).unwrap();
    assert!(auth_report
        .entries
        .iter()
        .any(|e| e.variable == "ANTHROPIC_API_KEY"));

    // U6.4 Global Doctor (diagnose_workspace)
    let diag = gestalt_app::doctor::diagnose_workspace(&overrides, false)
        .await
        .unwrap();
    assert!(diag.workspace_doctor.config_valid);
    assert!(diag.workspace_doctor.policies_valid);
    assert!(!diag.live);

    let _ = fs::remove_dir_all(&temp_root);
}
