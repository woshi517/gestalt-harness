use gestalt_app::config::CliOverrides;
use gestalt_app::context::explain_context;
use std::fs;
use std::path::PathBuf;

fn create_temp_workspace() -> PathBuf {
    std::env::set_var("XDG_CONFIG_HOME", "/tmp/non-existent-gestalt-test-dir");
    let temp = std::env::temp_dir().join(format!("gestalt-test-context-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp).unwrap();
    temp
}

#[tokio::test]
async fn test_context_explain_prompt() {
    let temp_root = create_temp_workspace();
    let gestalt_dir = temp_root.join(".gestalt");
    fs::create_dir_all(&gestalt_dir).unwrap();

    fs::write(
        gestalt_dir.join("config.toml"),
        r#"
[defaults]
provider = "anthropic"
mode = "confirm"
"#,
    )
    .unwrap();
    fs::write(
        gestalt_dir.join("workspace.md"),
        "# Workspace\nSome context about project",
    )
    .unwrap();
    fs::write(gestalt_dir.join("memory.md"), "# Memory\n").unwrap();
    fs::write(gestalt_dir.join("policies.toml"), "# Policies\n").unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    // Explain prompt: verify it builds context explain without any provider call
    let res = explain_context(
        &overrides,
        Some("Explain how to build the gestalt project"),
        None,
    )
    .await;
    assert!(res.is_ok());
    let rep = res.unwrap();
    assert_eq!(
        rep.prompt.as_deref(),
        Some("Explain how to build the gestalt project")
    );
    assert!(rep.token_estimate > 0);

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_context_explain_run() {
    let temp_root = create_temp_workspace();
    let gestalt_dir = temp_root.join(".gestalt");
    let runs_dir = gestalt_dir.join("runs");
    fs::create_dir_all(&runs_dir).unwrap();

    let run_dir = runs_dir.join("20260603T130000Z-session-ctx");
    fs::create_dir_all(&run_dir).unwrap();

    let trace_content = r#"{"v":1,"session_id":"session-ctx","turn_id":1,"seq":1,"ts":"2026-06-03T13:00:00Z","event":{"type":"context_built","packet_id":"v1","token_estimate":456,"packet_hash":"hash123","sources":[],"omissions":[],"prompt_source":""},"redacted":false}"#;
    fs::write(run_dir.join("trace.jsonl"), trace_content).unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    // Explain run: verify it extracts the last context packet from trace
    let res = explain_context(&overrides, None, Some("20260603T130000Z-session-ctx")).await;
    assert!(res.is_ok());
    let rep = res.unwrap();
    assert_eq!(rep.run_id.as_deref(), Some("20260603T130000Z-session-ctx"));
    assert_eq!(rep.token_estimate, 456);

    let _ = fs::remove_dir_all(&temp_root);
}

#[tokio::test]
async fn test_context_explain_prompt_budget_behavior() {
    let temp_root = create_temp_workspace();
    let gestalt_dir = temp_root.join(".gestalt");
    fs::create_dir_all(&gestalt_dir).unwrap();

    fs::write(
        temp_root.join("gestalt.json"),
        r#"{
            "version": 1,
            "defaults": {
                "provider": "anthropic",
                "mode": "confirm"
            },
            "context": {
                "memory": {
                    "enabled": true,
                    "max_tokens": 10
                }
            }
        }"#,
    )
    .unwrap();

    fs::write(
        gestalt_dir.join("workspace.md"),
        "# Workspace instructions\nThis is the workspace instructions content.",
    )
    .unwrap();

    fs::write(
        gestalt_dir.join("memory.md"),
        r"# Memory

## Facts
- <!-- gestalt-memory-id: pin1 --> this is a pinned entry

## General
- <!-- gestalt-memory-id: unpin1 --> this is an unpinned entry that takes way too many tokens to fit in the small budget of ten tokens
",
    )
    .unwrap();

    let overrides = CliOverrides {
        workspace: Some(temp_root.clone()),
        ..CliOverrides::default()
    };

    let res = explain_context(
        &overrides,
        Some("Testing context explain budget behavior"),
        None,
    )
    .await;

    assert!(res.is_ok(), "explain_context failed: {:?}", res.err());
    let rep = res.unwrap();

    // 1. Verify sources are present
    let sources = rep.sources;
    assert!(sources
        .iter()
        .any(|s| s.kind == "workspace" && s.path_or_label.contains("workspace.md")));
    assert!(sources
        .iter()
        .any(|s| s.kind == "memory" && s.path_or_label.contains("memory.md")));

    // 2. Verify pinned entry is in system_prompt while unpinned is not
    let system_prompt = rep.system_prompt.expect("system_prompt is missing");
    assert!(
        system_prompt.contains("this is a pinned entry"),
        "system_prompt should contain the pinned entry"
    );
    assert!(
        !system_prompt.contains("this is an unpinned entry"),
        "system_prompt should NOT contain the unpinned entry"
    );

    // 3. Verify unpinned entry appears in omissions as budget_exhausted
    let omissions = rep.omissions;
    assert!(
        omissions.iter().any(|o| o.kind == "memory"
            && o.path_or_label == "mem_id:unpin1"
            && o.reason == "budget_exhausted"),
        "Omissions should report the unpinned memory being budget-exhausted: {:?}",
        omissions
    );

    let _ = fs::remove_dir_all(&temp_root);
}
