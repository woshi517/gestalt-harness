#![forbid(unsafe_code)]
#![cfg(all(feature = "loops", feature = "tools", feature = "providers"))]
//! End-to-end integration, byte-prefix cache stability, sandbox security,
//! multi-phase routing, and offline replay conformance suite for `gestalt-std` (Milestone 1.3a).

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use gestalt_kernel::{
    Extension, ExtensionRegistry, LoopConfig, PauseReason, PolicyGate, ReplayEngine, ReplayMode,
    RunEvent, SessionTree, SteeringKind, SteeringSource,
};
use gestalt_runtime::sandbox::UnsandboxedFallbackDriver;
use gestalt_runtime::{
    CancellationToken, ConfigLoadOptions, CredentialStore, EventSink, LiveSteeringMessage,
    PauseHandler, RuntimeError, RuntimeHost, RuntimeRegistry, TurnOutcome, WorkspaceTrustStore,
    load_and_merge_config,
};
use gestalt_std::{
    StdExtension, StdToolsExtension, default_std_sandbox_policy, discover_tracked_artifact_paths,
    register_std_commands, register_std_providers, register_std_tools,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

#[derive(Default)]
struct CollectingSink {
    events: Mutex<Vec<RunEvent>>,
}

impl EventSink for CollectingSink {
    fn on_event(&self, event: &RunEvent) {
        if let Ok(mut g) = self.events.lock() {
            g.push(event.clone());
        }
    }
}

struct ApprovingPauseHandler;
impl PauseHandler for ApprovingPauseHandler {
    fn resolve_pause<'a>(
        &'a self,
        _pause_id: &'a str,
        _effect_id: &'a str,
        _reason: PauseReason,
        _payload: &'a Value,
        _cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<Value, RuntimeError>> + Send + 'a>> {
        Box::pin(async move { Ok(json!({ "approved": true })) })
    }
}

async fn spawn_mock_http_server(
    responses: Vec<(&'static str, String)>,
) -> (std::net::SocketAddr, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();

    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let mut content_len: usize = 0;
            let mut header_end: Option<usize> = None;

            loop {
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&buf[..n]);
                if header_end.is_none()
                    && let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    header_end = Some(pos + 4);
                    let hdr_str = String::from_utf8_lossy(&raw[..pos + 4]);
                    for line in hdr_str.lines() {
                        if let Some((k, v)) = line.split_once(':')
                            && k.trim().eq_ignore_ascii_case("content-length")
                        {
                            content_len = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                if let Some(hend) = header_end
                    && raw.len() >= hend + content_len
                {
                    let body_slice = &raw[hend..hend + content_len];
                    if let Ok(json_body) = serde_json::from_slice::<Value>(body_slice)
                        && let Ok(mut g) = captured_clone.lock()
                    {
                        g.push(json_body);
                    }
                    break;
                }
            }

            let next_resp = queue.lock().ok().and_then(|mut q| q.pop_front());
            let (ctype, body) = next_resp.unwrap_or(("application/json", "{}".to_string()));
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(http.as_bytes()).await;
        }
    });

    (addr, captured)
}

#[tokio::test]
async fn multi_turn_standard_builder_cache_stability_tools_and_replay() {
    let dir = std::env::temp_dir().join(format!("gestalt_std_e2e_1_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".gestalt/memories")).expect("mem");
    fs::create_dir_all(dir.join(".gestalt/skills/triage")).expect("skills");
    fs::create_dir_all(dir.join("src")).expect("src");
    fs::write(dir.join("AGENTS.md"), "Use cargo test.\n").expect("agents");
    fs::write(dir.join(".gestalt/memories/MEMORY.md"), "v1 memory\n").expect("mem");
    fs::write(
        dir.join(".gestalt/skills/triage/SKILL.md"),
        "---\nname: triage\ndescription: Quick triage\n---\nHIDDEN BODY",
    )
    .expect("skill");
    fs::write(dir.join("src/lib.rs"), "pub fn val() -> i32 { 1 }\n").expect("lib");
    let canon = fs::canonicalize(&dir).expect("canon");

    let sse_edit = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":100}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_edit\",\"name\":\"edit\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"src/lib.rs\\\",\\\"target\\\":\\\"1\\\",\\\"replacement\\\":\\\"2\\\"}\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":20}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ).to_string();
    let sse_done_1 = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":120}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Updated lib.rs\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":10}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ).to_string();
    let sse_done_2 = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":130}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Turn 2 complete\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":10}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ).to_string();

    let (addr, captured_reqs) = spawn_mock_http_server(vec![
        ("text/event-stream", sse_edit),
        ("text/event-stream", sse_done_1),
        ("text/event-stream", sse_done_2),
    ])
    .await;

    let mut reg = ExtensionRegistry::new();
    StdExtension.register(&mut reg).expect("std");
    StdToolsExtension.register(&mut reg).expect("tools");

    let mut gate = PolicyGate::new(Default::default(), None, default_std_sandbox_policy());
    let ws_toml = canon.join(".gestalt/gestalt.toml");
    fs::write(
        &ws_toml,
        format!(
            "[providers.default_llm]\nadapter = \"anthropic\"\nendpoint = \"http://{addr}/v1/messages\"\nmodel = \"claude-sonnet-4-5\"\n"
        ),
    )
    .expect("toml");
    let resolved = load_and_merge_config(
        ConfigLoadOptions {
            workspace_root: &canon,
            global_toml_path: None,
            workspace_toml_path: Some(&ws_toml),
            cli_extensions: BTreeMap::new(),
            trust_store: &WorkspaceTrustStore::default(),
            auto_trust_workspace: true,
        },
        &mut gate,
        &mut reg,
    )
    .expect("load config");

    let mut creds = CredentialStore::default();
    creds.insert_runtime_secret("anthropic", "sk-ant-mock-secret");
    let creds_arc = Arc::new(Mutex::new(creds.clone()));

    let mut runtime = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
    runtime.providers.merge_configs(resolved.providers);
    register_std_providers(&mut runtime.providers);
    let bg_tasks = register_std_tools(&mut runtime);
    register_std_commands(&mut runtime, creds_arc);

    let sink = Arc::new(CollectingSink::default());
    let session_path = canon.join(".gestalt/sessions/e2e.jsonl");
    let mut host = RuntimeHost::new(
        "run_e2e_1".into(),
        &canon,
        session_path.clone(),
        reg,
        gate,
        runtime,
        creds,
        Some(sink.clone()),
        Arc::new(ApprovingPauseHandler),
    )
    .expect("host");

    host.set_tracked_artifact_paths(discover_tracked_artifact_paths(&canon));
    let out1 = host
        .run_turn(json!("change 1 to 2"), "swe", &[], CancellationToken::new())
        .await
        .expect("turn 1");
    assert!(matches!(out1, TurnOutcome::Completed { .. }));
    assert_eq!(
        fs::read_to_string(canon.join("src/lib.rs")).expect("read"),
        "pub fn val() -> i32 { 2 }\n"
    );

    // Mutate volatile MEMORY.md before Turn 2 and send intra-turn steering
    fs::write(
        canon.join(".gestalt/memories/MEMORY.md"),
        "v2 memory updated\n",
    )
    .expect("mem2");
    host.set_tracked_artifact_paths(discover_tracked_artifact_paths(&canon));
    let (tx, rx) = mpsc::unbounded_channel();
    let _ = tx.send(LiveSteeringMessage {
        kind: SteeringKind::Steer,
        content: json!("be brief"),
        source: SteeringSource::User,
    });
    let out2 = host
        .run_turn_with_steering(
            json!("verify"),
            "swe",
            &[],
            CancellationToken::new(),
            Some(rx),
        )
        .await
        .expect("turn 2");
    assert!(matches!(out2, TurnOutcome::Completed { .. }));

    // Verify diff_preview ToolUi event emitted
    {
        let events = sink.events.lock().expect("events");
        assert!(events.iter().any(|e| matches!(
            e,
            RunEvent::ToolUi { renderer, .. } if renderer == "diff_preview"
        )));
    }

    // Verify Tier 1 (system[0]), Tier 2 (system[1]), and tools are byte-identical across all 3 LLM calls!
    {
        let reqs = captured_reqs.lock().expect("reqs");
        assert_eq!(reqs.len(), 3);
        let t1_0 = serde_json::to_string(&reqs[0]["system"][0]).expect("s0");
        let t2_0 = serde_json::to_string(&reqs[0]["system"][1]).expect("s1");
        let tools_0 = serde_json::to_string(&reqs[0]["tools"]).expect("tools0");
        for req in &reqs[1..] {
            assert_eq!(serde_json::to_string(&req["system"][0]).expect("s0"), t1_0);
            assert_eq!(serde_json::to_string(&req["system"][1]).expect("s1"), t2_0);
            assert_eq!(
                serde_json::to_string(&req["tools"]).expect("tools"),
                tools_0
            );
        }
    }

    // Verify hermetic offline replay achieves 100% parity
    let jsonl = fs::read_to_string(&session_path).expect("jsonl");
    let (tree, warn) = SessionTree::from_jsonl_str(&jsonl).expect("parse");
    assert!(warn.is_none());
    let report = ReplayEngine::new(host.kernel().registry(), host.kernel().policy_gate())
        .replay(&tree, ReplayMode::Default, |digest| {
            host.blob_store().get_utf8(digest).ok()
        })
        .expect("replay");
    assert!(report.is_parity(), "divergences: {:?}", report.divergences);

    assert_eq!(bg_tasks.reap_all(), 0);
    host.shutdown().await;
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn decision_router_triage_escalation_with_goal_check_and_curator_rollback() {
    let dir = std::env::temp_dir().join(format!("gestalt_std_e2e_2_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join(".gestalt/memories")).expect("mem");
    let canon = fs::canonicalize(&dir).expect("canon");

    let jev_low_conf = json!({
        "choice": "escalate",
        "confidence": 0.42,
        "latency_ms": 28,
        "usage": { "input_tokens": 20, "output_tokens": 5 }
    })
    .to_string();
    let sse_chat_1 = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":50}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Initial attempt\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":8}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ).to_string();
    let jev_goal_fail = json!({
        "choice": "fail",
        "confidence": 0.90,
        "latency_ms": 18
    })
    .to_string();
    let sse_chat_2 = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":60}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Verified attempt\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":8}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ).to_string();
    let jev_goal_pass = json!({
        "choice": "pass",
        "confidence": 0.95,
        "latency_ms": 15
    })
    .to_string();

    let (addr, _) = spawn_mock_http_server(vec![
        ("application/json", jev_low_conf),
        ("text/event-stream", sse_chat_1),
        ("application/json", jev_goal_fail),
        ("text/event-stream", sse_chat_2),
        ("application/json", jev_goal_pass),
    ])
    .await;

    let mut reg = ExtensionRegistry::new();
    StdExtension.register(&mut reg).expect("std");
    StdToolsExtension.register(&mut reg).expect("tools");
    reg.presets.insert(
        "swe_goal_curated".into(),
        gestalt_kernel::Preset {
            base_loop: "chat".into(),
            decorators: vec!["goal_check".into(), "curator".into()],
            builder: "standard".into(),
            config: LoopConfig {
                params: json!({
                    "goal_criteria": "Must verify output",
                    "decision_model": "fast_dec",
                    "max_goal_retries": 3,
                    "curator_candidate": {
                        "artifact": ".gestalt/memories/MEMORY.md",
                        "patch": { "patch_kind": "write_file", "content": "Curated lesson\n" }
                    }
                }),
                ..LoopConfig::default()
            },
        },
    );
    if let Some(triage) = reg.presets.get_mut("triage") {
        triage.config.params = json!({
            "confidence_threshold": 0.85,
            "fallback_preset": "swe_goal_curated",
            "decision_model": "fast_dec",
        });
    }

    let mut gate = PolicyGate::new(Default::default(), None, default_std_sandbox_policy());
    let ws_toml = canon.join(".gestalt/gestalt.toml");
    fs::write(
        &ws_toml,
        format!(
            "[providers.fast_dec]\nadapter = \"jev\"\nendpoint = \"http://{addr}/v1/decision\"\nmodel = \"jev-1.13.0\"\n\n[providers.default_llm]\nadapter = \"anthropic\"\nendpoint = \"http://{addr}/v1/messages\"\nmodel = \"claude-sonnet-4-5\"\n"
        ),
    )
    .expect("toml");
    let resolved = load_and_merge_config(
        ConfigLoadOptions {
            workspace_root: &canon,
            global_toml_path: None,
            workspace_toml_path: Some(&ws_toml),
            cli_extensions: BTreeMap::new(),
            trust_store: &WorkspaceTrustStore::default(),
            auto_trust_workspace: true,
        },
        &mut gate,
        &mut reg,
    )
    .expect("load config");

    let mut creds = CredentialStore::default();
    creds.insert_runtime_secret("jev", "jev-secret");
    creds.insert_runtime_secret("fast_dec", "jev-secret");
    creds.insert_runtime_secret("anthropic", "ant-secret");

    let mut runtime = RuntimeRegistry::new(Arc::new(UnsandboxedFallbackDriver));
    runtime.providers.merge_configs(resolved.providers);
    register_std_providers(&mut runtime.providers);
    let _ = register_std_tools(&mut runtime);

    let session_path = canon.join(".gestalt/sessions/e2e_2.jsonl");
    let mut host = RuntimeHost::new(
        "run_e2e_2".into(),
        &canon,
        session_path.clone(),
        reg,
        gate,
        runtime,
        creds,
        None,
        Arc::new(ApprovingPauseHandler),
    )
    .expect("host");

    let out = host
        .run_turn(
            json!("solve and remember"),
            "triage",
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("turn");
    match out {
        TurnOutcome::Completed { output, .. } => {
            assert_eq!(output["curator"]["promoted"], true);
            assert_eq!(output["output"]["goal_verified"], true);
            assert_eq!(output["output"]["attempts"], 2);
        }
        other => panic!("expected Completed, got {other:?}"),
    }

    assert_eq!(
        fs::read_to_string(canon.join(".gestalt/memories/MEMORY.md")).expect("mem"),
        "Curated lesson\n"
    );
    let ledger = host.artifacts().read_ledger().expect("ledger");
    assert_eq!(ledger.len(), 1);
    host.rollback_stage(&ledger[0].stage_id).expect("rollback");
    assert!(!canon.join(".gestalt/memories/MEMORY.md").exists());

    // Offline replay parity check (triage -> switch -> goal_check + curator)
    let jsonl = fs::read_to_string(&session_path).expect("jsonl");
    let (tree, warn) = SessionTree::from_jsonl_str(&jsonl).expect("parse");
    assert!(warn.is_none());
    let report = ReplayEngine::new(host.kernel().registry(), host.kernel().policy_gate())
        .replay(&tree, ReplayMode::Default, |d| {
            host.blob_store().get_utf8(d).ok()
        })
        .expect("replay");
    assert!(report.is_parity(), "divergences: {:?}", report.divergences);

    host.shutdown().await;
    let _ = fs::remove_dir_all(&dir);
}
