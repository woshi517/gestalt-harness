use gestalt_cli::config::{load_effective_config, CliOverrides};
use gestalt_cli::trace::{inspect_trace, replay_trace, validate_trace};
use std::fs;
use std::path::PathBuf;

fn create_temp_workspace() -> PathBuf {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let temp = std::env::temp_dir().join(format!("gestalt-test-trace-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[test]
fn test_trace_replay_and_inspect() {
    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run_dir = runs_dir.join("20260603T100000Z-session-123");
    fs::create_dir_all(&run_dir).unwrap();

    let trace_content = r#"{"v":1,"session_id":"session-123","turn_id":1,"seq":1,"ts":"2026-06-03T10:00:00Z","event":{"type":"user_message","content":"hello world"},"redacted":false}
{"v":1,"session_id":"session-123","turn_id":1,"seq":2,"ts":"2026-06-03T10:00:01Z","event":{"type":"model_request","provider":"anthropic","model":"claude-3-5-sonnet-20241022"},"redacted":false}
{"v":1,"session_id":"session-123","turn_id":1,"seq":3,"ts":"2026-06-03T10:00:02Z","event":{"type":"prompt_snapshot_created","snapshot_hash":"snapshot-hash","prefix_hash":"prefix-hash","created_turn":1},"redacted":false}
{"v":1,"session_id":"session-123","turn_id":1,"seq":4,"ts":"2026-06-03T10:00:03Z","event":{"type":"prompt_cache_plan_generated","snapshot_hash":"snapshot-hash","prefix_hash":"prefix-hash","prefix_message_count":2},"redacted":false}
{"v":1,"session_id":"session-123","turn_id":1,"seq":5,"ts":"2026-06-03T10:00:04Z","event":{"type":"prompt_snapshot_reused","snapshot_hash":"snapshot-hash","prefix_hash":"prefix-hash"},"redacted":false}
{"v":1,"session_id":"session-123","turn_id":1,"seq":6,"ts":"2026-06-03T10:00:05Z","event":{"type":"usage","input_tokens":15,"output_tokens":8},"redacted":false}
{"v":1,"session_id":"session-123","turn_id":1,"seq":7,"ts":"2026-06-03T10:00:06Z","event":{"type":"artifact_created","path":"artifacts/output.txt","size_bytes":12,"mime_type":"text/plain","hash":"abc123xyz"},"redacted":true}
{"v":1,"session_id":"session-123","turn_id":1,"seq":8,"ts":"2026-06-03T10:00:07Z","event":{"type":"stop","reason":"end_turn"},"redacted":false}"#;

    fs::write(run_dir.join("trace.jsonl"), trace_content).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    // 1. Test Replay
    let replay_rep = replay_trace(&config, "20260603T100000Z-session-123").unwrap();
    assert!(replay_rep.rendered.contains("user> hello world"));
    assert!(replay_rep
        .rendered
        .contains("model> anthropic/claude-3-5-sonnet-20241022"));
    assert!(replay_rep
        .rendered
        .contains("artifact-created> artifacts/output.txt size=12 mime=text/plain hash=abc123xy"));
    assert!(replay_rep
        .rendered
        .contains("snapshot-created> snapshot prefix=prefix-h"));
    assert!(replay_rep
        .rendered
        .contains("cache-plan> snapshot prefix=prefix-h messages=2"));
    assert!(replay_rep
        .rendered
        .contains("snapshot-reused> snapshot prefix=prefix-h"));

    // 2. Test Inspect
    let inspect_rep = inspect_trace(&config, "20260603T100000Z-session-123").unwrap();
    assert_eq!(inspect_rep.run_id, "20260603T100000Z-session-123");
    assert_eq!(inspect_rep.total_events, 8);
    assert_eq!(inspect_rep.turns, 1);
    assert_eq!(inspect_rep.total_input_tokens, 15);
    assert_eq!(inspect_rep.total_output_tokens, 8);
    assert_eq!(inspect_rep.prompt_snapshots_created, 1);
    assert_eq!(inspect_rep.prompt_cache_plans, 1);
    assert_eq!(inspect_rep.prompt_snapshots_reused, 1);
    assert!(inspect_rep.redacted);
    assert_eq!(
        inspect_rep.artifacts,
        vec!["artifacts/output.txt".to_string()]
    );

    // 3. Test with Run Dir path
    let inspect_by_dir = inspect_trace(&config, &run_dir.to_string_lossy()).unwrap();
    assert_eq!(inspect_by_dir.run_id, "20260603T100000Z-session-123");

    // 4. Test with trace.jsonl file path
    let trace_file_path = run_dir.join("trace.jsonl");
    let inspect_by_file = inspect_trace(&config, &trace_file_path.to_string_lossy()).unwrap();
    assert_eq!(inspect_by_file.run_id, "20260603T100000Z-session-123");

    // 5. Test Validate with Run Dir path
    let validate_by_dir = validate_trace(&config, &run_dir.to_string_lossy()).unwrap();
    assert!(validate_by_dir.valid);

    // 6. Test Validate with trace.jsonl file path
    let validate_by_file = validate_trace(&config, &trace_file_path.to_string_lossy()).unwrap();
    assert!(validate_by_file.valid);

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_trace_validation() {
    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run_dir = runs_dir.join("20260603T110000Z-session-val");
    fs::create_dir_all(&run_dir).unwrap();

    // Valid trace content
    let valid_trace = r#"{"v":1,"session_id":"session-val","turn_id":1,"seq":1,"ts":"2026-06-03T11:00:00Z","event":{"type":"user_message","content":"test"},"redacted":false}
{"v":1,"session_id":"session-val","turn_id":1,"seq":2,"ts":"2026-06-03T11:00:01Z","event":{"type":"stop","reason":"end_turn"},"redacted":false}"#;
    fs::write(run_dir.join("trace.jsonl"), valid_trace).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    let valid_rep = validate_trace(&config, "20260603T110000Z-session-val").unwrap();
    assert!(valid_rep.valid);
    assert!(valid_rep.errors.is_empty());

    // Invalid version trace
    let invalid_version_trace = r#"{"v":2,"session_id":"session-val","turn_id":1,"seq":1,"ts":"2026-06-03T11:00:00Z","event":{"type":"user_message","content":"test"},"redacted":false}"#;
    fs::write(run_dir.join("trace.jsonl"), invalid_version_trace).unwrap();
    let invalid_ver_rep = validate_trace(&config, "20260603T110000Z-session-val").unwrap();
    assert!(!invalid_ver_rep.valid);
    assert!(invalid_ver_rep.errors[0].contains("invalid schema version"));

    // Regression seq trace
    let reg_trace = r#"{"v":1,"session_id":"session-val","turn_id":1,"seq":5,"ts":"2026-06-03T11:00:00Z","event":{"type":"user_message","content":"test"},"redacted":false}
{"v":1,"session_id":"session-val","turn_id":1,"seq":3,"ts":"2026-06-03T11:00:01Z","event":{"type":"stop","reason":"end_turn"},"redacted":false}"#;
    fs::write(run_dir.join("trace.jsonl"), reg_trace).unwrap();
    let reg_rep = validate_trace(&config, "20260603T110000Z-session-val").unwrap();
    assert!(!reg_rep.valid);
    assert!(reg_rep.errors[0].contains("sequence number regression"));

    // Missing artifact trace
    let missing_artifact_trace = r#"{"v":1,"session_id":"session-val","turn_id":1,"seq":1,"ts":"2026-06-03T11:00:00Z","event":{"type":"artifact_created","path":"artifacts/missing.md","size_bytes":100,"mime_type":"text/markdown","hash":"abc"},"redacted":false}"#;
    fs::write(run_dir.join("trace.jsonl"), missing_artifact_trace).unwrap();
    let missing_art_rep = validate_trace(&config, "20260603T110000Z-session-val").unwrap();
    assert!(missing_art_rep.valid); // Artifact missing should be a warning, not an error
    assert_eq!(missing_art_rep.warnings.len(), 1);
    assert!(missing_art_rep.warnings[0]
        .contains("referenced artifact does not exist at artifacts/missing.md"));

    let _ = fs::remove_dir_all(&temp_root);
}
