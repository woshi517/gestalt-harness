use chrono::Utc;
use gestalt_cli::output::{
    CliReport, JsonEnvelope, WorkspaceDoctorReport, WorkspaceInfoReport, WorkspaceInitReport,
    WorkspaceSnapshotReport, WorkspaceStatusReport,
};
use gestalt_core::snapshot::WorkspaceSnapshot;
use std::collections::HashMap;
use std::path::PathBuf;

#[test]
fn test_workspace_init_report_contract() {
    let report = WorkspaceInitReport {
        workspace_root: PathBuf::from("/workspace"),
        created_files: vec![
            ".gestalt/config.toml".to_string(),
            ".gestalt/policies.toml".to_string(),
        ],
    };
    assert_eq!(report.kind(), "workspace.init");
    assert!(report
        .render_text()
        .contains("initialized workspace=/workspace"));
    assert!(report.render_text().contains("- .gestalt/config.toml"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"workspace.init""#));
}

#[test]
fn test_workspace_status_report_contract() {
    let mut auths = HashMap::new();
    auths.insert("anthropic".to_string(), "present".to_string());
    let report = WorkspaceStatusReport {
        workspace_root: PathBuf::from("/workspace"),
        config_valid: true,
        active_provider: Some("anthropic".to_string()),
        active_model: Some("claude-sonnet".to_string()),
        active_mode: Some("confirm".to_string()),
        recent_runs_count: 5,
        auth_summary: auths,
        warnings: vec!["missing openai key".to_string()],
    };
    assert_eq!(report.kind(), "workspace.status");
    assert!(report.render_text().contains("workspace_root=/workspace"));
    assert!(report.render_text().contains("active_provider=anthropic"));
    assert!(report.render_text().contains("auth.anthropic=present"));
    assert!(report.render_text().contains("missing openai key"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"workspace.status""#));
}

#[test]
fn test_workspace_info_report_contract() {
    let report = WorkspaceInfoReport {
        workspace_root: PathBuf::from("/workspace"),
        config_path: PathBuf::from("/workspace/.gestalt/config.toml"),
        policies_path: PathBuf::from("/workspace/.gestalt/policies.toml"),
        workspace_md_path: PathBuf::from("/workspace/.gestalt/workspace.md"),
        memory_md_path: PathBuf::from("/workspace/.gestalt/memory.md"),
    };
    assert_eq!(report.kind(), "workspace.info");
    assert!(report
        .render_text()
        .contains("config_path=/workspace/.gestalt/config.toml"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"workspace.info""#));
}

#[test]
fn test_workspace_snapshot_report_contract() {
    let snapshot = WorkspaceSnapshot {
        workspace_root: PathBuf::from("/workspace"),
        git_sha: Some("12345678".to_string()),
        git_dirty: Some(false),
        untracked_count: Some(2),
        content_hash: "abcdef".to_string(),
        captured_at: Utc::now(),
    };
    let report = WorkspaceSnapshotReport { snapshot };
    assert_eq!(report.kind(), "workspace.snapshot");
    assert!(report.render_text().contains("git_sha=12345678"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"workspace.snapshot""#));
}

#[test]
fn test_workspace_doctor_report_contract() {
    let mut auths = HashMap::new();
    auths.insert("openai".to_string(), "missing".to_string());
    let report = WorkspaceDoctorReport {
        workspace_root: PathBuf::from("/workspace"),
        config_valid: true,
        config_error: None,
        policies_valid: false,
        policies_error: Some("Syntax error".to_string()),
        missing_files: vec!["memory.md".to_string()],
        auth_summary: auths,
        run_dir_writable: true,
    };
    assert_eq!(report.kind(), "workspace.doctor");
    assert!(report.render_text().contains("policies_valid=false"));
    assert!(report.render_text().contains("policies_error=Syntax error"));
    assert!(report.render_text().contains("missing_files=memory.md"));

    let envelope = JsonEnvelope {
        schema_version: 1,
        kind: report.kind().to_string(),
        data: &report,
    };
    let serialized = serde_json::to_string(&envelope).unwrap();
    assert!(serialized.contains(r#""kind":"workspace.doctor""#));
}
