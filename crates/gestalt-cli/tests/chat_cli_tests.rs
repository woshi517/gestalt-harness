#![allow(clippy::large_futures)]

use gestalt_cli::chat::run_chat;
use gestalt_cli::config::CliOverrides;
use gestalt_core::HarnessError;
use gestalt_trace::run_manifest::{CompatibilityFingerprint, LifecycleState, RunKind, RunManifest};
use std::fs;
use std::path::PathBuf;

fn create_temp_workspace() -> PathBuf {
    let temp = std::env::temp_dir().join(format!("gestalt-test-chat-{}", uuid::Uuid::new_v4()));
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
        let config_toml = r#"
[defaults]
provider = "dummy-openai"
mode = "yolo"
max_turns = 1

[providers.dummy-openai]
kind = "openai-compatible"
default_model = "mock-model"
base_url = "http://127.0.0.1:9999/v1"
"#;
        fs::write(dest_gestalt.join("config.toml"), config_toml).unwrap();
        fs::write(dest_gestalt.join("policies.toml"), "[policies]\n").unwrap();
    }
}

#[tokio::test]
async fn test_chat_exits_on_cancelled_token() {
    let temp_root = create_temp_workspace();
    copy_minimal_workspace(&temp_root);
    let overrides = CliOverrides {
        workspace: Some(temp_root),
        ..Default::default()
    };
    let cancel_token = gestalt_core::cancel::CancelToken::new();
    cancel_token.cancel(); // cancel it before starting

    let res = run_chat(&overrides, None, None, cancel_token).await;
    res.unwrap();
}

#[tokio::test]
async fn test_chat_rejects_resume_unsafe_unfinalized() {
    let temp_root = create_temp_workspace();
    copy_minimal_workspace(&temp_root);

    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run_id = "run-unsafe-123";
    let run_dir = runs_dir.join(format!("20260602T100000Z-{}", run_id));
    fs::create_dir_all(&run_dir).unwrap();

    // Create an unfinalized manifest (LifecycleState::Running)
    let fingerprint = CompatibilityFingerprint {
        context_pipeline_version: "pipeline-v1".to_string(),
        tool_schema_hash: "hash".to_string(),
        policy_fingerprint: "policy".to_string(),
        hook_contract_hash: "hook".to_string(),
        execution_mode: "Yolo".to_string(),
        skill_fingerprint: None,
        workspace_context_snapshot_hash: None,
    };
    let manifest = RunManifest {
        v: 1,
        session_id: "session-unsafe".to_string(),
        run_id: run_id.to_string(),
        parent_run_id: None,
        base_checkpoint: None,
        run_kind: RunKind::New,
        created_at: chrono::Utc::now(),
        lifecycle_state: LifecycleState::Running, // Unfinalized!
        finalized_at: None,
        failure_kind: None,
        interrupted_phase: None,
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        compatibility_fingerprint: fingerprint,
    };
    manifest.save_to(&run_dir.join("run.json")).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root),
        mode: Some("yolo".to_string()),
        ..Default::default()
    };
    let cancel_token = gestalt_core::cancel::CancelToken::new();

    let res = run_chat(&overrides, Some(run_id.to_string()), None, cancel_token).await;
    assert!(res.is_err());
    if let Err(HarnessError::Policy(gestalt_core::PolicyError::Denied(ref reason))) = res {
        assert!(reason.contains("Resume rejected"));
    } else {
        panic!("expected Policy denied error due to unfinalized run status");
    }
}

#[test]
fn test_interactive_chat_lineage_and_session_id() {
    let temp_root = create_temp_workspace();
    copy_minimal_workspace(&temp_root);

    use std::io::Write as _;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .arg("--workspace")
        .arg(&temp_root)
        .arg("chat")
        .arg("--yes")
        .env("ANTHROPIC_API_KEY", "dummy-key")
        .env("OPENAI_COMPATIBLE_API_KEY", "dummy-key")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "first prompt").unwrap();
    writeln!(stdin, "second prompt").unwrap();
    writeln!(stdin, "/quit").unwrap();
    drop(stdin);

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());

    let runs_dir = temp_root.join(".gestalt/runs");
    assert!(runs_dir.exists());

    let mut run_manifests = Vec::new();
    for entry in std::fs::read_dir(runs_dir).unwrap().flatten() {
        if entry.path().is_dir() {
            let manifest_path = entry.path().join("run.json");
            if manifest_path.exists() {
                let manifest = RunManifest::load_from(&manifest_path).unwrap();
                run_manifests.push(manifest);
            }
        }
    }

    assert_eq!(run_manifests.len(), 2);
    run_manifests.sort_by_key(|m| m.created_at);

    let parent = &run_manifests[0];
    let child = &run_manifests[1];

    assert_eq!(parent.session_id, child.session_id);
    assert_eq!(child.parent_run_id.as_ref(), Some(&parent.run_id));
}

#[test]
fn test_cli_run_resume() {
    let temp_root = create_temp_workspace();
    copy_minimal_workspace(&temp_root);

    let _output1 = std::process::Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .arg("--workspace")
        .arg(&temp_root)
        .arg("run")
        .arg("first prompt")
        .arg("--yes")
        .env("ANTHROPIC_API_KEY", "dummy-key")
        .env("OPENAI_COMPATIBLE_API_KEY", "dummy-key")
        .output()
        .unwrap();

    let runs_dir = temp_root.join(".gestalt/runs");
    let mut run_manifests = Vec::new();
    for entry in std::fs::read_dir(&runs_dir).unwrap().flatten() {
        if entry.path().is_dir() {
            let manifest_path = entry.path().join("run.json");
            if manifest_path.exists() {
                let manifest = RunManifest::load_from(&manifest_path).unwrap();
                run_manifests.push(manifest);
            }
        }
    }
    assert_eq!(run_manifests.len(), 1);
    let parent_run_id = run_manifests[0].run_id.clone();
    let session_id = run_manifests[0].session_id.clone();

    let _output2 = std::process::Command::new(env!("CARGO_BIN_EXE_gestalt"))
        .arg("--workspace")
        .arg(&temp_root)
        .arg("run")
        .arg("second prompt")
        .arg("--resume")
        .arg(&parent_run_id)
        .arg("--yes")
        .env("ANTHROPIC_API_KEY", "dummy-key")
        .env("OPENAI_COMPATIBLE_API_KEY", "dummy-key")
        .output()
        .unwrap();

    let mut updated_manifests = Vec::new();
    for entry in std::fs::read_dir(&runs_dir).unwrap().flatten() {
        if entry.path().is_dir() {
            let manifest_path = entry.path().join("run.json");
            if manifest_path.exists() {
                let manifest = RunManifest::load_from(&manifest_path).unwrap();
                updated_manifests.push(manifest);
            }
        }
    }
    assert_eq!(updated_manifests.len(), 2);

    let child_manifest = updated_manifests
        .iter()
        .find(|m| m.run_id != parent_run_id)
        .unwrap();

    assert_eq!(child_manifest.session_id, session_id);
    assert_eq!(child_manifest.parent_run_id.as_ref(), Some(&parent_run_id));
}
