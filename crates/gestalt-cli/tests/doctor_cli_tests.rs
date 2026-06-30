#![cfg(feature = "full")]

use gestalt_app::config::CliOverrides;
use gestalt_app::doctor::diagnose_workspace;
use gestalt_cli::output::CliReport;
use std::fs;
use std::path::PathBuf;

fn create_temp_workspace() -> PathBuf {
    let temp = std::env::temp_dir().join(format!("gestalt-test-doctor-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[tokio::test]
async fn test_doctor_diagnostics() {
    std::env::remove_var("GESTALT_PROFILE");
    std::env::remove_var("GESTALT_PROVIDER");
    std::env::remove_var("GESTALT_MODEL");
    std::env::remove_var("GESTALT_MODE");
    std::env::remove_var("GESTALT_MAX_TURNS");
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");

    let temp_root = create_temp_workspace();
    let gestalt_dir = temp_root.join(".gestalt");
    fs::create_dir_all(&gestalt_dir).unwrap();

    fs::write(
        temp_root.join("gestalt.json"),
        r#"{
          "version":1,
          "defaults":{"provider":"custom-prov","model":"custom-prov/non-existent-model"},
          "providers":{"custom-prov":{"id":"custom-prov","display_name":"Custom Provider","protocol":"openai","base_url":"https://api.custom.com/v1","api_key_env":"CUSTOM_API_KEY"}}
        }"#,
    )
    .unwrap();
    fs::write(gestalt_dir.join("workspace.md"), "# Workspace\n").unwrap();
    fs::write(gestalt_dir.join("memory.md"), "# Memory\n").unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    // Run workspace diagnosis
    let report = diagnose_workspace(&overrides, false).await.unwrap();

    // 1. Check selected model validation
    assert_eq!(
        report.workspace_doctor.selected_model.as_deref(),
        Some("custom-prov/non-existent-model")
    );
    assert!(!report.workspace_doctor.model_valid);
    assert!(report
        .workspace_doctor
        .model_error
        .as_deref()
        .unwrap()
        .contains("not in the catalog"));

    // 2. Check custom provider is listed
    assert!(report
        .workspace_doctor
        .auth_summary
        .contains_key("custom-prov"));

    // 3. Render text output and check contents
    let text = report.workspace_doctor.render_text();
    assert!(text.contains("selected_model=custom-prov/non-existent-model"));
    assert!(text.contains("model_valid=false"));

    let global_text = report.render_text();
    assert!(global_text.contains("selected model 'custom-prov/non-existent-model'"));

    let _ = fs::remove_dir_all(&temp_root);
}
