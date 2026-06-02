use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use gestalt_cli::{
    auth::resolve_auth,
    config::{load_effective_config, validate_workspace_config, CliOverrides},
    cost::calculate_cost,
    models::{inspect_model, list_models},
    output::{
        AuthResolveReport, CliErrorPayload, CliReport, ConfigValidateReport, CostReportWrapper,
        JsonEnvelope, ModelsInspectReport, ModelsListReport, ModelsRefreshReport,
        ModelsSelectReport, OutputFormat, ProvidersDoctorReport, ProvidersInspectReport,
        ProvidersListReport, ReplayReport, RunReport, WorkspaceDoctorReport, WorkspaceInfoReport,
        WorkspaceInitReport, WorkspaceSnapshotReport, WorkspaceStatusReport,
    },
    providers::{doctor_provider, inspect_provider, list_providers},
    replay::replay_display,
    run::run_prompt,
    runs,
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
    #[arg(long, default_value = "text", global = true)]
    format: String,
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
    Init {
        #[arg(long)]
        force: bool,
    },
    Status,
    Workspace(WorkspaceCommand),
    Runs(RunsCommand),
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
    },
    Delete {
        run_id_or_path: String,
        #[arg(long, short)]
        yes: bool,
        #[arg(long)]
        json: bool,
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
}

#[derive(Args)]
struct AuthCommand {
    #[command(subcommand)]
    command: AuthSubcommand,
}

#[derive(Subcommand)]
enum AuthSubcommand {
    Resolve { provider: String },
}

#[derive(Args)]
struct ProvidersCommand {
    #[command(subcommand)]
    command: ProvidersSubcommand,
}

#[derive(Subcommand)]
enum ProvidersSubcommand {
    List,
    Inspect { provider: String },
    Test { provider: String },
    Doctor { provider: Option<String> },
}

#[derive(Args)]
struct ModelsCommand {
    #[command(subcommand)]
    command: ModelsSubcommand,
}

#[derive(Subcommand)]
enum ModelsSubcommand {
    List,
    Inspect { model: String },
    Refresh,
    Select { model: String },
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

fn handle_result<T: CliReport>(
    res: Result<T, Box<dyn std::error::Error>>,
    format: OutputFormat,
    quiet: bool,
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
                    if !quiet {
                        let text = report.render_text();
                        if !text.is_empty() {
                            println!("{}", text);
                        }
                    }
                }
            }
            Ok(())
        }
        Err(err) => {
            let payload = map_to_cli_error(err.as_ref());
            match format {
                OutputFormat::Json => {
                    let envelope = JsonEnvelope {
                        schema_version: 1,
                        kind: "error".to_string(),
                        data: payload,
                    };
                    eprintln!("{}", serde_json::to_string(&envelope)?);
                }
                OutputFormat::Text => {
                    eprintln!("error: {}", payload.message);
                }
            }
            std::process::exit(1);
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let overrides = CliOverrides {
        provider: cli.provider.clone(),
        model: cli.model.clone(),
        mode: cli.mode.clone(),
        max_turns: cli.max_turns,
        workspace: cli.workspace.clone(),
    };

    let format = match cli.format.to_lowercase().as_str() {
        "json" => OutputFormat::Json,
        _ => OutputFormat::Text,
    };
    let quiet = cli.quiet;

    match cli.command {
        Command::Run { prompt } => {
            let res: Result<RunReport, gestalt_core::HarnessError> = async {
                let config = load_effective_config(&overrides)?;
                let run_dir = run_prompt(&config, &prompt).await?;
                Ok(RunReport { run_dir })
            }
            .await;
            handle_result(
                res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                format,
                quiet,
            )?;
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
                let res: Result<ProvidersDoctorReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let result = doctor_provider(&config, &provider)?;
                    Ok(ProvidersDoctorReport {
                        results: vec![result],
                    })
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
            ProvidersSubcommand::Doctor { provider } => {
                let res: Result<ProvidersDoctorReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let mut results = Vec::new();
                    if let Some(provider) = provider {
                        results.push(doctor_provider(&config, &provider)?);
                    } else {
                        for p in list_providers(&config) {
                            results.push(doctor_provider(&config, &p)?);
                        }
                    }
                    Ok(ProvidersDoctorReport { results })
                })(
                );
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    format,
                    quiet,
                )?;
            }
        },
        Command::Models(command) => match command.command {
            ModelsSubcommand::List => {
                let res: Result<ModelsListReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let models = list_models(&config);
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
            ModelsSubcommand::Refresh => {
                let res: Result<ModelsRefreshReport, gestalt_core::HarnessError> = (|| {
                    let config = load_effective_config(&overrides)?;
                    let count = list_models(&config).len();
                    Ok(ModelsRefreshReport { count })
                })(
                );
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
        },
        Command::Init { force } => {
            let res: Result<WorkspaceInitReport, gestalt_core::HarnessError> = (|| {
                let workspace_root = overrides.workspace.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
                init_workspace(&workspace_root, force)
            })();
            handle_result(
                res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                format,
                quiet,
            )?;
        }
        Command::Status => {
            let res: Result<WorkspaceStatusReport, gestalt_core::HarnessError> =
                (|| status_workspace(&overrides))();
            handle_result(
                res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                format,
                quiet,
            )?;
        }
        Command::Workspace(command) => match command.command {
            WorkspaceSubcommand::Info => {
                let res: Result<WorkspaceInfoReport, gestalt_core::HarnessError> =
                    (|| info_workspace(&overrides))();
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
                let res: Result<WorkspaceDoctorReport, gestalt_core::HarnessError> =
                    (|| doctor_workspace(&overrides))();
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
            RunsSubcommand::Inspect { run_id_or_path, json } => {
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
                    let payload = map_to_cli_error(&e);
                    match format {
                        OutputFormat::Json => {
                            let envelope = JsonEnvelope {
                                schema_version: 1,
                                kind: "error".to_string(),
                                data: payload,
                            };
                            eprintln!("{}", serde_json::to_string(&envelope)?);
                        }
                        OutputFormat::Text => {
                            eprintln!("error: {}", payload.message);
                        }
                    }
                    std::process::exit(1);
                }
            }
            RunsSubcommand::Prune { older_than, dry_run, yes, json } => {
                let config = load_effective_config(&overrides)?;
                let fmt = if json { OutputFormat::Json } else { format };
                let res = runs::prune_runs(&config, older_than, dry_run, yes);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    fmt,
                    quiet,
                )?;
            }
            RunsSubcommand::Delete { run_id_or_path, yes, json } => {
                let config = load_effective_config(&overrides)?;
                let fmt = if json { OutputFormat::Json } else { format };
                let res = runs::delete_run(&config, &run_id_or_path, yes);
                handle_result(
                    res.map_err(|e| Box::new(e) as Box<dyn std::error::Error>),
                    fmt,
                    quiet,
                )?;
            }
        },
    }

    Ok(())
}
