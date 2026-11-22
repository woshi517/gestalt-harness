#![forbid(unsafe_code)]
//! Thin CLI composition root for the Gestalt agent harness.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
#[cfg(feature = "loops")]
use gestalt_kernel::Extension;
#[cfg(feature = "surfaces")]
use gestalt_kernel::ReplayMode;
#[cfg(not(feature = "loops"))]
use gestalt_kernel::SandboxPolicy;
use gestalt_kernel::{BudgetCap, ExtensionRegistry, PolicyGate};
use gestalt_runtime::{
    CancellationToken, ConfigLoadOptions, CredentialStore, ExtensionProtocol,
    ExternalExtensionConfig, HeadlessPauseHandler, ProviderRegistry, RuntimeError, RuntimeHost,
    RuntimeRegistry, SandboxKind, StreamRedactor, WorkspaceTrustStore, current_epoch_ms,
    load_and_merge_config, select_sandbox_driver,
};
use serde_json::{Value, json};
use tracing_subscriber::fmt::MakeWriter;

#[cfg(feature = "surfaces")]
use gestalt_std::surfaces::{
    SurfaceExitCode,
    print::{PrintOptions, run_print_surface},
    replay::{ReplaySurfaceOptions, run_replay_surface},
};

fn error_exit_code(err: &RuntimeError) -> i32 {
    #[cfg(feature = "surfaces")]
    {
        SurfaceExitCode::from_runtime_error(err).as_i32()
    }
    #[cfg(not(feature = "surfaces"))]
    {
        match err {
            RuntimeError::HeadlessPauseDenied(_)
            | RuntimeError::SandboxViolation(_)
            | RuntimeError::UntrustedWorkspace { .. } => 3,
            RuntimeError::TomlDe(_) | RuntimeError::Json(_) => 2,
            _ => 1,
        }
    }
}

/// Wire-safe `tracing` `MakeWriter` that writes strictly to `stderr` after
/// scrubbing all known secrets through [`StreamRedactor`].
#[derive(Clone)]
struct RedactingStderrMakeWriter {
    redactor: StreamRedactor,
}

struct RedactingStderrWriter {
    redactor: StreamRedactor,
}

impl Write for RedactingStderrWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let scrubbed = self.redactor.redact_str(&text);
        io::stderr().write_all(scrubbed.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

impl<'a> MakeWriter<'a> for RedactingStderrMakeWriter {
    type Writer = RedactingStderrWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingStderrWriter {
            redactor: self.redactor.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SurfaceMode {
    Tui,
    Print {
        prompt: String,
        json: bool,
    },
    Rpc,
    Acp,
    Replay {
        session_path: PathBuf,
        reexecute_ext: bool,
        json: bool,
    },
    Rollback {
        stage_id: String,
    },
    Help,
    Version,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliArgs {
    mode: SurfaceMode,
    preset: Option<String>,
    model: Option<String>,
    session: Option<PathBuf>,
    extensions: BTreeMap<String, ExternalExtensionConfig>,
    sandbox: Option<SandboxKind>,
    trust_workspace: bool,
    unsafe_no_sandbox: bool,
}

fn usage_text() -> &'static str {
    "Usage:\n  \
     gestalt [OPTIONS]\n  \
     gestalt -p \"<prompt>\" [--json] [OPTIONS]\n  \
     gestalt --mode rpc | acp [OPTIONS]\n  \
     gestalt replay <session.jsonl> [--reexecute-ext] [--json]\n  \
     gestalt rollback <stage_id>\n\n\
     Options:\n  \
     -p, --print <prompt>       Run one headless Turn and exit\n  \
         --json                 Emit structured NDJSON/JSON output\n  \
         --mode <rpc|acp>       Run JSON-RPC 2.0 or ACP stdio server\n  \
         --preset <name>        Select starting preset (default: general)\n  \
         --model <prov/model>   Override active provider/model target\n  \
         --session <path>       Resume or write to explicit SessionTree JSONL path\n  \
         --ext <[name=]cmd>     Attach external JSON-RPC or mcp:<cmd> extension (repeatable)\n  \
         --sandbox <driver>     Select sandbox driver (os | docker | microvm)\n  \
         --trust-workspace      Approve workspace .gestalt/gestalt.toml\n  \
         --unsafe-no-sandbox    Disable OS subprocess sandbox (recorded in SessionTree)\n  \
     -h, --help                 Show this help message\n  \
     -V, --version              Print version and exit"
}

fn parse_ext_spec(raw: &str, index: usize) -> Result<(String, ExternalExtensionConfig), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("--ext requires a non-empty command".to_string());
    }
    let (explicit_name, rhs) = match trimmed.split_once('=') {
        Some((lhs, rest)) if !lhs.trim().is_empty() && !rest.trim().is_empty() => {
            (Some(lhs.trim().to_string()), rest.trim())
        }
        _ => (None, trimmed),
    };
    let (protocol, cmd_str) = if let Some(mcp_cmd) = rhs.strip_prefix("mcp:") {
        (ExtensionProtocol::Mcp, mcp_cmd.trim())
    } else {
        (ExtensionProtocol::default(), rhs)
    };
    let command: Vec<String> = cmd_str
        .split_whitespace()
        .map(ToString::to_string)
        .collect();
    let Some(program) = command.first() else {
        return Err(format!("invalid --ext command '{raw}'"));
    };
    let name = explicit_name.unwrap_or_else(|| {
        let stem = Path::new(program)
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("ext");
        format!("{stem}_{index}")
    });
    Ok((
        name,
        ExternalExtensionConfig {
            command,
            protocol,
            timeout_ms: 15_000,
        },
    ))
}

fn parse_cli_args<I>(args: I) -> Result<CliArgs, String>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = args.into_iter().peekable();
    let mut preset = None;
    let mut model = None;
    let mut session = None;
    let mut extensions = BTreeMap::new();
    let mut sandbox = None;
    let mut trust_workspace = false;
    let mut unsafe_no_sandbox = false;
    let mut print_prompt: Option<String> = None;
    let mut json_flag = false;
    let mut mode_flag: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut reexecute_ext = false;

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                return Ok(CliArgs {
                    mode: SurfaceMode::Help,
                    preset,
                    model,
                    session,
                    extensions,
                    sandbox,
                    trust_workspace,
                    unsafe_no_sandbox,
                });
            }
            "-V" | "--version" => {
                return Ok(CliArgs {
                    mode: SurfaceMode::Version,
                    preset,
                    model,
                    session,
                    extensions,
                    sandbox,
                    trust_workspace,
                    unsafe_no_sandbox,
                });
            }
            "-p" | "--print" => {
                let val = iter
                    .next()
                    .ok_or_else(|| format!("{arg} requires a prompt"))?;
                print_prompt = Some(val);
            }
            "--json" => json_flag = true,
            "--reexecute-ext" => reexecute_ext = true,
            "--trust-workspace" => trust_workspace = true,
            "--unsafe-no-sandbox" => unsafe_no_sandbox = true,
            "--mode" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--mode requires 'rpc' or 'acp'".to_string())?;
                mode_flag = Some(val);
            }
            "--preset" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--preset requires a preset name".to_string())?;
                preset = Some(val);
            }
            "--model" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--model requires <provider/model>".to_string())?;
                model = Some(val);
            }
            "--session" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--session requires a file path".to_string())?;
                session = Some(PathBuf::from(val));
            }
            "--ext" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--ext requires <[name=]cmd>".to_string())?;
                let idx = extensions.len() + 1;
                let (name, cfg) = parse_ext_spec(&val, idx)?;
                extensions.insert(name, cfg);
            }
            "--sandbox" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "--sandbox requires 'os', 'docker', or 'microvm'".to_string())?;
                sandbox = Some(match val.as_str() {
                    "os" | "bwrap" | "seatbelt" => SandboxKind::Os,
                    "docker" => SandboxKind::Docker,
                    "microvm" => SandboxKind::MicroVm,
                    other => {
                        return Err(format!(
                            "unsupported --sandbox '{other}' (expected 'os', 'docker', or 'microvm')"
                        ));
                    }
                });
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown flag '{other}'"));
            }
            other => positional.push(other.to_string()),
        }
    }

    let mode = if let Some(sub) = positional.first().map(String::as_str) {
        match sub {
            "replay" => {
                let path = positional
                    .get(1)
                    .ok_or_else(|| "usage: gestalt replay <session.jsonl>".to_string())?;
                SurfaceMode::Replay {
                    session_path: PathBuf::from(path),
                    reexecute_ext,
                    json: json_flag,
                }
            }
            "rollback" => {
                let stage_id = positional
                    .get(1)
                    .ok_or_else(|| "usage: gestalt rollback <stage_id>".to_string())?;
                SurfaceMode::Rollback {
                    stage_id: stage_id.clone(),
                }
            }
            other => {
                return Err(format!("unknown subcommand '{other}'"));
            }
        }
    } else if let Some(prompt) = print_prompt {
        SurfaceMode::Print {
            prompt,
            json: json_flag,
        }
    } else if let Some(mode_name) = mode_flag {
        match mode_name.as_str() {
            "rpc" => SurfaceMode::Rpc,
            "acp" => SurfaceMode::Acp,
            other => {
                return Err(format!(
                    "unsupported --mode '{other}' (expected 'rpc' or 'acp')"
                ));
            }
        }
    } else {
        SurfaceMode::Tui
    };

    Ok(CliArgs {
        mode,
        preset,
        model,
        session,
        extensions,
        sandbox,
        trust_workspace,
        unsafe_no_sandbox,
    })
}

fn init_tracing(redactor: StreamRedactor) {
    let make_writer = RedactingStderrMakeWriter { redactor };
    let _ = tracing_subscriber::fmt()
        .with_writer(make_writer)
        .with_ansi(false)
        .try_init();
}

fn default_registry_and_policy()
-> std::result::Result<(ExtensionRegistry, PolicyGate), RuntimeError> {
    #[allow(unused_mut)]
    let mut registry = ExtensionRegistry::new();
    #[cfg(feature = "loops")]
    {
        gestalt_std::StdExtension
            .register(&mut registry)
            .map_err(|e| RuntimeError::ArtifactError(e.to_string()))?;
    }
    #[cfg(feature = "tools")]
    {
        gestalt_std::StdToolsExtension
            .register(&mut registry)
            .map_err(|e| RuntimeError::ArtifactError(e.to_string()))?;
    }
    let sandbox_policy = {
        #[cfg(feature = "loops")]
        {
            gestalt_std::default_std_sandbox_policy()
        }
        #[cfg(not(feature = "loops"))]
        {
            SandboxPolicy::default()
        }
    };
    let policy_gate = PolicyGate::new(BudgetCap::default(), None, sandbox_policy);
    Ok((registry, policy_gate))
}

struct BuiltHost {
    host: RuntimeHost,
    #[cfg(feature = "surfaces")]
    preset: String,
    #[cfg(feature = "surfaces")]
    headless_auto_approve: Vec<String>,
}

async fn build_host(cli: &CliArgs) -> std::result::Result<BuiltHost, RuntimeError> {
    let workspace_root = std::env::current_dir()?;
    let credentials = CredentialStore::load_default()?;
    let redactor = credentials.redactor();
    init_tracing(redactor.clone());

    let (mut registry, mut policy_gate) = default_registry_and_policy()?;

    let global_toml = CredentialStore::resolve_config_dir().map(|d| d.join("gestalt.toml"));
    let ws_toml = workspace_root.join(".gestalt").join("gestalt.toml");
    let trust_store = WorkspaceTrustStore::load_default()?;

    if cli.trust_workspace && ws_toml.exists() {
        let raw_bytes = std::fs::read(&ws_toml)?;
        let mut writable_trust = WorkspaceTrustStore::load_default()?;
        writable_trust.trust_workspace(&workspace_root, &raw_bytes)?;
    }

    let resolved = load_and_merge_config(
        ConfigLoadOptions {
            global_toml_path: global_toml.as_deref(),
            workspace_toml_path: Some(ws_toml.as_path()),
            workspace_root: &workspace_root,
            trust_store: &trust_store,
            auto_trust_workspace: cli.trust_workspace,
            cli_extensions: cli.extensions.clone(),
        },
        &mut policy_gate,
        &mut registry,
    )?;

    let active_preset = cli
        .preset
        .clone()
        .unwrap_or_else(|| resolved.default_preset.clone());

    let effective_model = cli.model.clone().or_else(|| {
        if resolved.providers.contains_key("default_llm") {
            return None;
        }
        let has_explicit = registry
            .presets
            .get(&active_preset)
            .and_then(|p| p.config.params.get("model"))
            .and_then(Value::as_str)
            .is_some();
        if has_explicit {
            return None;
        }
        if credentials
            .get("anthropic", Some("ANTHROPIC_API_KEY"))
            .is_some()
        {
            Some("anthropic/claude-sonnet-4-6".to_string())
        } else if credentials.get("openai", Some("OPENAI_API_KEY")).is_some() {
            Some("openai/gpt-5".to_string())
        } else {
            None
        }
    });

    if let Some(ref model_override) = effective_model
        && let Some(preset) = registry.presets.get_mut(&active_preset)
    {
        if !preset.config.params.is_object() {
            preset.config.params = json!({});
        }
        preset.config.params["model"] = Value::String(model_override.clone());
    }

    let effective_sb_kind = match cli.sandbox {
        Some(requested) => {
            if !cli.unsafe_no_sandbox
                && requested.isolation_rank() < resolved.sandbox_driver.isolation_rank()
            {
                return Err(RuntimeError::Kernel(
                    gestalt_kernel::KernelError::PolicyRelaxationForbidden(format!(
                        "--sandbox '{requested:?}' cannot downgrade configured '{:?}'",
                        resolved.sandbox_driver
                    )),
                ));
            }
            requested
        }
        None => resolved.sandbox_driver,
    };
    let sandbox_driver = select_sandbox_driver(
        effective_sb_kind,
        resolved.docker_sandbox.clone(),
        resolved.microvm_sandbox.clone(),
        cli.unsafe_no_sandbox,
    );

    #[allow(unused_mut)]
    let mut providers = ProviderRegistry::new();
    #[cfg(feature = "providers")]
    {
        gestalt_std::register_std_providers(&mut providers);
    }

    let mut runtime = RuntimeRegistry::new(sandbox_driver);
    runtime.providers = providers;

    #[cfg(feature = "tools")]
    {
        gestalt_std::register_std_tools(&mut runtime);
    }

    let cred_arc = Arc::new(std::sync::Mutex::new(credentials.clone()));
    #[cfg(feature = "loops")]
    {
        gestalt_std::register_std_commands(&mut runtime, cred_arc);
        gestalt_std::register_std_triggers(&mut runtime);
    }
    #[cfg(not(feature = "loops"))]
    let _ = cred_arc;

    runtime
        .connect_configured_extensions(
            &workspace_root,
            &resolved,
            &redactor,
            &mut policy_gate,
            &mut registry,
        )
        .await?;

    let session_path = match &cli.session {
        Some(p) => p.clone(),
        None => {
            let session_dir = workspace_root.join(&resolved.session_dir);
            std::fs::create_dir_all(&session_dir)?;
            session_dir.join(format!("session_{}.jsonl", current_epoch_ms()))
        }
    };
    if let Some(parent) = session_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let pause_handler: Arc<dyn gestalt_runtime::PauseHandler> = Arc::new(HeadlessPauseHandler {
        auto_approve: resolved.headless_auto_approve.clone(),
    });

    let run_id = format!("run_{}", current_epoch_ms());
    let ctor = if session_path.exists() && cli.session.is_some() {
        RuntimeHost::open_session
    } else {
        RuntimeHost::new
    };
    let mut host = ctor(
        run_id,
        &workspace_root,
        session_path,
        registry,
        policy_gate,
        runtime,
        credentials,
        None,
        pause_handler,
    )?;

    host.set_auto_trust_workspace(cli.trust_workspace);
    host.start_configured_triggers(&resolved).await?;

    #[cfg(feature = "loops")]
    {
        host.set_extra_read_roots(gestalt_std::extensions::builders::discover_global_skill_roots());
        let tracked = gestalt_std::discover_tracked_artifact_paths(&workspace_root);
        host.set_tracked_artifact_paths(tracked);
    }

    if cli.unsafe_no_sandbox {
        host.kernel_mut()
            .append_custom_node(
                "cli:unsafe_no_sandbox".to_string(),
                json!({
                    "warning": "OS subprocess sandbox disabled via --unsafe-no-sandbox",
                    "timestamp_ms": current_epoch_ms(),
                }),
                current_epoch_ms(),
            )
            .map_err(RuntimeError::Kernel)?;
        host.flush_pending()?;
    }

    Ok(BuiltHost {
        host,
        #[cfg(feature = "surfaces")]
        preset: active_preset,
        #[cfg(feature = "surfaces")]
        headless_auto_approve: resolved.headless_auto_approve,
    })
}

async fn run_cli() -> Result<i32> {
    let cli = match parse_cli_args(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("error: {err}\n\n{}", usage_text());
            return Ok(2);
        }
    };

    match &cli.mode {
        SurfaceMode::Version => {
            println!("gestalt {}", env!("CARGO_PKG_VERSION"));
            return Ok(0);
        }
        SurfaceMode::Help => {
            println!("{}", usage_text());
            return Ok(0);
        }
        _ => {}
    }

    let cancel = CancellationToken::new();
    let cancel_signal = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_signal.cancel();
        }
    });

    match cli.mode {
        SurfaceMode::Replay {
            ref session_path,
            reexecute_ext,
            json,
        } => {
            #[cfg(feature = "surfaces")]
            {
                let workspace_root = std::env::current_dir()?;
                let (registry, policy_gate) = match default_registry_and_policy() {
                    Ok(pair) => pair,
                    Err(err) => {
                        eprintln!("replay init error: {err}");
                        return Ok(error_exit_code(&err));
                    }
                };
                let mode = if reexecute_ext {
                    ReplayMode::ReexecuteExternal
                } else {
                    ReplayMode::Default
                };
                let opts = ReplaySurfaceOptions {
                    session_path,
                    workspace_root: &workspace_root,
                    mode,
                    json_mode: json,
                    ext_opts: None,
                };
                let mut stdout = tokio::io::stdout();
                let res = run_replay_surface(opts, &registry, &policy_gate, &mut stdout).await;
                if let Some(err) = res.error {
                    eprintln!("{err}");
                }
                Ok(res.exit_code.as_i32())
            }
            #[cfg(not(feature = "surfaces"))]
            {
                let _ = (session_path, reexecute_ext, json);
                eprintln!("replay surface requires the 'surfaces' feature");
                Ok(2)
            }
        }
        SurfaceMode::Rollback { ref stage_id } => {
            let built = match build_host(&cli).await {
                Ok(b) => b,
                Err(err) => {
                    eprintln!("rollback init error: {err}");
                    return Ok(error_exit_code(&err));
                }
            };
            match built.host.rollback_stage(stage_id) {
                Ok(entry) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "rolled_back": stage_id,
                            "ledger_entry": entry,
                        }))?
                    );
                    Ok(0)
                }
                Err(err) => {
                    eprintln!("rollback failed: {err}");
                    Ok(error_exit_code(&err))
                }
            }
        }
        SurfaceMode::Print { ref prompt, json } => {
            #[cfg(feature = "surfaces")]
            {
                let mut built = match build_host(&cli).await {
                    Ok(b) => b,
                    Err(err) => {
                        eprintln!("init error: {err}");
                        return Ok(error_exit_code(&err));
                    }
                };
                let opts = PrintOptions {
                    prompt: Value::String(prompt.clone()),
                    preset: built.preset,
                    extra_decorators: Vec::new(),
                    json_mode: json,
                    headless_auto_approve: built.headless_auto_approve,
                };
                let mut stdout = tokio::io::stdout();
                let res =
                    run_print_surface(&mut built.host, opts, &mut stdout, cancel.clone()).await;
                if let Some(err) = res.error {
                    eprintln!("{err}");
                }
                if cancel.is_cancelled() {
                    return Ok(SurfaceExitCode::Aborted.as_i32());
                }
                Ok(res.exit_code.as_i32())
            }
            #[cfg(not(feature = "surfaces"))]
            {
                let _ = (prompt, json);
                eprintln!("print surface requires the 'surfaces' feature");
                Ok(2)
            }
        }
        SurfaceMode::Rpc => {
            #[cfg(feature = "surfaces")]
            {
                let built = match build_host(&cli).await {
                    Ok(b) => b,
                    Err(err) => {
                        eprintln!("init error: {err}");
                        return Ok(error_exit_code(&err));
                    }
                };
                let stdin = tokio::io::BufReader::new(tokio::io::stdin());
                let stdout = tokio::io::stdout();
                gestalt_std::surfaces::rpc::run_rpc_surface(
                    built.host,
                    built.preset,
                    stdin,
                    stdout,
                )
                .await?;
                Ok(0)
            }
            #[cfg(not(feature = "surfaces"))]
            {
                eprintln!("rpc surface requires the 'surfaces' feature");
                Ok(2)
            }
        }
        SurfaceMode::Acp => {
            #[cfg(feature = "surfaces")]
            {
                let built = match build_host(&cli).await {
                    Ok(b) => b,
                    Err(err) => {
                        eprintln!("init error: {err}");
                        return Ok(error_exit_code(&err));
                    }
                };
                let stdin = tokio::io::BufReader::new(tokio::io::stdin());
                let stdout = tokio::io::stdout();
                gestalt_std::surfaces::acp::run_acp_surface(
                    built.host,
                    built.preset,
                    stdin,
                    stdout,
                )
                .await?;
                Ok(0)
            }
            #[cfg(not(feature = "surfaces"))]
            {
                eprintln!("acp surface requires the 'surfaces' feature");
                Ok(2)
            }
        }
        SurfaceMode::Tui => {
            #[cfg(feature = "tui")]
            {
                let built = match build_host(&cli).await {
                    Ok(b) => b,
                    Err(err) => {
                        eprintln!("init error: {err}");
                        return Ok(error_exit_code(&err));
                    }
                };
                let code = gestalt_std::surfaces::tui::run_tui_surface(
                    built.host,
                    built.preset,
                    gestalt_std::surfaces::tui::TuiComponentRegistry::default(),
                )
                .await?;
                Ok(code.as_i32())
            }
            #[cfg(not(feature = "tui"))]
            {
                eprintln!("interactive TUI requires the 'tui' feature");
                Ok(2)
            }
        }
        SurfaceMode::Help | SurfaceMode::Version => Ok(0),
    }
}

#[tokio::main]
async fn main() {
    let code = match run_cli().await {
        Ok(c) => c,
        Err(err) => {
            eprintln!("fatal error: {err}");
            1
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parses_version_help_and_default_tui() {
        assert_eq!(
            parse_cli_args(vec_args(&["--version"]))
                .expect("version")
                .mode,
            SurfaceMode::Version
        );
        assert_eq!(
            parse_cli_args(vec_args(&["-h"])).expect("help").mode,
            SurfaceMode::Help
        );
        let default = parse_cli_args(vec_args(&[])).expect("default");
        assert_eq!(default.mode, SurfaceMode::Tui);
    }

    #[test]
    fn parses_print_rpc_acp_replay_rollback_and_flags() {
        let p = parse_cli_args(vec_args(&[
            "-p",
            "fix bug",
            "--json",
            "--preset",
            "triage",
            "--model",
            "openai/gpt-5",
            "--trust-workspace",
            "--unsafe-no-sandbox",
            "--ext",
            "fs=mcp:npx -y server-fs",
        ]))
        .expect("print");
        assert_eq!(
            p.mode,
            SurfaceMode::Print {
                prompt: "fix bug".into(),
                json: true
            }
        );
        assert_eq!(p.preset.as_deref(), Some("triage"));
        assert_eq!(p.model.as_deref(), Some("openai/gpt-5"));
        assert!(p.trust_workspace);
        assert!(p.unsafe_no_sandbox);
        let ext = p.extensions.get("fs").expect("fs ext");
        assert_eq!(ext.protocol, ExtensionProtocol::Mcp);
        assert_eq!(ext.command, vec!["npx", "-y", "server-fs"]);

        let replay = parse_cli_args(vec_args(&[
            "replay",
            "trace.jsonl",
            "--reexecute-ext",
            "--json",
        ]))
        .expect("replay");
        assert_eq!(
            replay.mode,
            SurfaceMode::Replay {
                session_path: PathBuf::from("trace.jsonl"),
                reexecute_ext: true,
                json: true,
            }
        );

        let rollback = parse_cli_args(vec_args(&["rollback", "stg_42"])).expect("rollback");
        assert_eq!(
            rollback.mode,
            SurfaceMode::Rollback {
                stage_id: "stg_42".into()
            }
        );
    }

    #[test]
    fn rejects_unknown_flags_and_missing_values() {
        assert!(parse_cli_args(vec_args(&["--bogus"])).is_err());
        assert!(parse_cli_args(vec_args(&["-p"])).is_err());
        assert!(parse_cli_args(vec_args(&["--mode", "gui"])).is_err());
        assert!(parse_cli_args(vec_args(&["--sandbox", "invalid"])).is_err());
        assert!(parse_cli_args(vec_args(&["replay"])).is_err());

        let docker = parse_cli_args(vec_args(&["--sandbox", "docker"])).expect("docker");
        assert_eq!(docker.sandbox, Some(SandboxKind::Docker));
        let microvm = parse_cli_args(vec_args(&["--sandbox", "microvm"])).expect("microvm");
        assert_eq!(microvm.sandbox, Some(SandboxKind::MicroVm));
    }
}
