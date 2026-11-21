#![forbid(unsafe_code)]
//! Milestone 1.4 Self-Extension & External Rust Extension Proof Gate.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use gestalt_kernel::{
    AgentLoop, AgentLoopFactory, ArtifactPatch, ArtifactStoreView, BudgetCap, ContextBuilder,
    Extension, ExtensionError, ExtensionRegistry, LoopAction, LoopConfig, LoopContext,
    ModelCapabilities, ModelKind, PolicyGate, Preset, ReplayEngine, ReplayMode, SessionSlice,
    StepResult, UIRendererSpec, UISurface, Usage,
};
use gestalt_runtime::{
    BlobStore, CancellationToken, CredentialStore, ExternalReplayOptions, HeadlessPauseHandler,
    ModelProvider, ModelRequest, ModelResponse, ProviderContext, ProviderRegistry, RuntimeError,
    RuntimeHost, RuntimeRegistry, StreamDeltaFn, TurnOutcome, default_sandbox_driver,
};
use gestalt_std::surfaces::{SlashCommand, SurfaceSessionState, execute_slash_command_idle};
use gestalt_std::{
    StdExtension, StdToolsExtension, default_std_sandbox_policy, register_std_tools,
};
use serde_json::{Value, json};

struct CustomRustLoop;

impl AgentLoop for CustomRustLoop {
    fn id(&self) -> &str {
        "custom_rust_loop"
    }

    fn digest(&self) -> &str {
        "custom_rust_loop_v1"
    }

    fn step(
        &mut self,
        _ctx: &mut dyn LoopContext,
        _results: Option<Vec<StepResult>>,
    ) -> LoopAction {
        LoopAction::Done(json!({ "source": "custom_rust_extension", "status": "ok" }))
    }
}

struct CustomRustLoopFactory;

impl AgentLoopFactory for CustomRustLoopFactory {
    fn id(&self) -> &str {
        "custom_rust_loop"
    }

    fn digest(&self) -> &str {
        "custom_rust_loop_v1"
    }

    fn create(&self, _config: &LoopConfig, _registry: &ExtensionRegistry) -> Box<dyn AgentLoop> {
        Box::new(CustomRustLoop)
    }
}

struct CustomRustBuilder;

impl ContextBuilder for CustomRustBuilder {
    fn id(&self) -> &str {
        "custom_rust_builder"
    }

    fn digest(&self) -> &str {
        "custom_rust_builder_v1"
    }

    fn build(
        &self,
        _session: &SessionSlice,
        _artifacts: &ArtifactStoreView,
        _caps: &ModelCapabilities,
    ) -> Value {
        json!({ "custom": true })
    }
}

struct CustomRustExtension;

impl Extension for CustomRustExtension {
    fn name(&self) -> &str {
        "custom_rust_ext"
    }

    fn register(&self, registry: &mut ExtensionRegistry) -> Result<(), ExtensionError> {
        registry.loops.insert(
            "custom_rust_loop".to_string(),
            Box::new(CustomRustLoopFactory),
        );
        registry.builders.insert(
            "custom_rust_builder".to_string(),
            Box::new(CustomRustBuilder),
        );
        registry.presets.insert(
            "custom_rust_preset".to_string(),
            Preset {
                base_loop: "custom_rust_loop".to_string(),
                decorators: Vec::new(),
                builder: "custom_rust_builder".to_string(),
                config: LoopConfig::default(),
            },
        );
        registry.ui.merge(UISurface {
            renderers: BTreeMap::from([(
                "custom_rust_card".to_string(),
                UIRendererSpec {
                    id: "custom_rust_card".to_string(),
                    title: "Custom Rust Card".to_string(),
                    target_kind: "custom".to_string(),
                    schema: Some(json!({ "status": "string" })),
                },
            )]),
            ..UISurface::default()
        });
        Ok(())
    }
}

struct DummyLlm;

impl ModelProvider for DummyLlm {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn capabilities(&self, _model: &str) -> ModelCapabilities {
        ModelCapabilities::default()
    }

    fn invoke<'a>(
        &'a self,
        _req: &'a ModelRequest,
        _ctx: &'a ProviderContext<'a>,
        _on_delta: StreamDeltaFn<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ModelResponse, RuntimeError>> + Send + 'a>> {
        Box::pin(async move {
            Ok(ModelResponse {
                kind: ModelKind::Llm,
                output: json!({
                    "content": [{ "type": "text", "text": "hello from general" }],
                    "stop_reason": "end_turn"
                }),
                usage: Usage::default(),
            })
        })
    }
}

#[tokio::test]
async fn compile_time_external_rust_extension_registers_switches_and_replays() {
    let ws = std::env::temp_dir().join(format!("gestalt_rust_ext_proof_{}", std::process::id()));
    let _ = fs::remove_dir_all(&ws);
    fs::create_dir_all(ws.join(".gestalt/sessions")).expect("mkdir");

    let mut reg = ExtensionRegistry::new();
    StdExtension.register(&mut reg).expect("std");
    StdToolsExtension.register(&mut reg).expect("tools");
    CustomRustExtension
        .register(&mut reg)
        .expect("custom rust ext");
    assert!(reg.ui.renderers.contains_key("custom_rust_card"));

    let gate = PolicyGate::new(BudgetCap::default(), None, default_std_sandbox_policy());
    let mut providers = ProviderRegistry::new();
    providers.register_provider(Arc::new(DummyLlm));
    let mut runtime = RuntimeRegistry::new(default_sandbox_driver(true));
    runtime.providers = providers;
    register_std_tools(&mut runtime);

    let creds =
        CredentialStore::load_from_path(ws.join(".gestalt/credentials.toml")).expect("creds");
    let mut host = RuntimeHost::new(
        "rust_ext_run".to_string(),
        &ws,
        ws.join(".gestalt/sessions/run.jsonl"),
        reg,
        gate,
        runtime,
        creds,
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");

    let mut surface_state = SurfaceSessionState::new("general");
    let t1 = host
        .run_turn(
            json!("start in general"),
            &surface_state.active_preset,
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("turn 1");
    assert!(matches!(t1, TurnOutcome::Completed { .. }));

    execute_slash_command_idle(
        &mut host,
        &mut surface_state,
        SlashCommand::Mode {
            preset: "custom_rust_preset".to_string(),
        },
    )
    .await
    .expect("switch to custom_rust_preset");
    assert_eq!(surface_state.active_preset, "custom_rust_preset");

    let t2 = host
        .run_turn(
            json!("run custom loop"),
            &surface_state.active_preset,
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("turn 2");
    let TurnOutcome::Completed { output, .. } = t2 else {
        panic!("expected Completed");
    };
    assert_eq!(output["source"], "custom_rust_extension");

    let blobs = BlobStore::new(&ws);
    let report = ReplayEngine::new(host.kernel().registry(), host.kernel().policy_gate())
        .replay(host.kernel().session_tree(), ReplayMode::Default, |d| {
            blobs.get_utf8(d).ok()
        })
        .expect("replay");
    assert!(report.divergences.is_empty());

    let _ = fs::remove_dir_all(&ws);
}

fn worker_script(version: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([^,}}]*\).*/\1/p')
  case "$line" in
    *"extension/describe"*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"name":"auto_ext","version":"{version}","loops":[{{"id":"auto_loop","digest":"auto_loop_{version}"}}],"builders":[{{"id":"auto_builder","digest":"auto_builder_{version}"}}],"tools":[{{"name":"auto_ping","description":"ping {version}","parameters_schema":{{"type":"object"}},"default_side_effect":"read"}}]}}}}\n' "$id"
      ;;
    *"loop/step"*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"action":{{"action":"done","payload":{{"ext_version":"{version}"}}}}}}}}\n' "$id"
      ;;
    *"tool/execute"*)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"ok":true,"content":"pong_{version}"}}}}\n' "$id"
      ;;
    *)
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{}}}}\n' "$id"
      ;;
  esac
done
"#
    )
}

#[tokio::test]
async fn live_agent_authored_self_extension_trust_reload_upgrade_and_replay() {
    let ws = std::env::temp_dir().join(format!("gestalt_self_ext_proof_{}", std::process::id()));
    let _ = fs::remove_dir_all(&ws);
    fs::create_dir_all(ws.join(".gestalt/sessions")).expect("mkdir");
    let isolated_cfg = ws.join(".isolated_cfg");
    fs::create_dir_all(&isolated_cfg).expect("mkdir cfg");

    let mut reg = ExtensionRegistry::new();
    StdExtension.register(&mut reg).expect("std");
    StdToolsExtension.register(&mut reg).expect("tools");

    let gate = PolicyGate::new(BudgetCap::default(), None, default_std_sandbox_policy());
    let mut runtime = RuntimeRegistry::new(default_sandbox_driver(true));
    register_std_tools(&mut runtime);

    let creds =
        CredentialStore::load_from_path(isolated_cfg.join("credentials.toml")).expect("creds");
    let mut host = RuntimeHost::new(
        "self_ext_run".to_string(),
        &ws,
        ws.join(".gestalt/sessions/run.jsonl"),
        reg,
        gate,
        runtime,
        creds,
        None,
        Arc::new(HeadlessPauseHandler::default()),
    )
    .expect("host");

    let worker_rel = ".gestalt/extensions/auto_ext/worker.sh";
    let toml_rel = ".gestalt/gestalt.toml";
    let worker_path = ws.join(worker_rel);
    let toml_content = format!(
        "[presets.auto_preset]\nbase_loop = \"auto_loop\"\nbuilder = \"auto_builder\"\n\n\
         [extensions.auto_ext]\nprotocol = \"rpc\"\ncommand = [\"bash\", \"{}\"]\ntimeout_ms = 5000\n",
        worker_path.display()
    );

    let patch_worker = ArtifactPatch::WriteFile {
        content: worker_script("v1"),
    };
    let patch_toml = ArtifactPatch::WriteFile {
        content: toml_content,
    };
    host.artifacts()
        .stage_save("eff_1", "stg_v1_w", worker_rel, &patch_worker, 100)
        .expect("stage worker v1");
    host.artifacts()
        .promote_save("eff_2", "stg_v1_w", worker_rel, &patch_worker, 101)
        .expect("promote worker v1");
    host.artifacts()
        .stage_save("eff_3", "stg_v1_t", toml_rel, &patch_toml, 102)
        .expect("stage toml v1");
    host.artifacts()
        .promote_save("eff_4", "stg_v1_t", toml_rel, &patch_toml, 103)
        .expect("promote toml v1");

    let untrusted_err = host
        .reload_external_extension("auto_ext")
        .await
        .expect_err("must reject untrusted gestalt.toml");
    assert!(matches!(
        untrusted_err,
        RuntimeError::UntrustedWorkspace { .. }
    ));

    let mut state = SurfaceSessionState::new("general");
    let trust_res = execute_slash_command_idle(&mut host, &mut state, SlashCommand::Trust)
        .await
        .expect("/trust");
    assert_eq!(trust_res.data["trusted"], true);

    execute_slash_command_idle(
        &mut host,
        &mut state,
        SlashCommand::Reload {
            ext_name: "auto_ext".to_string(),
        },
    )
    .await
    .expect("/reload auto_ext v1");

    execute_slash_command_idle(
        &mut host,
        &mut state,
        SlashCommand::Mode {
            preset: "auto_preset".to_string(),
        },
    )
    .await
    .expect("/mode auto_preset");

    let out_v1 = host
        .run_turn(
            json!("run v1"),
            &state.active_preset,
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("turn v1");
    let TurnOutcome::Completed { output: o1, .. } = out_v1 else {
        panic!("expected v1 completion");
    };
    assert_eq!(o1["ext_version"], "v1");

    let patch_v2 = ArtifactPatch::WriteFile {
        content: worker_script("v2"),
    };
    host.artifacts()
        .stage_save("eff_5", "stg_v2_w", worker_rel, &patch_v2, 200)
        .expect("stage v2");
    host.artifacts()
        .promote_save("eff_6", "stg_v2_w", worker_rel, &patch_v2, 201)
        .expect("promote v2");

    execute_slash_command_idle(
        &mut host,
        &mut state,
        SlashCommand::Reload {
            ext_name: "auto_ext".to_string(),
        },
    )
    .await
    .expect("/reload auto_ext v2");

    let out_v2 = host
        .run_turn(
            json!("run v2"),
            &state.active_preset,
            &[],
            CancellationToken::new(),
        )
        .await
        .expect("turn v2");
    let TurnOutcome::Completed { output: o2, .. } = out_v2 else {
        panic!("expected v2 completion");
    };
    assert_eq!(o2["ext_version"], "v2");

    let blobs = BlobStore::new(&ws);
    let default_report = ReplayEngine::new(host.kernel().registry(), host.kernel().policy_gate())
        .replay(host.kernel().session_tree(), ReplayMode::Default, |d| {
            blobs.get_utf8(d).ok()
        })
        .expect("default replay");
    assert_eq!(default_report.turns_replayed, 2);
    assert!(
        default_report.divergences.iter().all(|d| {
            d.expected.to_string().contains("auto_loop_v1")
                && d.actual.to_string().contains("auto_loop_v2")
        }),
        "unexpected replay divergences: {:?}",
        default_report.divergences
    );

    let mut ext_configs = BTreeMap::new();
    ext_configs.insert(
        "auto_ext".to_string(),
        gestalt_runtime::ExternalExtensionConfig {
            command: vec!["bash".to_string(), worker_path.display().to_string()],
            protocol: gestalt_runtime::ExtensionProtocol::default(),
            timeout_ms: 5000,
        },
    );
    let sandbox_drv = default_sandbox_driver(true);
    let ext_report = gestalt_runtime::replay_session_tree(
        host.kernel().session_tree(),
        host.kernel().registry(),
        host.kernel().policy_gate(),
        &blobs,
        ReplayMode::ReexecuteExternal,
        Some(ExternalReplayOptions {
            workspace_root: &ws,
            sandbox_driver: &sandbox_drv,
            ext_configs: &ext_configs,
        }),
    )
    .await
    .expect("reexecute-ext replay");
    assert_eq!(ext_report.turns_replayed, 2);
    assert_eq!(ext_report.phases_replayed, 2);

    let _ = fs::remove_dir_all(&ws);
}
