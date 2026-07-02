use gestalt_core::context::{ContextOmission, ContextPacket, ContextSourceRef};
use gestalt_core::{ContextCaptureMode, DurabilityMode, TraceError};
use gestalt_runtime::api::v1::{
    load_context_build_report, persist_context_build_report, CapturedContributionV1,
    ContextBuildReportV1, MAX_CAPTURED_CONTRIBUTIONS_BYTES, MAX_CAPTURED_CONTRIBUTION_BYTES,
};
use gestalt_runtime::unstable::ContextBuildReportInputV1;
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
        capture_mode: ContextCaptureMode::HashOnly,
        captured_contributions: vec![CapturedContributionV1::capture_hash_only(
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
fn context_report_source_order_is_deterministic() {
    let left = report(&packet(vec![source("b"), source("a")]));
    let right = report(&packet(vec![source("a"), source("b")]));

    assert_eq!(left, right);
}

#[test]
fn context_report_replay_integrity_detects_tampering() {
    let mut capture =
        CapturedContributionV1::capture_full_for_replay("dynamic", "original".to_string())
            .expect("capture");
    capture.content = Some("tampered".to_string());

    assert!(matches!(
        capture.replay_content(),
        Err(TraceError::InvalidFormat { .. })
    ));
}

#[test]
fn deterministic_replay_does_not_repeat_contributor_side_effect() {
    let calls = AtomicUsize::new(0);
    let capture =
        CapturedContributionV1::capture_once("dynamic", ContextCaptureMode::FullForReplay, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok("captured".to_string())
        })
        .expect("capture")
        .expect("full capture");
    let report = ContextBuildReportV1::build(ContextBuildReportInputV1 {
        capture_mode: ContextCaptureMode::FullForReplay,
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
fn context_report_best_effort_emits_diagnostic() {
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

#[test]
fn context_report_hash_only_does_not_store_raw_content() {
    let secret = "OPENAI_API_KEY=top-secret";
    assert_eq!(
        gestalt_core::ContextManagementPolicy::default().capture,
        ContextCaptureMode::HashOnly
    );
    let capture = CapturedContributionV1::capture_hash_only("dynamic", secret.to_string())
        .expect("hash-only capture");

    assert_eq!(capture.mode, ContextCaptureMode::HashOnly);
    assert!(capture.content.is_none());
    assert!(!serde_json::to_string(&capture).unwrap().contains(secret));
}

#[test]
fn context_report_redacted_capture_removes_secrets() {
    let capture = CapturedContributionV1::capture_redacted(
        "dynamic",
        [
            "OPENAI_API_KEY=sk-live-secret",
            "Authorization: Bearer header-secret",
            "token=token-secret",
            "provider_credential=credential-secret",
            "keychain_ref=keychain://provider/account",
            "password=hunter2",
        ]
        .join("\n"),
    )
    .expect("redacted capture");
    let content = capture.content.as_deref().expect("redacted content");

    assert_eq!(capture.mode, ContextCaptureMode::Redacted);
    assert_eq!(content.matches("[REDACTED]").count(), 6);
    for secret in [
        "sk-live-secret",
        "header-secret",
        "token-secret",
        "credential-secret",
        "keychain://provider/account",
        "hunter2",
    ] {
        assert!(!content.contains(secret), "{secret} leaked");
    }

    let serialized_message = CapturedContributionV1::capture_redacted(
        "serialized",
        r#"{"type":"system","content":"OPENAI_API_KEY=sk-nested-secret"}"#.to_string(),
    )
    .expect("serialized message capture");
    assert!(!serialized_message
        .content
        .as_deref()
        .unwrap()
        .contains("sk-nested-secret"));
}

#[test]
fn context_report_full_capture_requires_explicit_policy() {
    let capture =
        CapturedContributionV1::capture_full_for_replay("dynamic", "raw replay".to_string())
            .expect("full capture");
    let packet = packet(Vec::new());
    let err = ContextBuildReportV1::build(ContextBuildReportInputV1 {
        captured_contributions: vec![capture.clone()],
        ..report_input(&packet)
    })
    .expect_err("hash-only report must reject full capture");
    assert!(matches!(err, TraceError::InvalidFormat { .. }));

    let report = ContextBuildReportV1::build(ContextBuildReportInputV1 {
        capture_mode: ContextCaptureMode::FullForReplay,
        captured_contributions: vec![capture],
        ..report_input(&packet)
    })
    .expect("explicit full capture report");
    assert_eq!(report.replay_contribution("dynamic").unwrap(), "raw replay");
}

#[test]
fn context_report_single_contribution_bound_is_enforced() {
    let content = "x".repeat(MAX_CAPTURED_CONTRIBUTION_BYTES + 1);
    assert!(matches!(
        CapturedContributionV1::capture_hash_only("large", content),
        Err(TraceError::InvalidFormat { .. })
    ));
}

#[test]
fn context_report_aggregate_bound_is_enforced() {
    let captures = (0..5)
        .map(|index| {
            CapturedContributionV1::capture_hash_only(
                format!("capture-{index}"),
                "x".repeat(MAX_CAPTURED_CONTRIBUTION_BYTES),
            )
            .expect("bounded capture")
        })
        .collect();
    let packet = packet(Vec::new());
    let err = ContextBuildReportV1::build(ContextBuildReportInputV1 {
        captured_contributions: captures,
        ..report_input(&packet)
    })
    .expect_err("aggregate bound must fail");

    assert!(matches!(err, TraceError::InvalidFormat { .. }));
    assert_eq!(
        MAX_CAPTURED_CONTRIBUTIONS_BYTES,
        4 * MAX_CAPTURED_CONTRIBUTION_BYTES
    );
}

#[test]
fn context_report_persistence_required_fails_on_write_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let artifacts_dir = dir.path().join("artifacts");
    fs::write(&artifacts_dir, "blocked").expect("block artifacts dir");
    let report = report(&packet(vec![source("a")]));

    assert!(matches!(
        persist_context_build_report(&report, &artifacts_dir, DurabilityMode::Required),
        Err(TraceError::WriteFailed(_))
    ));
}

#[test]
fn context_report_disabled_durability_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let report = report(&packet(vec![source("a")]));
    persist_context_build_report(&report, dir.path(), DurabilityMode::Disabled)
        .expect("disabled persistence");

    assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
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
        capture_mode: ContextCaptureMode::HashOnly,
        captured_contributions: Vec::new(),
        source_stabilities: BTreeMap::new(),
        deterministic: true,
        prompt_artifact_ref: None,
        projection_artifact_ref: Some("projection.json".to_string()),
    }
}
