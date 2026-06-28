use gestalt_app::config::{load_effective_config, CliOverrides};
use gestalt_cli::export::export_run;
use gestalt_cli::output::ExportFormat;
use std::fs;
use std::path::PathBuf;

fn create_temp_workspace() -> PathBuf {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let temp = std::env::temp_dir().join(format!("gestalt-test-export-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[test]
fn test_export_formats() {
    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run_dir = runs_dir.join("20260603T120000Z-session-export");
    fs::create_dir_all(&run_dir).unwrap();

    let trace_content = r#"{"v":1,"session_id":"session-export","turn_id":1,"seq":1,"ts":"2026-06-03T12:00:00Z","event":{"type":"user_message","content":"hello export"},"redacted":false}
{"v":1,"session_id":"session-export","turn_id":1,"seq":2,"ts":"2026-06-03T12:00:01Z","event":{"type":"stop","reason":"end_turn"},"redacted":false}"#;
    fs::write(run_dir.join("trace.jsonl"), trace_content).unwrap();

    // Summary configuration
    fs::write(run_dir.join("summary.md"), "# Run Summary\n").unwrap();
    fs::write(
        run_dir.join("cost.json"),
        r#"{"runs":1,"input_tokens":0,"output_tokens":0,"estimated_cost_usd":0.0,"warnings":[]}"#,
    )
    .unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    // 1. Export JSONL
    let jsonl_rep = export_run(
        &config,
        "20260603T120000Z-session-export",
        ExportFormat::Jsonl,
    )
    .unwrap();
    assert_eq!(jsonl_rep.format, "jsonl");
    assert_eq!(jsonl_rep.content.trim(), trace_content.trim());

    // 2. Export Markdown
    let md_rep = export_run(
        &config,
        "20260603T120000Z-session-export",
        ExportFormat::Markdown,
    )
    .unwrap();
    assert_eq!(md_rep.format, "markdown");
    assert!(md_rep
        .content
        .contains("# Run Export: 20260603T120000Z-session-export"));
    assert!(md_rep.content.contains("- **Session ID:** session-export"));
    assert!(md_rep.content.contains("user> hello export"));

    // 3. Export ShareGPT (Unsupported)
    let err_rep = export_run(
        &config,
        "20260603T120000Z-session-export",
        ExportFormat::Sharegpt,
    );
    assert!(err_rep.is_err());
    assert!(err_rep
        .unwrap_err()
        .to_string()
        .contains("ShareGPT export format is not supported yet"));

    // 4. Export ShareGPT (Unsupported) for a missing run
    let err_missing_run = export_run(&config, "non-existent-run-id", ExportFormat::Sharegpt);
    assert!(err_missing_run.is_err());
    assert!(err_missing_run
        .unwrap_err()
        .to_string()
        .contains("ShareGPT export format is not supported yet"));

    let _ = fs::remove_dir_all(&temp_root);
}
