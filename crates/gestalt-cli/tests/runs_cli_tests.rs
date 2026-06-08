use gestalt_cli::config::{load_effective_config, CliOverrides};
use gestalt_cli::runs::{delete_run, inspect_run, list_runs, prune_runs, resolve_run_path};
use std::fs;
use std::path::PathBuf;

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

    let prune_dry = prune_runs(&config, Some("1s".to_string()), true, true, false).unwrap();
    assert!(prune_dry.dry_run);
    assert_eq!(prune_dry.pruned_runs.len(), 2);
    assert!(run1.exists());
    assert!(run2.exists());

    let prune_real = prune_runs(&config, Some("1s".to_string()), false, true, false).unwrap();
    assert!(!prune_real.dry_run);
    assert_eq!(prune_real.pruned_runs.len(), 2);
    assert!(!run1.exists());
    assert!(!run2.exists());

    let run3 = runs_dir.join("20260602T150000Z-session-3");
    fs::create_dir_all(&run3).unwrap();
    fs::write(run3.join("trace.jsonl"), "{}").unwrap();
    assert!(run3.exists());

    let delete_rep = delete_run(&config, "20260602T150000Z-session-3", true, false).unwrap();
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
    assert_eq!(
        gestalt_cli::runs::parse_duration("10d").unwrap(),
        chrono::Duration::days(10)
    );
    assert_eq!(
        gestalt_cli::runs::parse_duration("5h").unwrap(),
        chrono::Duration::hours(5)
    );
    assert_eq!(
        gestalt_cli::runs::parse_duration("30m").unwrap(),
        chrono::Duration::minutes(30)
    );
    assert_eq!(
        gestalt_cli::runs::parse_duration("60s").unwrap(),
        chrono::Duration::seconds(60)
    );
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
    let temp_root =
        std::env::temp_dir().join(format!("gestalt-test-ambiguity-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(temp_root.join(".gestalt/runs")).unwrap();
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
    use std::io::{Seek, SeekFrom, Write};
    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    // 1. Test read_next_line tailing line stability
    let file_path = temp_root.join("test_tail.jsonl");
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
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

    let delete_outside = delete_run(&config, outside_run.to_str().unwrap(), true, false);
    assert!(delete_outside.is_err());
    let err_msg = match delete_outside {
        Err(e) => format!("{}", e),
        _ => panic!("expected error"),
    };
    assert!(err_msg.contains("is not within the run log directory"));

    // 4. Test non-interactive prune / delete rejecting execution (only if stdin is indeed non-interactive in this test environment)
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        let delete_noninteractive = delete_run(&config, "20260602T100000Z-session-1", false, false);
        assert!(delete_noninteractive.is_err());
        let err_msg_noninteractive = match delete_noninteractive {
            Err(e) => format!("{}", e),
            _ => panic!("expected error"),
        };
        assert!(err_msg_noninteractive.contains("non-interactive execution requires"));

        let prune_noninteractive = prune_runs(&config, Some("1s".to_string()), false, false, false);
        assert!(prune_noninteractive.is_err());
        let err_msg_prune = match prune_noninteractive {
            Err(e) => format!("{}", e),
            _ => panic!("expected error"),
        };
        assert!(err_msg_prune.contains("non-interactive execution requires"));
    }

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_runs_additional_patch_requirements() {
    // 1. negative/zero duration rejection
    assert!(gestalt_cli::runs::parse_duration("-1d").is_err());
    assert!(gestalt_cli::runs::parse_duration("0s").is_err());

    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run_dir = runs_dir.join("20260602T160000Z-session-4");
    fs::create_dir_all(&run_dir).unwrap();

    // 2. tool-use-only trace ends as interrupted, not completed
    let trace_tool_use = r#"{"v":1,"session_id":"session-4","turn_id":1,"seq":1,"ts":"2026-06-02T16:00:00Z","event":{"type":"model_request","provider":"openai","model":"gpt-4"},"redacted":false}
{"v":1,"session_id":"session-4","turn_id":1,"seq":2,"ts":"2026-06-02T16:01:00Z","event":{"type":"stop","reason":"tool_use"},"redacted":false}"#;
    fs::write(run_dir.join("trace.jsonl"), trace_tool_use).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    let scan = gestalt_cli::runs::scan_trace_file(&run_dir.join("trace.jsonl")).unwrap();
    assert_eq!(scan.apparent_status, "interrupted");

    // 3. recoverable error followed by success stays non-failed
    let trace_recoverable = r#"{"v":1,"session_id":"session-4","turn_id":1,"seq":1,"ts":"2026-06-02T16:00:00Z","event":{"type":"model_request","provider":"openai","model":"gpt-4"},"redacted":false}
{"v":1,"session_id":"session-4","turn_id":1,"seq":2,"ts":"2026-06-02T16:01:00Z","event":{"type":"error","message":"recoverable error","recoverable":true},"redacted":false}
{"v":1,"session_id":"session-4","turn_id":1,"seq":3,"ts":"2026-06-02T16:02:00Z","event":{"type":"stop","reason":"end_turn"},"redacted":false}"#;
    fs::write(run_dir.join("trace.jsonl"), trace_recoverable).unwrap();

    let scan2 = gestalt_cli::runs::scan_trace_file(&run_dir.join("trace.jsonl")).unwrap();
    assert_eq!(scan2.apparent_status, "completed");

    // 4. runs inspect includes summary/artifacts/snapshot metadata
    fs::write(run_dir.join("summary.md"), "Summary Content").unwrap();
    let artifacts_dir = run_dir.join("artifacts");
    fs::create_dir_all(&artifacts_dir).unwrap();
    fs::write(artifacts_dir.join("output.txt"), "some output").unwrap();

    let inspect = inspect_run(&config, "20260602T160000Z-session-4").unwrap();
    assert!(inspect.summary_exists);
    assert_eq!(inspect.artifacts, vec!["output.txt".to_string()]);

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_runs_descendant_aware_prune_and_delete() {
    use gestalt_trace::run_manifest::{
        CompatibilityFingerprint, LifecycleState, RunKind, RunManifest,
    };

    let temp_root = create_temp_workspace();
    let runs_dir = temp_root.join(".gestalt/runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };
    let config = load_effective_config(&overrides).unwrap();

    // Create a lineage: parent (old) -> child (new)
    let parent_id = "parent-id".to_string();
    let child_id = "child-id".to_string();

    let parent_dir = runs_dir.join(format!("20260602T100000Z-{}", parent_id));
    fs::create_dir_all(&parent_dir).unwrap();
    fs::write(parent_dir.join("trace.jsonl"), "{}").unwrap();

    let child_dir = runs_dir.join(format!("20260602T120000Z-{}", child_id));
    fs::create_dir_all(&child_dir).unwrap();
    fs::write(child_dir.join("trace.jsonl"), "{}").unwrap();

    let fingerprint = CompatibilityFingerprint {
        context_pipeline_version: "pipeline-v1".to_string(),
        tool_schema_hash: "hash".to_string(),
        policy_fingerprint: "policy".to_string(),
        hook_contract_hash: "hook".to_string(),
        execution_mode: "Yolo".to_string(),
    };

    let parent_manifest = RunManifest {
        v: 1,
        session_id: "session-1".to_string(),
        run_id: parent_id.clone(),
        parent_run_id: None,
        base_checkpoint: None,
        run_kind: RunKind::New,
        created_at: chrono::Utc::now() - chrono::Duration::hours(2),
        lifecycle_state: LifecycleState::Completed,
        finalized_at: Some(chrono::Utc::now() - chrono::Duration::hours(2)),
        failure_kind: None,
        interrupted_phase: None,
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        compatibility_fingerprint: fingerprint.clone(),
    };
    parent_manifest
        .save_to(&parent_dir.join("run.json"))
        .unwrap();

    let child_manifest = RunManifest {
        v: 1,
        session_id: "session-1".to_string(),
        run_id: child_id.clone(),
        parent_run_id: Some(parent_id.clone()),
        base_checkpoint: Some(1),
        run_kind: RunKind::Continue,
        created_at: chrono::Utc::now(),
        lifecycle_state: LifecycleState::Completed,
        finalized_at: Some(chrono::Utc::now()),
        failure_kind: None,
        interrupted_phase: None,
        prompt_snapshot_hash: None,
        prompt_snapshot_path: None,
        compatibility_fingerprint: fingerprint.clone(),
    };
    child_manifest.save_to(&child_dir.join("run.json")).unwrap();

    // 1. Delete parent without cascade should fail because child is a descendant
    let delete_err = delete_run(&config, &parent_id, true, false);
    assert!(delete_err.is_err());
    let err_msg = format!("{:?}", delete_err.err().unwrap());
    assert!(err_msg.contains("has descendant runs") || err_msg.contains("cascade"));

    // 2. Prune parent without cascade should fail
    let prune_err = prune_runs(&config, Some("1h".to_string()), false, true, false);
    assert!(prune_err.is_err());
    let prune_err_msg = format!("{:?}", prune_err.err().unwrap());
    assert!(prune_err_msg.contains("has descendant runs") || prune_err_msg.contains("cascade"));

    // 3. Delete parent WITH cascade should succeed and delete both parent and child
    let delete_ok = delete_run(&config, &parent_id, true, true).unwrap();
    assert_eq!(delete_ok.deleted_run, parent_id);
    assert!(!parent_dir.exists());
    assert!(!child_dir.exists());

    let _ = fs::remove_dir_all(&temp_root);
}
