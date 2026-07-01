use gestalt_core::context::{ContextOmission, ContextPacket, ContextSourceRef};
use gestalt_core::{DurabilityMode, TraceError};
use gestalt_runtime::unstable::{
    load_context_build_report, persist_context_build_report, CapturedContributionV1,
    ContextBuildReportInputV1, ContextBuildReportV1,
};
use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

fn packet(sources: Vec<ContextSourceRef>) -> ContextPacket {
    ContextPacket {
        messages: Vec::new(),
        packet_hash: "packet".to_string(),
        pipeline_version: "pipeline-v1".to_string(),
        tokenizer_id: "tokenizer-v1".to_string(),
        token_estimate: 80,
        sources,
        omissions: vec![ContextOmission {
            kind: "history".to_string(),
            path_or_label: "old".to_string(),
            trust: "trusted".to_string(),
            reason: "budget_exhausted".to_string(),
            token_estimate: 20,
            authority: None,
        }],
        message_hashes: Vec::new(),
        prompt_assembly_strategy: Default::default(),
        snapshot_hash: None,
        cache_prefix_hash: None,
        segments: Vec::new(),
        cache_plan: None,
        prompt_source: None,
    }
}

fn source(label: &str) -> ContextSourceRef {
    ContextSourceRef {
        kind: "workspace".to_string(),
        path_or_label: label.to_string(),
        trust: "trusted".to_string(),
        token_estimate: 10,
        included: true,
        authority: Some("instructions".to_string()),
    }
}

fn report(packet: &ContextPacket) -> ContextBuildReportV1 {
    ContextBuildReportV1::build(ContextBuildReportInputV1 {
        session_id: "session",
        run_id: "run",
        turn_id: 1,
        packet,
        input_limit: 100,
        context_policy_fingerprint: "policy",
        model_capability_fingerprint: "model",
        runtime_fingerprint: "runtime",
        tool_fingerprint: "tools",
        workspace_snapshot_hash: Some("workspace"),
        captured_contributions: vec![CapturedContributionV1::capture_redacted(
            "workspace:a",
            "captured".to_string(),
        )
        .expect("capture")],
        source_stabilities: BTreeMap::new(),
        deterministic: true,
        prompt_artifact_ref: None,
        projection_artifact_ref: Some("projection.json".to_string()),
    })
    .expect("report")
}

#[test]
fn report_identity_is_independent_of_source_registration_order() {
    let left = report(&packet(vec![source("b"), source("a")]));
    let right = report(&packet(vec![source("a"), source("b")]));

    assert_eq!(left, right);
}

#[test]
fn replay_rejects_tampered_capture() {
    let mut capture = CapturedContributionV1::capture_redacted("dynamic", "original".to_string())
        .expect("capture");
    capture.content = "tampered".to_string();

    assert!(matches!(
        capture.replay_content(),
        Err(TraceError::InvalidFormat { .. })
    ));
}

#[test]
fn deterministic_replay_does_not_repeat_contributor_side_effect() {
    let calls = AtomicUsize::new(0);
    let capture = CapturedContributionV1::capture_redacted_once("dynamic", || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok("captured".to_string())
    })
    .expect("capture");
    let report = ContextBuildReportV1::build(ContextBuildReportInputV1 {
        captured_contributions: vec![capture],
        ..report_input(&packet(Vec::new()))
    })
    .expect("report");

    assert_eq!(
        (
            report.replay_contribution("dynamic").expect("replay"),
            calls.load(Ordering::SeqCst)
        ),
        ("captured", 1)
    );
}

#[test]
fn persisted_report_round_trips_and_rejects_unsupported_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = report(&packet(vec![source("a")]));
    persist_context_build_report(&report, dir.path(), DurabilityMode::Required)
        .expect("persist report");

    let loaded = load_context_build_report(&report.report_id, dir.path()).expect("load report");
    assert_eq!(loaded, report);

    let path = dir
        .path()
        .join(format!("context_report_{}.json", report.report_id));
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read report")).expect("parse report json");
    value["v"] = serde_json::json!(2);
    fs::write(
        &path,
        serde_json::to_vec_pretty(&value).expect("serialize mutated report"),
    )
    .expect("write mutated report");

    let err = load_context_build_report(&report.report_id, dir.path())
        .expect_err("unsupported report version must fail");
    assert!(matches!(err, TraceError::InvalidFormat { .. }));
}

#[test]
fn best_effort_persistence_returns_structured_diagnostic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifacts_dir = dir.path().join("artifacts");
    fs::write(&artifacts_dir, "blocked").expect("block artifacts dir");

    let report = report(&packet(vec![source("a")]));
    let diagnostic =
        persist_context_build_report(&report, &artifacts_dir, DurabilityMode::BestEffort)
            .expect("best effort persistence");

    let diagnostic = diagnostic.expect("best effort persistence should return a diagnostic");
    assert_eq!(diagnostic.code, "CONTEXT_REPORT_PERSISTENCE_FAILED");
}

fn report_input(packet: &ContextPacket) -> ContextBuildReportInputV1<'_> {
    ContextBuildReportInputV1 {
        session_id: "session",
        run_id: "run",
        turn_id: 1,
        packet,
        input_limit: 100,
        context_policy_fingerprint: "policy",
        model_capability_fingerprint: "model",
        runtime_fingerprint: "runtime",
        tool_fingerprint: "tools",
        workspace_snapshot_hash: Some("workspace"),
        captured_contributions: Vec::new(),
        source_stabilities: BTreeMap::new(),
        deterministic: true,
        prompt_artifact_ref: None,
        projection_artifact_ref: Some("projection.json".to_string()),
    }
}
