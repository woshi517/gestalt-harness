#![cfg(feature = "full")]

//! Smoke test to verify test fixture paths exist.
//! Prevents accidental deletion of fixture directories.

use std::path::Path;

use gestalt_core::{AgentEvent, ApprovalOutcome, RiskLevel, SessionGrant};

#[test]
fn fixture_directories_exist() {
    // Cargo runs integration tests with the current working directory set to the crate's directory
    let fixtures = Path::new("../../tests/fixtures");
    assert!(fixtures.exists(), "tests/fixtures/ directory missing");

    let expected_dirs = [
        "provider-streams",
        "policy",
        "traces",
        "workspaces",
        "sources",
        "cli-golden",
    ];

    for dir in &expected_dirs {
        let path = fixtures.join(dir);
        assert!(
            path.exists(),
            "fixture directory missing: {}",
            path.display()
        );
    }
}

#[test]
fn minimal_workspace_fixture_exists() {
    let workspace = Path::new("../../tests/fixtures/workspaces/minimal/.gestalt");
    assert!(Path::new("../../tests/fixtures/workspaces/minimal/gestalt.json").exists());
    assert!(workspace.join("config.toml").exists());
    assert!(workspace.join("policies.toml").exists());
    assert!(workspace.join("workspace.md").exists());
    assert!(workspace.join("memory.md").exists());

    let legacy_workspace = Path::new("../../tests/fixtures/workspaces/legacy-minimal/.gestalt");
    assert!(legacy_workspace.join("config.toml").exists());
    assert!(legacy_workspace.join("policies.toml").exists());
    assert!(legacy_workspace.join("workspace.md").exists());
    assert!(legacy_workspace.join("memory.md").exists());
}

#[test]
fn provider_and_trace_fixtures_are_populated() {
    let fixtures = Path::new("../../tests/fixtures");
    assert!(fixtures
        .join("provider-streams/openai-multiple-tools.sse")
        .exists());
    assert!(fixtures
        .join("provider-streams/anthropic-single-tool.sse")
        .exists());
    assert!(fixtures.join("traces/minimal-run.jsonl").exists());
    assert!(fixtures.join("cli-golden/replay-display.txt").exists());
}

#[test]
fn approval_decision_event_round_trips_through_serde() {
    let grant = SessionGrant {
        tool_name: "bash".to_string(),
        input_hash: "deadbeefdeadbeef".to_string(),
        risk_ceiling: RiskLevel::Medium,
        matched_rule: "confirm-all".to_string(),
        policy_source: "session_grant".to_string(),
        granted_at_turn: 0,
        expires_in_turns: 4,
    };
    let event = AgentEvent::ApprovalDecision {
        tool_call_id: "call-1".to_string(),
        decision: ApprovalOutcome::AlwaysAllow,
        original_input_hash: "abcdabcdabcdabcd".to_string(),
        edited_input_hash: None,
        grant_terms: Some(grant),
    };

    let encoded = serde_json::to_string(&event).expect("event encodes");
    let decoded: AgentEvent = serde_json::from_str(&encoded).expect("event decodes");
    assert_eq!(event, decoded);

    let value: serde_json::Value = serde_json::from_str(&encoded).expect("json value");
    assert_eq!(value["type"], "approval_decision");
    assert_eq!(value["decision"], "always_allow");
    assert!(value["grant_terms"]["tool_name"].as_str() == Some("bash"));
}

#[test]
fn approval_decision_event_without_grant_is_valid() {
    let event = AgentEvent::ApprovalDecision {
        tool_call_id: "call-2".to_string(),
        decision: ApprovalOutcome::Deny,
        original_input_hash: "0000000000000000".to_string(),
        edited_input_hash: None,
        grant_terms: None,
    };
    let encoded = serde_json::to_string(&event).expect("event encodes");
    let decoded: AgentEvent = serde_json::from_str(&encoded).expect("event decodes");
    assert_eq!(event, decoded);
}
