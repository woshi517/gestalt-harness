use std::fs;
use std::path::PathBuf;
use gestalt_cli::config::{CliOverrides, load_effective_config};
use gestalt_cli::verify::verify_run;
use gestalt_core::event::VerificationStatus;

fn create_temp_workspace() -> PathBuf {
    let temp = std::env::temp_dir().join(format!("gestalt-test-verify-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[tokio::test]
async fn test_verify_run_artifacts() {
    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run_dir = runs_dir.join("20260603T130000Z-session-verify");
    let artifacts_dir = run_dir.join("artifacts");
    fs::create_dir_all(&artifacts_dir).unwrap();

    // 1. Write clean artifacts (no secrets, good markdown format)
    fs::write(artifacts_dir.join("clean.md"), "# Title\nThis is clean content without secrets.\n").unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    let clean_res = verify_run(&config, "20260603T130000Z-session-verify").await.unwrap();
    assert_eq!(clean_res.status, VerificationStatus::Passed);
    assert_eq!(clean_res.total_failed, 0);

    // 2. Write an artifact containing a secret key (should fail verification)
    fs::write(artifacts_dir.join("leaked.txt"), "AWS_SECRET_ACCESS_KEY=abcd1234\n").unwrap();
    let dirty_res = verify_run(&config, "20260603T130000Z-session-verify").await.unwrap();
    assert_eq!(dirty_res.status, VerificationStatus::Failed);
    assert!(dirty_res.total_failed > 0);

    let _ = fs::remove_dir_all(&temp_root);
}
