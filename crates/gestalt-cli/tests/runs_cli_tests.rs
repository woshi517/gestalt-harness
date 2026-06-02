use std::fs;
use std::path::PathBuf;
use gestalt_cli::config::{CliOverrides, load_effective_config};
use gestalt_cli::runs::{list_runs, inspect_run, resolve_run_path, prune_runs, delete_run};

fn create_temp_workspace() -> PathBuf {
    let temp = std::env::temp_dir().join(format!("gestalt-test-runs-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[test]
fn test_runs_list_and_inspect() {
    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run1 = runs_dir.join("20260602T100000Z-session-1");
    fs::create_dir_all(&run1).unwrap();
    let trace1 = r#"{"v":1,"session_id":"session-1","turn_id":1,"seq":1,"ts":"2026-06-02T10:00:00Z","event":{"type":"model_request","provider":"anthropic","model":"claude-3-5-sonnet-20241022"},"redacted":false}
{"v":1,"session_id":"session-1","turn_id":1,"seq":2,"ts":"2026-06-02T10:01:00Z","event":{"type":"stop","reason":"end_turn"},"redacted":false}"#;
    fs::write(run1.join("trace.jsonl"), trace1).unwrap();
    fs::write(run1.join("cost.json"), r#"{"runs":1,"input_tokens":100,"output_tokens":50,"estimated_cost_usd":0.0015,"warnings":[]}"#).unwrap();

    let run2 = runs_dir.join("20260602T120000Z-session-2");
    fs::create_dir_all(&run2).unwrap();
    let trace2 = r#"{"v":1,"session_id":"session-2","turn_id":1,"seq":1,"ts":"2026-06-02T12:00:00Z","event":{"type":"model_request","provider":"openai","model":"gpt-4o"},"redacted":false}
{"v":1,"session_id":"session-2","turn_id":1,"seq":2,"ts":"2026-06-02T12:01:00Z","event":{"type":"error","message":"provider issue","recoverable":false},"redacted":false}"#;
    fs::write(run2.join("trace.jsonl"), trace2).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    let resolved = resolve_run_path(&config, "20260602T100000Z").unwrap();
    assert_eq!(resolved, run1);

    let resolved_prefix = resolve_run_path(&config, "20260602T12").unwrap();
    assert_eq!(resolved_prefix, run2);

    let err_res = resolve_run_path(&config, "nonexistent");
    assert!(err_res.is_err());

    let list_all = list_runs(&config, None).unwrap();
    assert_eq!(list_all.runs.len(), 2);
    assert_eq!(list_all.runs[0].run_id, "20260602T120000Z-session-2");
    assert_eq!(list_all.runs[0].apparent_status, "failed");
    assert_eq!(list_all.runs[0].provider.as_deref(), Some("openai"));
    assert_eq!(list_all.runs[0].model.as_deref(), Some("gpt-4o"));

    assert_eq!(list_all.runs[1].run_id, "20260602T100000Z-session-1");
    assert_eq!(list_all.runs[1].apparent_status, "completed");
    assert_eq!(list_all.runs[1].provider.as_deref(), Some("anthropic"));
    assert_eq!(list_all.runs[1].estimated_cost_usd, Some(0.0015));

    let list_limit = list_runs(&config, Some(1)).unwrap();
    assert_eq!(list_limit.runs.len(), 1);
    assert_eq!(list_limit.runs[0].run_id, "20260602T120000Z-session-2");

    let inspect1 = inspect_run(&config, "20260602T100000Z-session-1").unwrap();
    assert_eq!(inspect1.run_id, "20260602T100000Z-session-1");
    assert_eq!(inspect1.apparent_status, "completed");
    assert_eq!(inspect1.turns, Some(1));
    assert_eq!(inspect1.total_input_tokens, Some(100));

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_runs_prune_and_delete() {
    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run1 = runs_dir.join("20260602T100000Z-session-1");
    fs::create_dir_all(&run1).unwrap();
    fs::write(run1.join("trace.jsonl"), "{}").unwrap();

    let run2 = runs_dir.join("20260602T120000Z-session-2");
    fs::create_dir_all(&run2).unwrap();
    fs::write(run2.join("trace.jsonl"), "{}").unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    let prune_dry = prune_runs(&config, Some("1s".to_string()), true, true).unwrap();
    assert!(prune_dry.dry_run);
    assert_eq!(prune_dry.pruned_runs.len(), 2);
    assert!(run1.exists());
    assert!(run2.exists());

    let prune_real = prune_runs(&config, Some("1s".to_string()), false, true).unwrap();
    assert!(!prune_real.dry_run);
    assert_eq!(prune_real.pruned_runs.len(), 2);
    assert!(!run1.exists());
    assert!(!run2.exists());

    let run3 = runs_dir.join("20260602T150000Z-session-3");
    fs::create_dir_all(&run3).unwrap();
    fs::write(run3.join("trace.jsonl"), "{}").unwrap();
    assert!(run3.exists());

    let delete_rep = delete_run(&config, "20260602T150000Z-session-3", true).unwrap();
    assert_eq!(delete_rep.deleted_run, "20260602T150000Z-session-3");
    assert!(!run3.exists());

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_runs_edge_cases() {
    // 1. parse_duration edge cases
    assert!(gestalt_cli::runs::parse_duration("").is_err());
    assert!(gestalt_cli::runs::parse_duration("10").is_err());
    assert!(gestalt_cli::runs::parse_duration("10x").is_err());
    assert_eq!(gestalt_cli::runs::parse_duration("10d").unwrap(), chrono::Duration::days(10));
    assert_eq!(gestalt_cli::runs::parse_duration("5h").unwrap(), chrono::Duration::hours(5));
    assert_eq!(gestalt_cli::runs::parse_duration("30m").unwrap(), chrono::Duration::minutes(30));
    assert_eq!(gestalt_cli::runs::parse_duration("60s").unwrap(), chrono::Duration::seconds(60));
    assert!(gestalt_cli::runs::parse_duration("10秒").is_err());

    // 2. parse_run_timestamp edge cases
    assert!(gestalt_cli::runs::parse_run_timestamp("").is_none());
    assert!(gestalt_cli::runs::parse_run_timestamp("20260602T100000Z").is_none());
    assert!(gestalt_cli::runs::parse_run_timestamp("20260602T100000Z-session-1").is_some());
    assert!(gestalt_cli::runs::parse_run_timestamp("20260602T100000Z-日本語").is_none());

    // 3. scan_trace_file edge cases
    let missing_path = std::path::Path::new("nonexistent-trace-file.jsonl");
    assert!(gestalt_cli::runs::scan_trace_file(missing_path).is_err());

    // 4. resolve_run_path ambiguity
    let temp_root = std::env::temp_dir().join(format!("gestalt-test-ambiguity-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_root.join(".gestalt/runs")).unwrap();
    let run1 = temp_root.join(".gestalt/runs/20260602T100000Z-session-1");
    let run2 = temp_root.join(".gestalt/runs/20260602T100000Z-session-2");
    fs::create_dir_all(&run1).unwrap();
    fs::create_dir_all(&run2).unwrap();
    fs::write(run1.join("trace.jsonl"), "{}").unwrap();
    fs::write(run2.join("trace.jsonl"), "{}").unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();
    let err_res = resolve_run_path(&config, "20260602T10");
    assert!(err_res.is_err());
    let err_msg = format!("{}", err_res.unwrap_err());
    assert!(err_msg.contains("ambiguous run ID"));

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_runs_new_features() {
    use std::io::{Write, Seek, SeekFrom};
    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    // 1. Test read_next_line tailing line stability
    let file_path = temp_root.join("test_tail.jsonl");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&file_path)
        .unwrap();

    // Write complete line and check
    file.write_all(b"complete line\n").unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut buf = String::new();
    let n = gestalt_cli::runs::read_next_line(&mut file, &mut buf).unwrap();
    assert_eq!(n, 14);
    assert_eq!(buf, "complete line\n");

    // Write partial line (without newline) and check it seeks back (returns 0)
    let current_pos = file.stream_position().unwrap();
    file.write_all(b"partial line").unwrap();
    file.seek(SeekFrom::Start(current_pos)).unwrap();
    let n2 = gestalt_cli::runs::read_next_line(&mut file, &mut buf).unwrap();
    assert_eq!(n2, 0);
    // Position should be restored to current_pos
    assert_eq!(file.stream_position().unwrap(), current_pos);

    // Now write the newline to make it complete, and check it reads it
    file.seek(SeekFrom::End(0)).unwrap();
    file.write_all(b"\n").unwrap();
    file.seek(SeekFrom::Start(current_pos)).unwrap();
    let n3 = gestalt_cli::runs::read_next_line(&mut file, &mut buf).unwrap();
    assert_eq!(n3, 13);
    assert_eq!(buf, "partial line\n");

    // 2. Test resolve_run_path file name constraint
    let run1 = runs_dir.join("20260602T100000Z-session-1");
    fs::create_dir_all(&run1).unwrap();
    let trace_path = run1.join("trace.jsonl");
    let cost_path = run1.join("cost.json");
    fs::write(&trace_path, "{}").unwrap();
    fs::write(&cost_path, "{}").unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    // trace.jsonl resolves to its parent
    let resolved_trace = resolve_run_path(&config, trace_path.to_str().unwrap()).unwrap();
    assert_eq!(resolved_trace, run1);

    // cost.json does not resolve to parent (it should fail resolution or treat as prefix match)
    let resolved_cost = resolve_run_path(&config, cost_path.to_str().unwrap());
    assert!(resolved_cost.is_err());

    // 3. Destructive containment / path traversal prevention
    let outside_run = temp_root.join("outside_run");
    fs::create_dir_all(&outside_run).unwrap();
    fs::write(outside_run.join("trace.jsonl"), "{}").unwrap();

    let delete_outside = delete_run(&config, outside_run.to_str().unwrap(), true);
    assert!(delete_outside.is_err());
    let err_msg = match delete_outside {
        Err(e) => format!("{}", e),
        _ => panic!("expected error"),
    };
    assert!(err_msg.contains("is not within the run log directory"));

    // 4. Test non-interactive prune / delete rejecting execution (only if stdin is indeed non-interactive in this test environment)
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let delete_noninteractive = delete_run(&config, "20260602T100000Z-session-1", false);
        assert!(delete_noninteractive.is_err());
        let err_msg_noninteractive = match delete_noninteractive {
            Err(e) => format!("{}", e),
            _ => panic!("expected error"),
        };
        assert!(err_msg_noninteractive.contains("non-interactive execution requires"));

        let prune_noninteractive = prune_runs(&config, Some("1s".to_string()), false, false);
        assert!(prune_noninteractive.is_err());
        let err_msg_prune = match prune_noninteractive {
            Err(e) => format!("{}", e),
            _ => panic!("expected error"),
        };
        assert!(err_msg_prune.contains("non-interactive execution requires"));
    }

    let _ = fs::remove_dir_all(&temp_root);
}


