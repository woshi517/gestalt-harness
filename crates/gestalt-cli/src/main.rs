use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use gestalt_cli::{
    auth::{auth_doctor, resolve_auth},
    chat,
    config::{explain_config, load_effective_config, validate_workspace_config, CliOverrides},
    context,
    cost::calculate_cost,
    doctor, export,
    models::{inspect_model, list_models, refresh_models, search_models},
    output::{
        AuthDoctorReport, AuthResolveReport, CliErrorPayload, CliReport, ConfigExplainReport,
        ConfigPathsReport, ConfigShowReport, ConfigValidateReport, ContextExplainReport,
        CostReportWrapper, ExportFormat, ExtensionActionReport, ExtensionInspectReport,
        ExtensionsListReport, JsonEnvelope, ModelsInspectReport, ModelsListReport,
        ModelsRefreshReport, ModelsSearchReport, ModelsSelectReport, OutputFormat,
        PolicyExplainReport, PolicyTestReport, PolicyValidateReport, ProvidersDoctorReport,
        ProvidersInspectReport, ProvidersListReport, ReplayReport, RunReport, RuntimeDoctorReport,
        RuntimeEventsReport, RuntimeInspectReport, SkillActionReport, SkillInspectReport,
        SkillsListReport, WorkspaceSnapshotReport,
    },
    policy,
    providers::{doctor_provider, inspect_provider, list_providers},
    replay::replay_display,
    run::run_prompt,
    runs, sessions, tools, trace, verify,
    workspace::{
        doctor_workspace, info_workspace, init_workspace, snapshot_workspace, status_workspace,
    },
};

#[derive(Parser)]
#[command(name = "gestalt")]
struct Cli {
    #[arg(long, global = true)]
    workspace: Option<PathBuf>,
    #[arg(long, global = true)]
    model: Option<String>,
    #[arg(long, global = true)]
    mode: Option<String>,
    #[arg(long, global = true)]
    max_turns: Option<usize>,
    #[arg(long, global = true)]
    provider: Option<String>,
    #[arg(long, global = true)]
    profile: Option<String>,
    #[arg(long, global = true)]
    api_key: Option<String>,
    #[arg(long, default_value = "text")]
    format: OutputFormat,
    #[arg(long, short, global = true)]
    quiet: bool,
    #[arg(long, short, global = true)]
    verbose: bool,
    #[arg(long, global = true)]
    no_color: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Run {
        prompt: String,
        #[arg(long)]
        resume: Option<String>,
        #[arg(long, short)]
        yes: bool,
        #[arg(long)]
        tui: bool,
        #[arg(long)]
        skill: Vec<String>,
    },
    Chat {
        #[arg(long)]
        resume: Option<String>,
        #[arg(long, short)]
        yes: bool,
        #[arg(long)]
        tui: bool,
    },
    Tui {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
    },
    Replay {
        path: PathBuf,
    },
    Cost {
        path: PathBuf,
    },
    Config(ConfigCommand),
    Auth(AuthCommand),
    Providers(ProvidersCommand),
    Models(ModelsCommand),
    Connect {
        provider: String,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        no_keychain: bool,
        #[arg(long)]
        set_default: bool,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        default_model: Option<String>,
        #[arg(long)]
        api_key_env: Option<String>,
    },
    Disconnect {
        provider: String,
        #[arg(long)]
        force: bool,
    },
    Profiles(ProfilesCommand),
    Init {
        #[arg(long)]
        force: bool,
    },
    Status,
    Workspace(WorkspaceCommand),
    Runs(RunsCommand),
    Trace(TraceCommand),
    Sessions(SessionsCommand),
    Export {
        run_id_or_path: String,
        #[arg(long, default_value = "markdown")]
        format: ExportFormat,
    },
    Verify(VerifyCommand),
    Policy(PolicyCommand),
    Context(ContextCommand),
    Tools(ToolsCommand),
    Runtime(RuntimeCommand),
    Extension(ExtensionCommand),
    Skill(SkillCommand),
    Doctor {
        #[arg(long)]
        live: bool,
    },
}

#[derive(Args)]
pub struct ProfilesCommand {
    #[command(subcommand)]
    pub command: ProfilesSubcommand,
}

#[derive(Subcommand)]
pub enum ProfilesSubcommand {
    List,
    Inspect { name: String },
    Use { name: String },
}

#[derive(Args)]
pub struct ToolsCommand {
    #[command(subcommand)]
    pub command: ToolsSubcommand,
}

#[derive(Args)]
pub struct RuntimeCommand {
    #[command(subcommand)]
    pub command: RuntimeSubcommand,
}

#[derive(Subcommand, Clone)]
pub enum RuntimeSubcommand {
    Inspect,
    Events,
    Doctor,
}

#[derive(Args)]
pub struct ExtensionCommand {
    #[command(subcommand)]
    pub command: ExtensionSubcommand,
}

#[derive(Subcommand, Clone)]
pub enum ExtensionSubcommand {
    List,
    Enable { id: String },
    Disable { id: String },
    Inspect { id: String },
    Reload,
    Validate { path: PathBuf },
}

#[derive(Args)]
pub struct SkillCommand {
    #[command(subcommand)]
    pub command: SkillSubcommand,
}

#[derive(Subcommand, Clone)]
pub enum SkillSubcommand {
    List,
    Inspect { name: String },
    Activate { name: String },
    Deactivate { name: String },
    Validate { path: PathBuf },
}

#[derive(Subcommand)]
pub enum ToolsSubcommand {
    List,
    Inspect {
        tool: String,
    },
    Classify {
        #[command(subcommand)]
        sub: ClassifySubcommand,
    },
}

#[derive(Subcommand)]
pub enum ClassifySubcommand {
    Bash {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}

#[derive(Args)]
pub struct ContextCommand {
    #[command(subcommand)]
    pub command: ContextSubcommand,
}

#[derive(Subcommand)]
pub enum ContextSubcommand {
    Explain {
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long)]
        run: Option<String>,
    },
}

#[derive(Args)]
pub struct PolicyCommand {
    #[command(subcommand)]
    pub command: PolicySubcommand,
}

#[derive(Subcommand)]
pub enum PolicySubcommand {
    Validate,
    Explain {
        #[arg(long)]
        tool: String,
        #[arg(long)]
        input: String,
    },
    Test {
        #[arg(long)]
        tool: String,
        #[arg(long)]
        input: String,
        #[arg(long)]
        mode: Option<String>,
    },
}

#[derive(Args)]
pub struct TraceCommand {
    #[command(subcommand)]
    pub command: TraceSubcommand,
}

#[derive(Subcommand)]
pub enum TraceSubcommand {
    Replay {
        run_id_or_path: String,
    },
    Inspect {
        run_id_or_path: String,
    },
    Validate {
        run_id_or_path: String,
    },
    /// Analyze tool-calling reliability metrics over a run or
    /// directory of fixture traces. Wraps
    /// `gestalt_trace::analyze_tool_metrics` so the CLI does not
    /// duplicate the JSONL walking logic.
    ///
    /// The `--tools` flag is the historical entry point and is
    /// preserved as an alias for `--kind tools`. Both forms are
    /// equivalent; `--kind` exists so future analyzers (cost,
    /// retries, etc.) can be added without breaking the surface.
    Analyze {
        run_id_or_path: String,
        #[arg(long, default_value = "tools")]
        kind: String,
        /// Shorthand for `--kind tools`. Kept for parity with the
        /// original `gestalt trace analyze --tools` invocation.
        #[arg(long)]
        tools: bool,
    },
}

#[derive(Args)]
pub struct VerifyCommand {
    #[command(subcommand)]
    pub command: VerifySubcommand,
}

#[derive(Subcommand)]
pub enum VerifySubcommand {
    Run { run_id_or_path: String },
}

#[derive(Args)]
pub struct RunsCommand {
    #[command(subcommand)]
    pub command: RunsSubcommand,
}

#[derive(Subcommand)]
pub enum RunsSubcommand {
    List {
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        json: bool,
    },
    Inspect {
        run_id_or_path: String,
        #[arg(long)]
        json: bool,
    },
    Tail {
        run_id_or_path: String,
    },
    Prune {
        #[arg(long)]
        older_than: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, short)]
        yes: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        cascade: bool,
    },
    Delete {
        run_id_or_path: String,
        #[arg(long, short)]
        yes: bool,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        cascade: bool,
    },
}

#[derive(Args)]
pub struct SessionsCommand {
    #[command(subcommand)]
    pub command: SessionsSubcommand,
}

#[derive(Subcommand)]
pub enum SessionsSubcommand {
    List,
    Inspect {
        session_id: String,
    },
    History {
        session_id: String,
    },
    Continue {
        session_id: String,
        prompt: String,
    },
    Resume {
        run_id_or_path: String,
    },
    Branch {
        run_id_or_path: String,
        #[arg(long)]
        at: u64,
        prompt: String,
    },
}

#[derive(Args)]
struct WorkspaceCommand {
    #[command(subcommand)]
    command: WorkspaceSubcommand,
}

#[derive(Subcommand)]
enum WorkspaceSubcommand {
    Info,
    Snapshot,
    Doctor,
}

#[derive(Args)]
struct ConfigCommand {
    #[command(subcommand)]
    command: ConfigSubcommand,
}

#[derive(Subcommand)]
enum ConfigSubcommand {
    Validate,
    Show {
        #[arg(long)]
        source: bool,
    },
    Explain,
    Paths,
}

#[derive(Args)]
struct AuthCommand {
    #[command(subcommand)]
    command: AuthSubcommand,
}

#[derive(Subcommand)]
enum AuthSubcommand {
    Resolve { provider: String },
    Doctor,
}

#[derive(Args)]
struct ProvidersCommand {
    #[command(subcommand)]
    command: ProvidersSubcommand,
}

#[derive(Subcommand)]
enum ProvidersSubcommand {
    List,
    Inspect {
        provider: String,
    },
    Test {
        provider: String,
    },
    Doctor {
        provider: Option<String>,
        #[arg(long)]
        live: bool,
    },
}

#[derive(Args)]
struct ModelsCommand {
    #[command(subcommand)]
    command: ModelsSubcommand,
}

#[derive(Subcommand)]
enum ModelsSubcommand {
    List {
        #[arg(long)]
        provider: Option<String>,
    },
    Inspect {
        model: String,
    },
    Refresh {
        #[arg(long)]
        live: bool,
    },
    Select {
        model: String,
    },
    Search {
        query: String,
    },
}

fn map_to_cli_error(err: &(dyn std::error::Error + 'static)) -> CliErrorPayload {
    if let Some(harness_err) = err.downcast_ref::<gestalt_core::HarnessError>() {
        match harness_err {
            gestalt_core::HarnessError::Config(cfg_err) => CliErrorPayload {
                code: "CONFIG_ERROR".to_string(),
                message: cfg_err.to_string(),
                details: None,
            },
            gestalt_core::HarnessError::Provider(prov_err) => CliErrorPayload {
                code: "PROVIDER_ERROR".to_string(),
                message: prov_err.to_string(),
                details: None,
            },
            gestalt_core::HarnessError::Policy(pol_err) => CliErrorPayload {
                code: "POLICY_ERROR".to_string(),
                message: pol_err.to_string(),
                details: None,
            },
            gestalt_core::HarnessError::Context(ctx_err) => CliErrorPayload {
                code: "CONTEXT_ERROR".to_string(),
                message: ctx_err.to_string(),
                details: None,
            },
            gestalt_core::HarnessError::Tool(t_err) => CliErrorPayload {
                code: "TOOL_ERROR".to_string(),
                message: t_err.to_string(),
                details: None,
            },
            gestalt_core::HarnessError::Trace(tr_err) => CliErrorPayload {
                code: "TRACE_ERROR".to_string(),
                message: tr_err.to_string(),
                details: None,
            },
            gestalt_core::HarnessError::Approval(app_err) => CliErrorPayload {
                code: "APPROVAL_ERROR".to_string(),
                message: app_err.to_string(),
                details: None,
            },
            gestalt_core::HarnessError::Cancelled => CliErrorPayload {
                code: "CANCELLED".to_string(),
                message: "Execution was cancelled".to_string(),
                details: None,
            },
        }
    } else if let Some(trace_err) = err.downcast_ref::<gestalt_core::TraceError>() {
        CliErrorPayload {
            code: "TRACE_ERROR".to_string(),
            message: trace_err.to_string(),
            details: None,
        }
    } else {
        CliErrorPayload {
            code: "INTERNAL_ERROR".to_string(),
            message: err.to_string(),
            details: None,
        }
    }
}

fn print_error_and_exit(err: &(dyn std::error::Error + 'static), format: OutputFormat) -> ! {
    let payload = map_to_cli_error(err);
    match format {
        OutputFormat::Json => {
            let envelope = JsonEnvelope {
                schema_version: 1,
                kind: "error".to_string(),
                data: payload,
            };
            if let Ok(json) = serde_json::to_string(&envelope) {
                eprintln!("{}", json);
            }
        }
        OutputFormat::Text => {
            eprintln!("error: {}", payload.message);
        }
    }
    std::process::exit(1);
}

fn handle_result<T: CliReport>(
    res: Result<T, Box<dyn std::error::Error>>,
    format: OutputFormat,
    _quiet: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match res {
        Ok(report) => {
            match format {
                OutputFormat::Json => {
                    let envelope = JsonEnvelope {
                        schema_version: 1,
                        kind: report.kind().to_string(),
                        data: report,
                    };
                    println!("{}", serde_json::to_string(&envelope)?);
                }
                OutputFormat::Text => {
                    let text = report.render_text();
                    if !text.is_empty() {
                        println!("{}", text);
                    }
                }
            }
            Ok(())
        }
        Err(err) => {
            print_error_and_exit(err.as_ref(), format);
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let mut overrides = CliOverrides {
        provider: cli.provider.clone(),
        model: cli.model.clone(),
        mode: cli.mode.clone(),
        max_turns: cli.max_turns,
        workspace: cli.workspace.clone(),
        profile: cli.profile.clone(),
        skills: Vec::new(),
    };

    let format = cli.format;
    let quiet = cli.quiet;

    match cli.command {
        Command::Run {
            prompt,
            resume,
            yes,
            tui,
            skill,
        } => {
            overrides.skills = skill;
            if yes {
                overrides.mode = Some("yolo".to_string());
            }
            if tui {
                #[cfg(feature = "tui")]
                {
                    let cancel_token = gestalt_core::CancelToken::new();
                    let config = load_effective_config(&overrides)?;
                    gestalt_cli::tui::run_tui(
                        &config,
                        resume,
                        Some(prompt),
                        cli.api_key.clone(),
                        cancel_token,
                    )
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                }
                #[cfg(not(feature = "tui"))]
                {
                    return Err(Box::new(gestalt_core::HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
                        field: "tui".to_string(),
                        reason: "TUI support is not enabled in this build. Please compile with --features tui.".to_string(),
                    })) as Box<dyn std::error::Error>);
                }
            } else {
                let cancel_token = gestalt_core::CancelToken::new();
                let cancel_token_clone = cancel_token.clone();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        eprintln!("\n[Interrupt] Cancellation requested. Cleaning up...");
                        cancel_token_clone.cancel();

                        if tokio::signal::ctrl_c().await.is_ok() {
                            eprintln!("\n[Interrupt] Force exiting immediately.");
                            std::process::exit(130);
                        }
                    }
                });
                #[cfg(unix)]
                {
                    let cancel_token_clone = cancel_token.clone();
                    tokio::spawn(async move {
                        use tokio::signal::unix::{signal, SignalKind};
                        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                            sigterm.recv().await;
                            eprintln!("\n[Interrupt] SIGTERM received. Cleaning up...");
                            cancel_token_clone.cancel();
                        }
                    });
                }

                let res: Result<RunReport, gestalt_core::HarnessError> = async {
                    let config = load_effective_config(&overrides)?;
                    let run_dir = if let Some(ref target) = resume {
                        sessions::run_session_action(
                            &config,
                            "branch",
                            target,
                            Some(prompt),
                            None,
                            cli.api_key.clone(),
                            cancel_token,
                            None,
                            None,
                        )
                        .await?
                    } else {
                        run_prompt(
                            &config,
                            &prompt,
                            cli.api_key.clone(),
                            cancel_token,
                            None,
                            None,
                            None,
                        )
                        .await?
                    };
                    Ok(RunReport { run_dir })
                }
                .await;
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
        }
        Command::Chat { resume, yes, tui } => {
            if yes {
                overrides.mode = Some("yolo".to_string());
            }
            if tui {
                #[cfg(feature = "tui")]
                {
                    let cancel_token = gestalt_core::CancelToken::new();
                    let config = load_effective_config(&overrides)?;
                    gestalt_cli::tui::run_tui(
                        &config,
                        resume,
                        None,
                        cli.api_key.clone(),
                        cancel_token,
                    )
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
                }
                #[cfg(not(feature = "tui"))]
                {
                    return Err(Box::new(gestalt_core::HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
                        field: "tui".to_string(),
                        reason: "TUI support is not enabled in this build. Please compile with --features tui.".to_string(),
                    })) as Box<dyn std::error::Error>);
                }
            } else {
                let cancel_token = gestalt_core::CancelToken::new();
                #[cfg(unix)]
                {
                    let cancel_token_clone = cancel_token.clone();
                    tokio::spawn(async move {
                        use tokio::signal::unix::{signal, SignalKind};
                        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                            sigterm.recv().await;
                            eprintln!("\n[Interrupt] SIGTERM received. Cleaning up...");
                            cancel_token_clone.cancel();
                        }
                    });
                }

                let res =
                    chat::run_chat(&overrides, resume, cli.api_key.clone(), cancel_token).await;
                if let Err(err) = res {
                    print_error_and_exit(&err, format);
                }
            }
        }
        Command::Tui { run, prompt } => {
            #[cfg(feature = "tui")]
            {
                let cancel_token = gestalt_core::CancelToken::new();
                let config = load_effective_config(&overrides)?;
                gestalt_cli::tui::run_tui(&config, run, prompt, cli.api_key.clone(), cancel_token)
                    .await
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            }
            #[cfg(not(feature = "tui"))]
            {
                let _ = run;
                let _ = prompt;
                return Err(Box::new(gestalt_core::HarnessError::Config(gestalt_core::ConfigError::InvalidValue {
                    field: "tui".to_string(),
                    reason: "TUI support is not enabled in this build. Please compile with --features tui.".to_string(),
                })) as Box<dyn std::error::Error>);
            }
        }
        Command::Replay { path } => {
            let res: Result<ReplayReport, Box<dyn std::error::Error>> = (|| {
                let resolved = if path.exists() {
                    if path.is_dir() {
                        path.join("trace.jsonl")
                    } else {
                        path.clone()
                    }
                } else {
                    let config = load_effective_config(&overrides)?;
                    let run_dir = runs::resolve_run_path(&config, &path.to_string_lossy())?;
                    run_dir.join("trace.jsonl")
                };
                let rendered = replay_display(&resolved)?;
                Ok(ReplayReport { rendered })
            })();
            handle_result(res, format, quiet)?;
        }
        Command::Cost { path } => {
            let res: Result<CostReportWrapper, Box<dyn std::error::Error>> = (|| {
                let resolved = if path.exists() {
                    path.clone()
                } else {
                    let config = load_effective_config(&overrides)?;
                    runs::resolve_run_path(&config, &path.to_string_lossy())?
                };
                let report = calculate_cost(&resolved)?;
                Ok(CostReportWrapper(report))
            })();
            handle_result(res, format, quiet)?;
        }
        Command::Config(command) => match command.command {
            ConfigSubcommand::Validate => {
                let res: Result<ConfigValidateReport, gestalt_core::HarnessError> = (|| {
                    let config = validate_workspace_config(&overrides)?;
                    Ok(ConfigValidateReport {
                        workspace_root: config.workspace_root,
                    })
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ConfigSubcommand::Show { source } => {
                let res: Result<ConfigShowReport, Box<dyn std::error::Error>> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let explain_map = if source {
                        Some(explain_config(&overrides)?)
                    } else {
                        None
                    };
                    Ok(ConfigShowReport {
                        config,
                        source,
                        explain_map,
                    })
                })(
                );
                handle_result(res, format, quiet)?;
            }
            ConfigSubcommand::Explain => {
                let res: Result<ConfigExplainReport, Box<dyn std::error::Error>> = (|| {
                    let explain_map = explain_config(&overrides)?;
                    Ok(ConfigExplainReport { explain_map })
                })(
                );
                handle_result(res, format, quiet)?;
            }
            ConfigSubcommand::Paths => {
                let res: Result<ConfigPathsReport, Box<dyn std::error::Error>> = (|| {
                    let workspace_root = overrides
                        .workspace
                        .clone()
                        .unwrap_or(std::env::current_dir()?);
                    let global_path = gestalt_cli::config::global_config_path();
                    let legacy_global_path = gestalt_cli::config::legacy_global_config_path();
                    let workspace_path =
                        gestalt_cli::config::workspace_config_path(&workspace_root);
                    let legacy_workspace_path =
                        gestalt_cli::config::legacy_workspace_config_path(&workspace_root);
                    let legacy_policies_path =
                        gestalt_cli::config::legacy_workspace_policies_path(&workspace_root);

                    let global_exists = global_path.exists();
                    let legacy_global_exists = legacy_global_path.exists();
                    let workspace_exists = workspace_path.exists();
                    let legacy_workspace_exists = legacy_workspace_path.exists();
                    let legacy_policies_exists = legacy_policies_path.exists();

                    let mut ambiguities = Vec::new();
                    if global_exists && legacy_global_exists {
                        ambiguities.push(format!(
                            "Both global configs exist. '{}' takes precedence over '{}'.",
                            global_path.display(),
                            legacy_global_path.display()
                        ));
                    }
                    if workspace_exists && legacy_workspace_exists {
                        ambiguities.push(format!(
                            "Both workspace configs exist. '{}' takes precedence over '{}'.",
                            workspace_path.display(),
                            legacy_workspace_path.display()
                        ));
                    }
                    if legacy_global_exists {
                        ambiguities.push(format!(
                            "Legacy global TOML configuration '{}' is deprecated. Please migrate to gestalt.json.",
                            legacy_global_path.display()
                        ));
                    }
                    if legacy_workspace_exists {
                        ambiguities.push(format!(
                            "Legacy workspace TOML configuration '{}' is deprecated. Please migrate to gestalt.json.",
                            legacy_workspace_path.display()
                        ));
                    }
                    if legacy_policies_exists {
                        ambiguities.push(format!(
                            "Legacy workspace TOML policies configuration '{}' is deprecated. Please migrate to gestalt.json.",
                            legacy_policies_path.display()
                        ));
                    }

                    Ok(ConfigPathsReport {
                        global_path,
                        global_exists,
                        legacy_global_path,
                        legacy_global_exists,
                        workspace_path,
                        workspace_exists,
                        legacy_workspace_path,
                        legacy_workspace_exists,
                        legacy_policies_path,
                        legacy_policies_exists,
                        ambiguities,
                    })
                })(
                );
                handle_result(res, format, quiet)?;
            }
        },
        Command::Auth(command) => match command.command {
            AuthSubcommand::Resolve { provider } => {
                let res: Result<AuthResolveReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let report = resolve_auth(&config, &provider)?;
                    Ok(report)
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            AuthSubcommand::Doctor => {
                let res: Result<AuthDoctorReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let report = auth_doctor(&config)?;
                    Ok(report)
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
        },
        Command::Providers(command) => match command.command {
            ProvidersSubcommand::List => {
                let res: Result<ProvidersListReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let providers = list_providers(&config);
                    Ok(ProvidersListReport { providers })
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ProvidersSubcommand::Inspect { provider } => {
                let res: Result<ProvidersInspectReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let value = inspect_provider(&config, &provider)?;
                    Ok(ProvidersInspectReport {
                        provider,
                        config: value,
                    })
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ProvidersSubcommand::Test { provider } => {
                let res: Result<ProvidersDoctorReport, gestalt_core::HarnessError> = async {
                    let config = load_effective_config(&overrides)?;
                    let result = doctor_provider(&config, &provider, true).await?;
                    Ok(ProvidersDoctorReport {
                        results: vec![result],
                    })
                }
                .await;
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ProvidersSubcommand::Doctor { provider, live } => {
                let res: Result<ProvidersDoctorReport, gestalt_core::HarnessError> = async {
                    let config = load_effective_config(&overrides)?;
                    let mut results = Vec::new();
                    if let Some(provider) = provider {
                        results.push(doctor_provider(&config, &provider, live).await?);
                    } else {
                        for p in list_providers(&config) {
                            results.push(doctor_provider(&config, &p, live).await?);
                        }
                    }
                    Ok(ProvidersDoctorReport { results })
                }
                .await;
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
        },
        Command::Models(command) => match command.command {
            ModelsSubcommand::List { provider } => {
                let res: Result<ModelsListReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let models = list_models(&config, provider.as_deref());
                    Ok(ModelsListReport { models })
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ModelsSubcommand::Inspect { model } => {
                let res: Result<ModelsInspectReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let model_info = inspect_model(&config, &model)?;
                    Ok(ModelsInspectReport { model: model_info })
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ModelsSubcommand::Refresh { live } => {
                let res: Result<ModelsRefreshReport, gestalt_core::HarnessError> = async {
                    let config = load_effective_config(&overrides)?;
                    refresh_models(&config, live).await
                }
                .await;
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ModelsSubcommand::Select { model } => {
                let res: Result<ModelsSelectReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let info = inspect_model(&config, &model)?;
                    Ok(ModelsSelectReport {
                        qualified_id: info.qualified_id,
                        display_name: info.display_name,
                    })
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ModelsSubcommand::Search { query } => {
                let res: Result<ModelsSearchReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let models = search_models(&config, &query);
                    Ok(ModelsSearchReport { models })
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
        },
        Command::Init { force } => {
            let workspace_root = overrides
                .workspace
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let res = init_workspace(&workspace_root, force);
            handle_result(
                res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                format,
                quiet,
            )?;
        }
        Command::Status => {
            let res = status_workspace(&overrides).await;
            handle_result(
                res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                format,
                quiet,
            )?;
        }
        Command::Workspace(command) => match command.command {
            WorkspaceSubcommand::Info => {
                let res = info_workspace(&overrides);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            WorkspaceSubcommand::Snapshot => {
                let res: Result<WorkspaceSnapshotReport, gestalt_core::HarnessError> =
                    async { snapshot_workspace(&overrides).await }.await;
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            WorkspaceSubcommand::Doctor => {
                let res = doctor_workspace(&overrides).await;
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
        },
        Command::Runs(command) => match command.command {
            RunsSubcommand::List { limit, json } => {
                let fmt = if json { OutputFormat::Json } else { format };
                let res = runs::list_runs(&load_effective_config(&overrides)?, limit);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    fmt,
                    quiet,
                )?;
            }
            RunsSubcommand::Inspect {
                run_id_or_path,
                json,
            } => {
                let fmt = if json { OutputFormat::Json } else { format };
                let res = runs::inspect_run(&load_effective_config(&overrides)?, &run_id_or_path);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    fmt,
                    quiet,
                )?;
            }
            RunsSubcommand::Tail { run_id_or_path } => {
                let config = load_effective_config(&overrides)?;
                if let Err(e) = runs::tail_run(&config, &run_id_or_path, format) {
                    print_error_and_exit(&e, format);
                }
            }
            RunsSubcommand::Prune {
                older_than,
                dry_run,
                yes,
                json,
                cascade,
            } => {
                let config = load_effective_config(&overrides)?;
                let fmt = if json { OutputFormat::Json } else { format };
                let res = runs::prune_runs(&config, older_than, dry_run, yes, cascade);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    fmt,
                    quiet,
                )?;
            }
            RunsSubcommand::Delete {
                run_id_or_path,
                yes,
                json,
                cascade,
            } => {
                let config = load_effective_config(&overrides)?;
                let fmt = if json { OutputFormat::Json } else { format };
                let res = runs::delete_run(&config, &run_id_or_path, yes, cascade);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    fmt,
                    quiet,
                )?;
            }
        },
        Command::Trace(command) => match command.command {
            TraceSubcommand::Replay { run_id_or_path } => {
                let config = load_effective_config(&overrides)?;
                let res = trace::replay_trace(&config, &run_id_or_path);
                handle_result(res, format, quiet)?;
            }
            TraceSubcommand::Inspect { run_id_or_path } => {
                let config = load_effective_config(&overrides)?;
                let res = trace::inspect_trace(&config, &run_id_or_path);
                handle_result(res, format, quiet)?;
            }
            TraceSubcommand::Validate { run_id_or_path } => {
                let config = load_effective_config(&overrides)?;
                let res = trace::validate_trace(&config, &run_id_or_path);
                handle_result(res, format, quiet)?;
            }
            TraceSubcommand::Analyze {
                run_id_or_path,
                kind,
                tools,
            } => {
                // `--tools` is the historical entry point and
                // short-circuits `--kind` so it is the explicit opt-in
                // for tool-calling metrics. Any future analyzer kind
                // would be selected via `--kind` instead.
                let effective_kind = if tools { "tools" } else { kind.as_str() };
                let config = load_effective_config(&overrides)?;
                let res = trace::analyze_trace(&config, &run_id_or_path, effective_kind);
                handle_result(res, format, quiet)?;
            }
        },
        Command::Sessions(command) => match command.command {
            SessionsSubcommand::List => {
                let res = sessions::list_sessions(&load_effective_config(&overrides)?);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            SessionsSubcommand::Inspect { session_id } => {
                let res =
                    sessions::inspect_session(&load_effective_config(&overrides)?, &session_id);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            SessionsSubcommand::History { session_id } => {
                let res =
                    sessions::history_session(&load_effective_config(&overrides)?, &session_id);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            SessionsSubcommand::Continue { session_id, prompt } => {
                let cancel_token = gestalt_core::CancelToken::new();
                let cancel_token_clone = cancel_token.clone();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        eprintln!("\n[Interrupt] Cancellation requested. Cleaning up...");
                        cancel_token_clone.cancel();
                        if tokio::signal::ctrl_c().await.is_ok() {
                            eprintln!("\n[Interrupt] Force exiting immediately.");
                            std::process::exit(130);
                        }
                    }
                });
                #[cfg(unix)]
                {
                    let cancel_token_clone = cancel_token.clone();
                    tokio::spawn(async move {
                        use tokio::signal::unix::{signal, SignalKind};
                        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                            sigterm.recv().await;
                            eprintln!("\n[Interrupt] SIGTERM received. Cleaning up...");
                            cancel_token_clone.cancel();
                        }
                    });
                }
                let res = sessions::run_session_action(
                    &load_effective_config(&overrides)?,
                    "continue",
                    &session_id,
                    Some(prompt),
                    None,
                    cli.api_key.clone(),
                    cancel_token,
                    None,
                    None,
                )
                .await;
                handle_result(
                    res.map(|run_dir| RunReport { run_dir })
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            SessionsSubcommand::Resume { run_id_or_path } => {
                let cancel_token = gestalt_core::CancelToken::new();
                let cancel_token_clone = cancel_token.clone();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        eprintln!("\n[Interrupt] Cancellation requested. Cleaning up...");
                        cancel_token_clone.cancel();
                        if tokio::signal::ctrl_c().await.is_ok() {
                            eprintln!("\n[Interrupt] Force exiting immediately.");
                            std::process::exit(130);
                        }
                    }
                });
                #[cfg(unix)]
                {
                    let cancel_token_clone = cancel_token.clone();
                    tokio::spawn(async move {
                        use tokio::signal::unix::{signal, SignalKind};
                        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                            sigterm.recv().await;
                            eprintln!("\n[Interrupt] SIGTERM received. Cleaning up...");
                            cancel_token_clone.cancel();
                        }
                    });
                }
                let res = sessions::run_session_action(
                    &load_effective_config(&overrides)?,
                    "resume",
                    &run_id_or_path,
                    None,
                    None,
                    cli.api_key.clone(),
                    cancel_token,
                    None,
                    None,
                )
                .await;
                handle_result(
                    res.map(|run_dir| RunReport { run_dir })
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            SessionsSubcommand::Branch {
                run_id_or_path,
                at,
                prompt,
            } => {
                let cancel_token = gestalt_core::CancelToken::new();
                let cancel_token_clone = cancel_token.clone();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        eprintln!("\n[Interrupt] Cancellation requested. Cleaning up...");
                        cancel_token_clone.cancel();
                        if tokio::signal::ctrl_c().await.is_ok() {
                            eprintln!("\n[Interrupt] Force exiting immediately.");
                            std::process::exit(130);
                        }
                    }
                });
                #[cfg(unix)]
                {
                    let cancel_token_clone = cancel_token.clone();
                    tokio::spawn(async move {
                        use tokio::signal::unix::{signal, SignalKind};
                        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                            sigterm.recv().await;
                            eprintln!("\n[Interrupt] SIGTERM received. Cleaning up...");
                            cancel_token_clone.cancel();
                        }
                    });
                }
                let res = sessions::run_session_action(
                    &load_effective_config(&overrides)?,
                    "branch",
                    &run_id_or_path,
                    Some(prompt),
                    Some(at),
                    cli.api_key.clone(),
                    cancel_token,
                    None,
                    None,
                )
                .await;
                handle_result(
                    res.map(|run_dir| RunReport { run_dir })
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
        },
        Command::Export {
            run_id_or_path,
            format: export_format,
        } => {
            let config = load_effective_config(&overrides)?;
            let res = export::export_run(&config, &run_id_or_path, export_format);
            handle_result(res, format, quiet)?;
        }
        Command::Verify(command) => match command.command {
            VerifySubcommand::Run { run_id_or_path } => {
                let config = load_effective_config(&overrides)?;
                let res = verify::verify_run(&config, &run_id_or_path).await;
                handle_result(res, format, quiet)?;
            }
        },
        Command::Policy(command) => match command.command {
            PolicySubcommand::Validate => {
                let res: Result<PolicyValidateReport, gestalt_core::HarnessError> =
                    policy::validate_policy(&overrides);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            PolicySubcommand::Explain { tool, input } => {
                let res: Result<PolicyExplainReport, Box<dyn std::error::Error>> =
                    policy::explain_policy(&overrides, &tool, &input).await;
                handle_result(res, format, quiet)?;
            }
            PolicySubcommand::Test { tool, input, mode } => {
                let res: Result<PolicyTestReport, Box<dyn std::error::Error>> =
                    policy::test_policy(&overrides, &tool, &input, mode.as_deref()).await;
                handle_result(res, format, quiet)?;
            }
        },
        Command::Context(command) => match command.command {
            ContextSubcommand::Explain { prompt, run } => {
                let res: Result<ContextExplainReport, Box<dyn std::error::Error>> =
                    context::explain_context(&overrides, prompt.as_deref(), run.as_deref()).await;
                handle_result(res, format, quiet)?;
            }
        },
        Command::Tools(command) => match command.command {
            ToolsSubcommand::List => {
                let res = tools::list_tools(&overrides);
                handle_result(res, format, quiet)?;
            }
            ToolsSubcommand::Inspect { tool } => {
                let res = tools::inspect_tool(&overrides, &tool);
                handle_result(res, format, quiet)?;
            }
            ToolsSubcommand::Classify { sub } => match sub {
                ClassifySubcommand::Bash { command } => {
                    let res = tools::classify_bash(&overrides, &command);
                    handle_result(res, format, quiet)?;
                }
            },
        },
        Command::Runtime(command) => match command.command {
            RuntimeSubcommand::Inspect => {
                let res = gestalt_cli::runtime::inspect_runtime(&overrides, cli.api_key.clone())
                    .await
                    .map(|inspect| RuntimeInspectReport { inspect });
                handle_result(res, format, quiet)?;
            }
            RuntimeSubcommand::Events => {
                let res = gestalt_cli::runtime::get_runtime_events(&overrides, cli.api_key.clone())
                    .await
                    .map(|events| RuntimeEventsReport { events });
                handle_result(res, format, quiet)?;
            }
            RuntimeSubcommand::Doctor => {
                let res = gestalt_cli::runtime::runtime_doctor(&overrides)
                    .map(|checks| RuntimeDoctorReport { checks });
                handle_result(res, format, quiet)?;
            }
        },
        Command::Extension(command) => match command.command {
            ExtensionSubcommand::List => {
                let res = gestalt_cli::runtime::list_extensions(&overrides)
                    .map(|extensions| ExtensionsListReport { extensions });
                handle_result(res, format, quiet)?;
            }
            ExtensionSubcommand::Enable { id } => {
                let res = gestalt_cli::runtime::enable_extension(&overrides, &id).map(|_| {
                    ExtensionActionReport {
                        action: "enable".to_string(),
                        extension_id: id.clone(),
                        success: true,
                        message: format!("Extension '{}' enabled.", id),
                    }
                });
                handle_result(res, format, quiet)?;
            }
            ExtensionSubcommand::Disable { id } => {
                let res = gestalt_cli::runtime::disable_extension(&overrides, &id).map(|_| {
                    ExtensionActionReport {
                        action: "disable".to_string(),
                        extension_id: id.clone(),
                        success: true,
                        message: format!("Extension '{}' disabled.", id),
                    }
                });
                handle_result(res, format, quiet)?;
            }
            ExtensionSubcommand::Inspect { id } => {
                let res = gestalt_cli::runtime::inspect_extension(&overrides, &id)
                    .and_then(|opt| {
                        opt.ok_or_else(|| format!("Extension '{}' not found", id).into())
                    })
                    .map(|manifest| ExtensionInspectReport { manifest });
                handle_result(res, format, quiet)?;
            }
            ExtensionSubcommand::Reload => {
                let res = gestalt_cli::runtime::list_extensions(&overrides).map(|extensions| {
                    ExtensionActionReport {
                        action: "reload".to_string(),
                        extension_id: "all".to_string(),
                        success: true,
                        message: format!(
                            "Reloaded extensions. Active count: {}",
                            extensions.iter().filter(|e| e.enabled).count()
                        ),
                    }
                });
                handle_result(res, format, quiet)?;
            }
            ExtensionSubcommand::Validate { path } => {
                let res = gestalt_cli::runtime::validate_extension(&path)
                    .map(|manifest| ExtensionInspectReport { manifest });
                handle_result(res, format, quiet)?;
            }
        },
        Command::Skill(command) => match command.command {
            SkillSubcommand::List => {
                let res = gestalt_cli::runtime::list_skills(&overrides)
                    .map(|skills| SkillsListReport { skills });
                handle_result(res, format, quiet)?;
            }
            SkillSubcommand::Inspect { name } => {
                let res = gestalt_cli::runtime::inspect_skill(&overrides, &name)
                    .and_then(|opt| opt.ok_or_else(|| format!("Skill '{}' not found", name).into()))
                    .map(|skill| SkillInspectReport {
                        name: skill.name,
                        description: skill.description,
                        skill_root: skill.skill_root.to_string_lossy().to_string(),
                        manifest_path: skill.manifest_path.to_string_lossy().to_string(),
                        manifest_hash: skill.manifest_hash,
                        trust_level: format!("{:?}", skill.trust_level),
                        source: format!("{:?}", skill.source),
                        license: skill.license,
                        compatibility: skill.compatibility,
                        allowed_tools: skill.allowed_tools,
                    });
                handle_result(res, format, quiet)?;
            }
            SkillSubcommand::Activate { name } => {
                let res = gestalt_cli::runtime::activate_skill(&overrides, &name).map(|_| {
                    SkillActionReport {
                        action: "activate".to_string(),
                        skill_name: name.clone(),
                        success: true,
                        message: format!("Skill '{}' activated.", name),
                    }
                });
                handle_result(res, format, quiet)?;
            }
            SkillSubcommand::Deactivate { name } => {
                let res = gestalt_cli::runtime::deactivate_skill(&overrides, &name).map(|_| {
                    SkillActionReport {
                        action: "deactivate".to_string(),
                        skill_name: name.clone(),
                        success: true,
                        message: format!("Skill '{}' deactivated.", name),
                    }
                });
                handle_result(res, format, quiet)?;
            }
            SkillSubcommand::Validate { path } => {
                let res = gestalt_cli::runtime::validate_skill(&path).map(|manifest| {
                    SkillInspectReport {
                        name: manifest.name,
                        description: manifest.description,
                        skill_root: path.to_string_lossy().to_string(),
                        manifest_path: path.join("SKILL.md").to_string_lossy().to_string(),
                        manifest_hash: "validated".to_string(),
                        trust_level: "Explicit".to_string(),
                        source: "ExplicitPath".to_string(),
                        license: manifest.license,
                        compatibility: manifest.compatibility,
                        allowed_tools: manifest.allowed_tools,
                    }
                });
                handle_result(res, format, quiet)?;
            }
        },
        Command::Connect {
            provider,
            api_key,
            no_keychain,
            set_default,
            name,
            base_url,
            default_model,
            api_key_env,
        } => {
            let res = async {
                let config = load_effective_config(&overrides)?;
                gestalt_cli::connect::connect_provider(
                    &config,
                    &provider,
                    api_key,
                    no_keychain,
                    set_default,
                    name,
                    base_url,
                    default_model,
                    api_key_env,
                )
            }
            .await;
            handle_result(
                res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                format,
                quiet,
            )?;
        }
        Command::Disconnect { provider, force } => {
            let res = async {
                let config = load_effective_config(&overrides)?;
                gestalt_cli::connect::disconnect_provider(&config, &provider, force)
            }
            .await;
            handle_result(
                res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                format,
                quiet,
            )?;
        }
        Command::Profiles(command) => match command.command {
            ProfilesSubcommand::List => {
                let res = (|| {
                    let config = load_effective_config(&overrides)?;
                    gestalt_cli::profiles::list_profiles(&config)
                })();
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ProfilesSubcommand::Inspect { name } => {
                let res = (|| {
                    let config = load_effective_config(&overrides)?;
                    gestalt_cli::profiles::inspect_profile(&config, &name)
                })();
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ProfilesSubcommand::Use { name } => {
                let res = (|| {
                    let config = load_effective_config(&overrides)?;
                    gestalt_cli::profiles::use_profile(&config, &name)
                })();
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
        },
        Command::Doctor { live } => {
            let res = doctor::diagnose_workspace(&overrides, live).await;
            handle_result(
                res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                format,
                quiet,
            )?;
        }
    }

    Ok(())
}
