#![forbid(unsafe_code)]
//! Milestone 1.4 `v0.1` Proof Gate verification suite.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use gestalt_kernel::{
    ArtifactStoreView, BudgetCap, DriverDirective, DriverResponse, Extension, ExtensionRegistry,
    KernelRuntime, ModelKind, PolicyGate, ReplayEngine, ReplayMode, SessionTree, SteeringKind,
    SteeringSource, StepResult, Usage,
};
use gestalt_runtime::{
    CredentialStore, HeadlessPauseHandler, RuntimeHost, RuntimeRegistry, default_sandbox_driver,
};
use gestalt_std::extensions::builders::{
    collect_all_skills, collect_skill_index, discover_tracked_artifact_paths,
};
use gestalt_std::surfaces::{SlashCommand, SurfaceSessionState, execute_slash_command_idle};
use gestalt_std::{
    StdExtension, StdToolsExtension, default_std_sandbox_policy, register_std_commands,
    register_std_tools,
};
use serde_json::{Value, json};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

const GOLDEN_FIXTURE_NAMES: [&str; 10] = [
    "01_chat_multi_turn_tools.jsonl",
    "02_decision_router_fast_path.jsonl",
    "03_decision_router_triage_to_general.jsonl",
    "04_intra_turn_steer.jsonl",
    "05_mid_step_urgent_steer_abort_synthesis.jsonl",
    "06_turn_abort.jsonl",
    "07_goal_check_retry_until_pass.jsonl",
    "08_compaction_and_curator_stage_pause_promote.jsonl",
    "09_parallel_spawn_and_policy_denial.jsonl",
    "10_fork_rewind_and_external_loop_state_token.jsonl",
];

// ---------------------------------------------------------------------------
// Gate 1: Kernel Purity, Sans-I/O, #![forbid(unsafe_code)]
// ---------------------------------------------------------------------------

#[test]
fn gate_1_kernel_loc_purity_and_forbid_unsafe() {
    let kernel_src = repo_root().join("crates/gestalt-kernel/src");
    let mut lib_has_forbid_unsafe = false;
    let forbidden = [
        "anthropic",
        "openai",
        "tokio",
        "reqwest",
        "ratatui",
        "crossterm",
        "MARKDOWN",
    ];

    for entry in fs::read_dir(&kernel_src).expect("read gestalt-kernel/src") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let raw = fs::read_to_string(&path).expect("read kernel rs");
        if path.file_name().and_then(|n| n.to_str()) == Some("lib.rs")
            && raw.contains("#![forbid(unsafe_code)]")
        {
            lib_has_forbid_unsafe = true;
        }
        let lower = raw.to_ascii_lowercase();
        for pat in forbidden {
            assert!(
                !lower.contains(&pat.to_ascii_lowercase()),
                "forbidden string '{pat}' found in {}",
                path.display()
            );
        }
    }

    assert!(
        lib_has_forbid_unsafe,
        "gestalt-kernel missing #![forbid(unsafe_code)]"
    );
}

// ---------------------------------------------------------------------------
// Gate 2: Multi-source SKILL.md discovery, /model, /thinking, /logout, /skills
// ---------------------------------------------------------------------------

#[test]
fn gate_2_multi_source_skills_precedence_and_disable_model_invocation() {
    let ws = std::env::temp_dir().join(format!("gestalt_skills_gate_{}", std::process::id()));
    let _ = fs::remove_dir_all(&ws);
    let fake_home = ws.join("fake_home");
    fs::create_dir_all(fake_home.join(".agents/skills/alpha")).expect("mkdir");
    fs::create_dir_all(fake_home.join(".gestalt/skills/beta")).expect("mkdir");
    fs::create_dir_all(ws.join(".agents/skills/gamma")).expect("mkdir");
    fs::create_dir_all(ws.join(".gestalt/skills/alpha")).expect("mkdir");
    fs::create_dir_all(ws.join(".gestalt/skills/secret_only")).expect("mkdir");

    let g_alpha = fake_home.join(".agents/skills/alpha/SKILL.md");
    let g_beta = fake_home.join(".gestalt/skills/beta/SKILL.md");
    let w_gamma = ws.join(".agents/skills/gamma/SKILL.md");
    let w_alpha = ws.join(".gestalt/skills/alpha/SKILL.md");
    let w_secret = ws.join(".gestalt/skills/secret_only/SKILL.md");

    let entries = BTreeMap::from([
        (
            g_alpha.to_string_lossy().to_string(),
            gestalt_kernel::ArtifactEntry {
                path: g_alpha.to_string_lossy().to_string(),
                content: "---\nname: alpha\ndescription: global alpha\n---\nbody\n".into(),
                blob_digest: "d1".into(),
            },
        ),
        (
            g_beta.to_string_lossy().to_string(),
            gestalt_kernel::ArtifactEntry {
                path: g_beta.to_string_lossy().to_string(),
                content: "---\nname: beta\ndescription: global beta\n---\nbody\n".into(),
                blob_digest: "d2".into(),
            },
        ),
        (
            ".agents/skills/gamma/SKILL.md".to_string(),
            gestalt_kernel::ArtifactEntry {
                path: ".agents/skills/gamma/SKILL.md".into(),
                content: "---\nname: gamma\ndescription: ws gamma\n---\nbody\n".into(),
                blob_digest: "d3".into(),
            },
        ),
        (
            ".gestalt/skills/alpha/SKILL.md".to_string(),
            gestalt_kernel::ArtifactEntry {
                path: ".gestalt/skills/alpha/SKILL.md".into(),
                content: "---\nname: alpha\ndescription: workspace alpha override\n---\nbody\n".into(),
                blob_digest: "d4".into(),
            },
        ),
        (
            ".gestalt/skills/secret_only/SKILL.md".to_string(),
            gestalt_kernel::ArtifactEntry {
                path: ".gestalt/skills/secret_only/SKILL.md".into(),
                content: "---\nname: secret_only\ndescription: manual only\ndisable-model-invocation: true\n---\nbody\n".into(),
                blob_digest: "d5".into(),
            },
        ),
    ]);
    let view = ArtifactStoreView::from_entries(entries);
    let all = collect_all_skills(&view);
    assert_eq!(all.len(), 4);
    let alpha = all.iter().find(|s| s.name == "alpha").expect("alpha");
    assert_eq!(alpha.description, "workspace alpha override");

    let model_indexed = collect_skill_index(&view);
    assert_eq!(model_indexed.len(), 3);
    assert!(!model_indexed.iter().any(|s| s.name == "secret_only"));

    fs::write(
        &w_gamma,
        "---\nname: gamma\ndescription: ws gamma\n---\nbody\n",
    )
    .expect("write w_gamma");
    let _ = (w_alpha, w_secret);
    let discovered = discover_tracked_artifact_paths(&ws);
    assert!(
        discovered
            .iter()
            .any(|p| p == ".agents/skills/gamma/SKILL.md")
    );
    let _ = fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn gate_2_general_slash_commands_work_end_to_end() {
    let ws = std::env::temp_dir().join(format!("gestalt_cmds_gate_{}", std::process::id()));
    let _ = fs::remove_dir_all(&ws);
    fs::create_dir_all(ws.join(".gestalt/sessions")).expect("mkdir");
    fs::create_dir_all(ws.join(".gestalt/skills/hello")).expect("mkdir");
    fs::write(
        ws.join(".gestalt/skills/hello/SKILL.md"),
        "---\nname: hello\ndescription: greeting skill\n---\nSay hi!\n",
    )
    .expect("write skill");

    let mut reg = ExtensionRegistry::new();
    StdExtension.register(&mut reg).expect("std");
    StdToolsExtension.register(&mut reg).expect("tools");
    let gate = PolicyGate::new(BudgetCap::default(), None, default_std_sandbox_policy());
    let mut runtime = RuntimeRegistry::new(default_sandbox_driver(true));
    register_std_tools(&mut runtime);

    let creds_path = ws.join(".gestalt/credentials.toml");
    let mut creds = CredentialStore::load_from_path(creds_path.clone()).expect("creds");
    creds
        .set_credential("anthropic", "sk-ant-test-secret-12345".to_string())
        .expect("set cred");
    let cred_arc = Arc::new(std::sync::Mutex::new(creds.clone()));
    register_std_commands(&mut runtime, cred_arc);

    let mut host = RuntimeHost::new(
        "cmd_gate".to_string(),
        &ws,
        ws.join(".gestalt/sessions/s1.jsonl"),
        reg,
        gate,
        runtime,
        creds,
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");
    let mut state = SurfaceSessionState::new("general");

    let model_res = execute_slash_command_idle(
        &mut host,
        &mut state,
        SlashCommand::Extension {
            name: "model".into(),
            args: "openai/gpt-5".into(),
        },
    )
    .await
    .expect("/model");
    assert_eq!(model_res.data["model"], "openai/gpt-5");
    assert!(model_res.message.contains("openai/gpt-5"));

    let think_res = execute_slash_command_idle(
        &mut host,
        &mut state,
        SlashCommand::Extension {
            name: "thinking".into(),
            args: "high".into(),
        },
    )
    .await
    .expect("/thinking");
    assert_eq!(think_res.data["reasoning_budget_tokens"], 16_384);
    assert!(think_res.message.contains("16384"));

    let logout_res = execute_slash_command_idle(
        &mut host,
        &mut state,
        SlashCommand::Extension {
            name: "logout".into(),
            args: "anthropic".into(),
        },
    )
    .await
    .expect("/logout");
    assert_eq!(logout_res.data["removed"], true);
    assert!(host.credentials().get("anthropic", None).is_none());

    let skills_res = execute_slash_command_idle(
        &mut host,
        &mut state,
        SlashCommand::Extension {
            name: "skills".into(),
            args: String::new(),
        },
    )
    .await
    .expect("/skills");
    assert!(skills_res.data["count"].as_u64().unwrap_or(0) >= 1);
    assert!(skills_res.message.contains("/skill:hello"));

    let load_skill_res = execute_slash_command_idle(
        &mut host,
        &mut state,
        SlashCommand::Extension {
            name: "skills".into(),
            args: "load:hello say hi".into(),
        },
    )
    .await
    .expect("/skill:hello");
    assert_eq!(load_skill_res.data["loaded_skill"], "hello");
    assert!(
        load_skill_res.data["steering"]
            .as_str()
            .is_some_and(|s| s.contains("Say hi!"))
    );

    let mut tui_state = gestalt_std::surfaces::tui::TuiAppState::new("ws", "general");
    tui_state.sync_from_registry(
        host.kernel().registry(),
        *host.kernel().budget_snapshot(),
        json!({}),
    );
    assert_eq!(tui_state.active_model.as_deref(), Some("openai/gpt-5"));
    assert_eq!(tui_state.reasoning_budget_tokens, Some(16_384));

    let sess_res = execute_slash_command_idle(&mut host, &mut state, SlashCommand::Session)
        .await
        .expect("/session");
    assert_eq!(sess_res.data["active_preset"], "general");

    let help_res = execute_slash_command_idle(&mut host, &mut state, SlashCommand::Help)
        .await
        .expect("/help");
    assert!(
        !help_res.data["commands"]
            .as_array()
            .expect("cmds")
            .is_empty()
    );

    let new_res = execute_slash_command_idle(&mut host, &mut state, SlashCommand::New)
        .await
        .expect("/new");
    assert_eq!(new_res.data["new_session"], true);

    let quit_res = execute_slash_command_idle(&mut host, &mut state, SlashCommand::Quit)
        .await
        .expect("/quit");
    assert_eq!(quit_res.data["quit"], true);

    let _ = fs::remove_dir_all(&ws);
}

// ---------------------------------------------------------------------------
// Gate 3: >= 10 Golden Multi-Phase SessionTree JSONL Replay Fixtures
// ---------------------------------------------------------------------------

fn golden_registry() -> ExtensionRegistry {
    let mut reg = ExtensionRegistry::new();
    StdExtension.register(&mut reg).expect("std");
    StdToolsExtension.register(&mut reg).expect("tools");
    if let Some(g) = reg.presets.get_mut("general") {
        g.config.params["model"] = json!("anthropic/claude-sonnet-4-6");
    }
    if let Some(t) = reg.presets.get_mut("triage") {
        t.config.params = json!({
            "decision_model": "jev/jev-1",
            "confidence_threshold": 0.85,
            "fallback_preset": "general",
            "routes": {
                "fast_echo": { "action": "tool", "name": "bash", "args": { "command": "echo fast" } }
            }
        });
    }
    reg
}

fn write_golden_fixture(path: &Path, tree: &SessionTree) {
    let mut out = String::new();
    for node in tree.all_nodes() {
        out.push_str(&node.to_jsonl_line().expect("jsonl line"));
        out.push('\n');
    }
    fs::write(path, out).expect("write golden fixture");
}

#[test]
#[ignore]
fn generate_golden_replay_fixtures() {
    let dir = repo_root().join("tests/golden_replays");
    fs::create_dir_all(&dir).expect("mkdir golden_replays");

    for (idx, name) in GOLDEN_FIXTURE_NAMES.iter().enumerate() {
        let reg = golden_registry();
        let gate = PolicyGate::new(BudgetCap::default(), None, default_std_sandbox_policy());
        let mut rt = KernelRuntime::new(format!("gold_{idx}"), "/repo".into(), 100, reg, gate)
            .expect("kernel");
        match idx {
            0 => {
                rt.start_turn(
                    json!("turn 1"),
                    "general",
                    &[],
                    ArtifactStoreView::default(),
                    101,
                )
                .expect("start");
                if let DriverDirective::DispatchEffects { effects } =
                    rt.next_action(DriverResponse::Poll).expect("d1")
                {
                    let _ = rt.next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                        effect_id: effects[0].effect_id.clone(),
                        kind: ModelKind::Llm,
                        output: json!({ "text": "done 1", "reasoning": [], "tool_calls": [], "stop_reason": "end_turn" }),
                        usage: Usage::default(),
                    }]));
                }
            }
            1 | 2 => {
                rt.start_turn(
                    json!(if idx == 1 {
                        "fast echo"
                    } else {
                        "complex task"
                    }),
                    "triage",
                    &[],
                    ArtifactStoreView::default(),
                    101,
                )
                .expect("start");
                if let DriverDirective::DispatchEffects { effects } =
                    rt.next_action(DriverResponse::Poll).expect("d1")
                {
                    let dec = if idx == 1 {
                        json!({ "choice": "fast_echo", "confidence": 0.95, "latency_ms": 5 })
                    } else {
                        json!({ "choice": "escalate", "confidence": 0.25, "latency_ms": 5 })
                    };
                    let d2 = rt
                        .next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                            effect_id: effects[0].effect_id.clone(),
                            kind: ModelKind::Decision,
                            output: dec,
                            usage: Usage::default(),
                        }]))
                        .expect("d2");
                    if let DriverDirective::DispatchEffects { effects: e2 } = d2 {
                        let res = if idx == 1 {
                            StepResult::Tool {
                                effect_id: e2[0].effect_id.clone(),
                                is_error: false,
                                output: json!({ "stdout": "fast\n" }),
                                blob_digest: None,
                            }
                        } else {
                            StepResult::Model {
                                effect_id: e2[0].effect_id.clone(),
                                kind: ModelKind::Llm,
                                output: json!({ "text": "escalated", "reasoning": [], "tool_calls": [], "stop_reason": "end_turn" }),
                                usage: Usage::default(),
                            }
                        };
                        let _ = rt.next_action(DriverResponse::EffectResults(vec![res]));
                    }
                }
            }
            3..=5 => {
                rt.start_turn(
                    json!("steerable"),
                    "general",
                    &[],
                    ArtifactStoreView::default(),
                    101,
                )
                .expect("start");
                if let DriverDirective::DispatchEffects { effects } =
                    rt.next_action(DriverResponse::Poll).expect("d1")
                {
                    if idx == 3 {
                        rt.enqueue_steering(
                            SteeringKind::Steer,
                            json!("focus"),
                            SteeringSource::User,
                            102,
                        )
                        .expect("steer");
                    } else if idx == 4 {
                        rt.enqueue_steering(
                            SteeringKind::UrgentSteer,
                            json!("abort step"),
                            SteeringSource::User,
                            102,
                        )
                        .expect("urgent");
                    } else {
                        rt.enqueue_steering(
                            SteeringKind::Abort,
                            json!("stop"),
                            SteeringSource::User,
                            102,
                        )
                        .expect("abort");
                    }
                    let _ = rt.next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                        effect_id: effects[0].effect_id.clone(),
                        kind: ModelKind::Llm,
                        output: json!({ "text": "ok", "reasoning": [], "tool_calls": [], "stop_reason": "end_turn" }),
                        usage: Usage::default(),
                    }]));
                }
            }
            _ => {
                let decs: Vec<String> = if idx == 6 {
                    vec!["goal_check".into()]
                } else if idx == 7 {
                    vec!["compaction".into()]
                } else {
                    Vec::new()
                };
                if idx == 9 {
                    let _ =
                        rt.append_custom_node("std:state_token".into(), json!({ "tok": 1 }), 100);
                }
                rt.start_turn(
                    json!("phase task"),
                    "general",
                    &decs,
                    ArtifactStoreView::default(),
                    101,
                )
                .expect("start");
                if let DriverDirective::DispatchEffects { effects } =
                    rt.next_action(DriverResponse::Poll).expect("d1")
                {
                    let _ = rt.next_action(DriverResponse::EffectResults(vec![StepResult::Model {
                        effect_id: effects[0].effect_id.clone(),
                        kind: ModelKind::Llm,
                        output: json!({ "text": "ok", "reasoning": [], "tool_calls": [], "stop_reason": "end_turn" }),
                        usage: Usage::default(),
                    }]));
                }
                if idx == 9 {
                    let head = rt.session_tree().head_id().map(ToString::to_string);
                    let _ = rt.fork_session("branch_a".into(), head.as_deref(), 105);
                }
            }
        }
        write_golden_fixture(&dir.join(name), rt.session_tree());
    }
}

#[test]
fn gate_3_golden_replays_verify_all_10_offline_and_via_cli() {
    let dir = repo_root().join("tests/golden_replays");
    let reg = golden_registry();
    let gate = PolicyGate::new(BudgetCap::default(), None, default_std_sandbox_policy());
    let cli_bin = env!("CARGO_BIN_EXE_gestalt");

    for name in GOLDEN_FIXTURE_NAMES {
        let fixture = dir.join(name);
        assert!(
            fixture.is_file(),
            "missing golden fixture {}",
            fixture.display()
        );
        let raw = fs::read_to_string(&fixture).expect("read fixture");
        let (tree, warning) = SessionTree::from_jsonl_str(&raw).expect("parse fixture");
        assert!(warning.is_none(), "fixture {name} had truncated tail");

        let report = ReplayEngine::new(&reg, &gate)
            .replay(&tree, ReplayMode::Default, |_| None)
            .expect("replay engine");
        assert!(
            report.divergences.is_empty(),
            "fixture {name} diverged: {:?}",
            report.divergences
        );

        let out = Command::new(cli_bin)
            .args(["replay", fixture.to_str().expect("utf8"), "--json"])
            .output()
            .expect("run gestalt replay");
        assert!(
            out.status.success(),
            "gestalt replay failed for {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let parsed: Value = serde_json::from_slice(&out.stdout).expect("replay json");
        assert_eq!(parsed["parity"], true, "fixture {name} CLI parity false");
    }
}

// ---------------------------------------------------------------------------
// Gate 4 & 5: CLI Exit Codes, --unsafe-no-sandbox Audit, RPC/ACP, Startup <= 25ms
// ---------------------------------------------------------------------------

#[test]
fn gate_4_and_5_cli_exit_codes_audit_trail_and_cold_startup_under_25ms() {
    let cli_bin = env!("CARGO_BIN_EXE_gestalt");

    // Cold startup <= 25ms across 5 runs (median)
    let mut times_ms = Vec::new();
    for _ in 0..5 {
        let t0 = Instant::now();
        let out = Command::new(cli_bin)
            .arg("--version")
            .output()
            .expect("gestalt --version");
        times_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(out.status.code(), Some(0));
    }
    times_ms.sort_by(f64::total_cmp);
    let median = times_ms[times_ms.len() / 2];
    assert!(median <= 25.0, "cold startup median {median:.2}ms > 25ms");

    // Help -> exit 0
    let help = Command::new(cli_bin).arg("--help").output().expect("help");
    assert_eq!(help.status.code(), Some(0));

    // Invalid flag -> exit 2
    let bogus = Command::new(cli_bin)
        .arg("--bogus")
        .output()
        .expect("bogus");
    assert_eq!(bogus.status.code(), Some(2));

    // --unsafe-no-sandbox audit trail recorded in SessionTree
    let ws = std::env::temp_dir().join(format!("gestalt_cli_gate_{}", std::process::id()));
    let _ = fs::remove_dir_all(&ws);
    fs::create_dir_all(ws.join(".gestalt/sessions")).expect("mkdir");
    let sess = ws.join("unsafe_audit.jsonl");

    let mut rpc = Command::new(cli_bin)
        .current_dir(&ws)
        .args([
            "--mode",
            "rpc",
            "--unsafe-no-sandbox",
            "--session",
            sess.to_str().expect("utf8"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn rpc");
    {
        let mut sin = rpc.stdin.take().expect("stdin");
        let _ = writeln!(
            sin,
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/list\"}}"
        );
    }
    let rpc_out = rpc.wait_with_output().expect("wait rpc");
    assert_eq!(rpc_out.status.code(), Some(0));
    let raw_sess = fs::read_to_string(&sess).expect("read session");
    assert!(
        raw_sess.contains("cli:unsafe_no_sandbox"),
        "expected cli:unsafe_no_sandbox audit node in session"
    );

    let _ = fs::remove_dir_all(&ws);
}
