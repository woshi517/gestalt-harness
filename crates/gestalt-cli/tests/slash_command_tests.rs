use gestalt_app::config::{load_effective_config, CliOverrides};
use gestalt_cli::slash::{calculate_session_cost, handle_slash_command, SlashOutcome};
use gestalt_trace::run_manifest::{CompatibilityFingerprint, LifecycleState, RunKind, RunManifest};
use std::fs;
use std::path::PathBuf;

fn create_temp_workspace() -> PathBuf {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let temp = std::env::temp_dir().join(format!("gestalt-test-slash-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

fn copy_minimal_workspace(dest: &std::path::Path) {
    let src = std::path::Path::new("tests/fixtures/workspaces/minimal");
    let src_gestalt = src.join(".gestalt");
    let dest_gestalt = dest.join(".gestalt");
    fs::create_dir_all(&dest_gestalt).unwrap();

    if src_gestalt.exists() {
        for entry in fs::read_dir(&src_gestalt).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            fs::copy(src_gestalt.join(&name), dest_gestalt.join(&name)).unwrap();
        }
    } else {
        // Fallback for scaffold if running in wrong dir
        fs::write(
            dest_gestalt.join("config.toml"),
            "[defaults]\nprovider = \"mock\"\n",
        )
        .unwrap();
        fs::write(dest_gestalt.join("policies.toml"), "[policies]\n").unwrap();
    }
}

#[tokio::test]
async fn test_slash_quit_and_exit() {
    let temp_root = create_temp_workspace();
    copy_minimal_workspace(&temp_root);
    let mut overrides = CliOverrides {
        workspace: Some(temp_root),
        ..Default::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    let res = handle_slash_command("/quit", "session-1", None, &mut overrides, &config)
        .await
        .unwrap();
    assert!(matches!(res, SlashOutcome::Quit));

    let res = handle_slash_command("/exit", "session-1", None, &mut overrides, &config)
        .await
        .unwrap();
    assert!(matches!(res, SlashOutcome::Quit));
}

#[tokio::test]
async fn test_slash_mode_change() {
    let temp_root = create_temp_workspace();
    copy_minimal_workspace(&temp_root);
    let mut overrides = CliOverrides {
        workspace: Some(temp_root),
        mode: Some("confirm".to_string()),
        ..Default::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    let res = handle_slash_command("/mode yolo", "session-1", None, &mut overrides, &config)
        .await
        .unwrap();
    if let SlashOutcome::ChangeMode(mode) = res {
        assert_eq!(mode, "yolo");
    } else {
        panic!("expected ChangeMode");
    }
    assert_eq!(overrides.mode.as_deref(), Some("yolo"));
}

#[tokio::test]
async fn test_slash_cost_calculation() {
    let temp_root = create_temp_workspace();
    copy_minimal_workspace(&temp_root);

    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let session_id = "session-cost-test";

    // Create 2 mock run directories with cost reports
    let run1_dir = runs_dir.join("run1");
    fs::create_dir_all(&run1_dir).unwrap();
    let fp = CompatibilityFingerprint {
        context_pipeline_version: "v1".to_string(),
        tool_schema_hash: "hash".to_string(),
        policy_fingerprint: "policy".to_string(),
        hook_contract_hash: "hook".to_string(),
        execution_mode: "Yolo".to_string(),
        skill_fingerprint: None,
        workspace_context_snapshot_hash: None,
    };
    let m1 = RunManifest {
        v: 1,
        session_id: session_id.to_string(),
        run_id: "run1".to_string(),
        parent_run_id: None,
        base_checkpoint: None,
        run_kind: RunKind::New,
        created_at: chrono::Utc::now(),
        lifecycle_state: LifecycleState::Completed,
        finalized_at: Some(chrono::Utc::now()),
        failure_kind: None,
        interrupted_phase: None,
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        resolved_model: None,
        compatibility_fingerprint: fp.clone(),
    };
    m1.save_to(&run1_dir.join("run.json")).unwrap();
    fs::write(run1_dir.join("cost.json"), r#"{"runs":1,"input_tokens":100,"output_tokens":50,"estimated_cost_usd":0.0015,"warnings":[]}"#).unwrap();

    let run2_dir = runs_dir.join("run2");
    fs::create_dir_all(&run2_dir).unwrap();
    let m2 = RunManifest {
        v: 1,
        session_id: session_id.to_string(),
        run_id: "run2".to_string(),
        parent_run_id: Some("run1".to_string()),
        base_checkpoint: Some(1),
        run_kind: RunKind::Continue,
        created_at: chrono::Utc::now(),
        lifecycle_state: LifecycleState::Completed,
        finalized_at: Some(chrono::Utc::now()),
        failure_kind: None,
        interrupted_phase: None,
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        resolved_model: None,
        compatibility_fingerprint: fp,
    };
    m2.save_to(&run2_dir.join("run.json")).unwrap();
    fs::write(run2_dir.join("cost.json"), r#"{"runs":1,"input_tokens":200,"output_tokens":100,"estimated_cost_usd":0.0035,"warnings":[]}"#).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root),
        ..Default::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    let total = calculate_session_cost(&config, session_id);
    assert!((total - 0.0050).abs() < 1e-6);
}
