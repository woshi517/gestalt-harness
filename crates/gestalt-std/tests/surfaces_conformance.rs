#![forbid(unsafe_code)]
#![cfg(all(
    feature = "loops",
    feature = "tools",
    feature = "providers",
    feature = "surfaces"
))]
//! End-to-end conformance suite for all 5 `gestalt-std` surfaces (`print`,
//! `replay`, `rpc`, `acp`, and `tui`) wired to `StdExtension`, `StdToolsExtension`,
//! `AnthropicProvider`, and `RuntimeHost` (Milestone 1.3b).

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gestalt_kernel::{Extension, ExtensionRegistry, PolicyGate, ReplayMode, RunEvent};
use gestalt_runtime::sandbox::UnsandboxedFallbackDriver;
use gestalt_runtime::{
    CancellationToken, ConfigLoadOptions, CredentialStore, HeadlessPauseHandler, RuntimeHost,
    RuntimeRegistry, WorkspaceTrustStore, load_and_merge_config,
};
use gestalt_std::surfaces::{
    PrintOptions, RPC_METHOD_NOT_FOUND, ReplaySurfaceOptions, SurfaceExitCode, run_acp_surface,
    run_print_surface, run_replay_surface, run_rpc_surface,
};
use gestalt_std::{
    StdExtension, StdToolsExtension, default_std_sandbox_policy, discover_tracked_artifact_paths,
    register_std_commands, register_std_providers, register_std_tools,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

async fn spawn_mock_sse_server(responses: Vec<String>) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let queue = Arc::new(Mutex::new(VecDeque::from(responses)));

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
                    break;
                }
            }

            let body = queue
                .lock()
                .ok()
                .and_then(|mut q| q.pop_front())
                .unwrap_or_else(|| "data: {}\n\n".to_string());
            let http = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(http.as_bytes()).await;
        }
    });

    addr
}

async fn spawn_stalling_sse_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");

    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let _ = sock
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                .await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    addr
}

fn setup_std_host(
    dir: &std::path::Path,
    addr: std::net::SocketAddr,
    session_name: &str,
) -> (RuntimeHost, ExtensionRegistry, PolicyGate) {
    fs::create_dir_all(dir.join(".gestalt/memories")).expect("mem");
    fs::create_dir_all(dir.join(".gestalt/skills")).expect("skills");
    fs::create_dir_all(dir.join("src")).expect("src");
    fs::write(dir.join("src/lib.rs"), "pub fn num() -> i32 { 1 }\n").expect("lib");
    let canon = fs::canonicalize(dir).expect("canon");

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
    let _bg = register_std_tools(&mut runtime);
    register_std_commands(&mut runtime, creds_arc);

    let replay_reg = {
        let mut r = ExtensionRegistry::new();
        StdExtension.register(&mut r).expect("std");
        StdToolsExtension.register(&mut r).expect("tools");
        r
    };
    let replay_gate = PolicyGate::new(Default::default(), None, default_std_sandbox_policy());

    let mut host = RuntimeHost::new(
        format!("run_{session_name}"),
        &canon,
        canon.join(format!(".gestalt/sessions/{session_name}.jsonl")),
        reg,
        gate,
        runtime,
        creds,
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");
    host.set_tracked_artifact_paths(discover_tracked_artifact_paths(&canon));
    (host, replay_reg, replay_gate)
}

fn sse_edit_and_done_pair() -> Vec<String> {
    let sse_edit = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu_1\",\"name\":\"edit\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"src/lib.rs\\\",\\\"target\\\":\\\"1\\\",\\\"replacement\\\":\\\"2\\\"}\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ).to_string();
    let sse_done = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":20}}}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Checked the edit and verified the change in full across the workspace so that streaming redaction buffer flushes cleanly even when long host environment secrets are present in the process.\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\"}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Updated src/lib.rs from 1 to 2 successfully and verified all edits and formatting in the workspace with zero regressions or issues remaining.\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":8}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ).to_string();
    vec![sse_edit, sse_done]
}

#[tokio::test]
async fn print_e2e_streams_ndjson_and_replay_surface_verifies_parity() {
    let dir = std::env::temp_dir().join(format!("gestalt_surf_e2e_print_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let addr = spawn_mock_sse_server(sse_edit_and_done_pair()).await;
    let (mut host, replay_reg, replay_gate) = setup_std_host(&dir, addr, "print");
    let session_path = host.session_path().to_path_buf();
    let ws_root = host.workspace_root().to_path_buf();

    let mut out = Vec::new();
    let res = run_print_surface(
        &mut host,
        PrintOptions {
            prompt: json!("change 1 to 2"),
            preset: "swe".into(),
            extra_decorators: Vec::new(),
            json_mode: true,
            headless_auto_approve: Vec::new(),
        },
        &mut out,
        CancellationToken::new(),
    )
    .await;
    assert_eq!(res.exit_code, SurfaceExitCode::Success);
    let events: Vec<RunEvent> = String::from_utf8(out)
        .expect("utf8")
        .lines()
        .map(|l| serde_json::from_str(l).expect("RunEvent"))
        .collect();
    assert!(events.iter().any(|e| matches!(
        e,
        RunEvent::ToolUi { renderer, .. } if renderer == "diff_preview"
    )));
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunEvent::StreamDelta { channel, .. } if channel == "thought"
        )),
        "events: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, RunEvent::TurnCompleted { .. }))
    );
    drop(host);

    let mut replay_out = Vec::new();
    let replay_res = run_replay_surface(
        ReplaySurfaceOptions {
            session_path: &session_path,
            workspace_root: &ws_root,
            mode: ReplayMode::Default,
            json_mode: true,
            ext_opts: None,
        },
        &replay_reg,
        &replay_gate,
        &mut replay_out,
    )
    .await;
    assert_eq!(replay_res.exit_code, SurfaceExitCode::Success);
    assert!(replay_res.report.expect("report").is_parity());

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rpc_e2e_parity_commands_and_fast_mid_turn_cancel() {
    let dir = std::env::temp_dir().join(format!("gestalt_surf_e2e_rpc_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let addr = spawn_stalling_sse_server().await;
    let (host, _, _) = setup_std_host(&dir, addr, "rpc");

    let (cin, sin) = tokio::io::duplex(8192);
    let (sout, cout) = tokio::io::duplex(8192);
    let server = tokio::spawn(async move {
        run_rpc_surface(host, "swe".into(), BufReader::new(sin), sout).await
    });

    let mut cin = cin;
    let mut cout = BufReader::new(cout).lines();

    // 1. surface/describe & command/run (/gate)
    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"surface/describe\"}\n")
        .await
        .unwrap();
    let desc: Value = serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
    assert!(desc["result"]["ui"]["panels"]["policy_gate"].is_object());

    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"command/run\",\"params\":{\"command\":\"gate\"}}\n")
        .await
        .unwrap();
    let gate_res: Value = serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(gate_res["result"]["command"], "gate");

    // /goal merges goal_criteria into active preset params, and /login emits non-blocking pause
    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":10,\"method\":\"command/run\",\"params\":{\"command\":\"goal\",\"args\":\"cargo test passes\"}}\n")
        .await
        .unwrap();
    let goal_res: Value = serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(goal_res["result"]["command"], "goal");

    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":11,\"method\":\"command/run\",\"params\":{\"command\":\"login\",\"args\":\"mock_rpc_provider\"}}\n")
        .await
        .unwrap();
    let mut login_pause_id = String::new();
    let mut saw_login_cmd_ok = false;
    while !(saw_login_cmd_ok && !login_pause_id.is_empty()) {
        let v: Value = serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
        if v["method"] == "event/notify" && v["params"]["event"] == "paused" {
            login_pause_id = v["params"]["pause_id"].as_str().unwrap().to_string();
        } else if v["id"] == 11 {
            saw_login_cmd_ok = true;
        }
    }
    let resolve_login = json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "pause/resolve",
        "params": { "pause_id": login_pause_id, "resolution": { "secret": "sk-rpc-login-secret" } }
    });
    cin.write_all(format!("{resolve_login}\n").as_bytes())
        .await
        .unwrap();
    let login_done: Value =
        serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(login_done["result"]["resolved"], true);
    assert_eq!(login_done["result"]["data"]["stored"], true);
    assert!(!login_done.to_string().contains("sk-rpc-login-secret"));

    // 2. Unknown method -> -32601
    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"nope\"}\n")
        .await
        .unwrap();
    let err_res: Value = serde_json::from_str(&cout.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(err_res["error"]["code"], RPC_METHOD_NOT_FOUND);

    // 3. Start Turn against stalling SSE, then turn/cancel -> must abort <= 250 ms
    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"turn/start\",\"params\":{\"prompt\":\"hang\"}}\n")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    let t0 = Instant::now();
    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"turn/cancel\"}\n")
        .await
        .unwrap();

    let mut saw_abort = false;
    while let Ok(Some(line)) = tokio::time::timeout(Duration::from_secs(2), cout.next_line())
        .await
        .unwrap()
    {
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["id"] == 4 {
            assert_eq!(v["result"]["status"], "aborted");
            saw_abort = true;
            break;
        }
    }
    assert!(saw_abort);
    assert!(t0.elapsed() <= Duration::from_millis(250));

    drop(cin);
    let _ = server.await.unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn acp_e2e_prompt_updates_slash_mode_and_cancel() {
    let dir = std::env::temp_dir().join(format!("gestalt_surf_e2e_acp_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let addr = spawn_mock_sse_server(sse_edit_and_done_pair()).await;
    let (host, _, _) = setup_std_host(&dir, addr, "acp");

    let (cin, sin) = tokio::io::duplex(8192);
    let (sout, cout) = tokio::io::duplex(8192);
    let server = tokio::spawn(async move {
        run_acp_surface(host, "swe".into(), BufReader::new(sin), sout).await
    });

    let mut cin = cin;
    let mut cout = BufReader::new(cout).lines();

    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n")
        .await
        .unwrap();
    let _ = cout.next_line().await.unwrap().unwrap();

    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/set_mode\",\"params\":{\"modeId\":\"general\"}}\n")
        .await
        .unwrap();
    let mut mode_updated = false;
    while let Some(line) = cout.next_line().await.unwrap() {
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["params"]["update"]["sessionUpdate"] == "current_mode_update" {
            assert_eq!(v["params"]["update"]["currentModeId"], "general");
            mode_updated = true;
        }
        if v["id"] == 2 {
            break;
        }
    }
    assert!(mode_updated);

    // Switch back to swe via slash prefix in session/prompt
    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/prompt\",\"params\":{\"prompt\":[{\"type\":\"text\",\"text\":\"/mode swe\"}]}}\n")
        .await
        .unwrap();
    while let Some(line) = cout.next_line().await.unwrap() {
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["id"] == 3 {
            assert_eq!(v["result"]["stopReason"], "end_turn");
            break;
        }
    }

    // Real prompt -> thought chunk + message chunk + tool_call_update with gestalt_tool_ui
    cin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"session/prompt\",\"params\":{\"prompt\":[{\"type\":\"text\",\"text\":\"edit lib.rs\"}]}}\n")
        .await
        .unwrap();
    let mut saw_thought = false;
    let mut saw_text = false;
    let mut saw_diff_ui = false;
    let mut seen_updates = Vec::new();
    while let Ok(Some(line)) = tokio::time::timeout(Duration::from_secs(2), cout.next_line())
        .await
        .unwrap()
    {
        let v: Value = serde_json::from_str(&line).unwrap();
        seen_updates.push(line.clone());
        if v["method"] == "session/update" {
            let upd = &v["params"]["update"];
            match upd["sessionUpdate"].as_str() {
                Some("agent_thought_chunk") => saw_thought = true,
                Some("agent_message_chunk") => saw_text = true,
                Some("tool_call_update")
                    if upd["_meta"]["gestalt_tool_ui"]["renderer"] == "diff_preview" =>
                {
                    saw_diff_ui = true;
                }
                _ => {}
            }
        } else if v["id"] == 4 {
            assert_eq!(v["result"]["stopReason"], "end_turn");
            break;
        }
    }
    assert!(saw_thought, "missing thought in: {seen_updates:?}");
    assert!(saw_text, "missing text in: {seen_updates:?}");
    assert!(saw_diff_ui, "missing diff_ui in: {seen_updates:?}");

    drop(cin);
    let _ = server.await.unwrap();
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(feature = "tui")]
#[test]
fn tui_e2e_renders_std_ui_surface_and_inspector_panels() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use gestalt_std::surfaces::tui::{TuiAppState, TuiComponentRegistry, render_tui};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut reg = ExtensionRegistry::new();
    StdExtension.register(&mut reg).expect("std");
    StdToolsExtension.register(&mut reg).expect("tools");

    let mut state = TuiAppState::new("gestalt-harness", "swe");
    state.sync_from_registry(
        &reg,
        Default::default(),
        json!({ "rule_ids": ["baseline"] }),
    );
    state.apply_run_event(&RunEvent::StreamDelta {
        turn_id: "t1".into(),
        phase_id: "p1".into(),
        effect_id: "e1".into(),
        channel: "text".into(),
        payload: json!({ "text": "Applied edit." }),
    });
    state.apply_run_event(&RunEvent::ToolUi {
        turn_id: "t1".into(),
        effect_id: "e2".into(),
        renderer: "diff_preview".into(),
        data: json!({
            "path": "src/lib.rs",
            "additions": 1,
            "deletions": 1,
            "unified_diff": "-1\n+2",
        }),
    });

    // Open Ctrl+P inspector panel overlay
    state.handle_key_event(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).expect("term");
    let comp_reg = TuiComponentRegistry::default();
    terminal
        .draw(|f| render_tui(f, &mut state, &comp_reg))
        .expect("draw");

    let buf = terminal.backend().buffer();
    let mut screen = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            screen.push_str(buf[(x, y)].symbol());
        }
        screen.push('\n');
    }
    assert!(screen.contains("PolicyGate"));
    assert!(screen.contains("Budget Meter"));
}
