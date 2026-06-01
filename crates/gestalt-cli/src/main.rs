use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use gestalt_cli::{
    auth::resolve_auth,
    config::{load_effective_config, validate_workspace_config, CliOverrides},
    cost::{calculate_cost, render_cost},
    models::{inspect_model, list_models, refresh_models, select_model},
    providers::{doctor_provider, inspect_provider, list_providers},
    replay::replay_display,
    run::run_prompt,
};

#[derive(Parser)]
#[command(name = "gestalt")]
struct Cli {
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    mode: Option<String>,
    #[arg(long)]
    max_turns: Option<usize>,
    #[arg(long)]
    provider: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Run { prompt: String },
    Replay { path: PathBuf },
    Cost { path: PathBuf },
    Config(ConfigCommand),
    Auth(AuthCommand),
    Providers(ProvidersCommand),
    Models(ModelsCommand),
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

    match cli.command {
        Command::Run { prompt } => {
            let config = load_effective_config(&overrides)?;
            let run_dir = run_prompt(&config, &prompt).await?;
            println!("run_dir={}", run_dir.display());
        }
        Command::Replay { path } => println!("{}", replay_display(&path)?),
        Command::Cost { path } => println!("{}", render_cost(&calculate_cost(&path)?)),
        Command::Config(command) => match command.command {
            ConfigSubcommand::Validate => {
                let config = validate_workspace_config(&overrides)?;
                println!("valid workspace={}", config.workspace_root.display());
            }
        },
        Command::Auth(command) => match command.command {
            AuthSubcommand::Resolve { provider } => {
                let config = load_effective_config(&overrides)?;
                println!("{}", resolve_auth(&config, &provider)?);
            }
        },
        Command::Providers(command) => match command.command {
            ProvidersSubcommand::List => {
                let config = load_effective_config(&overrides)?;
                for provider in list_providers(&config) {
                    println!("{provider}");
                }
            }
            ProvidersSubcommand::Inspect { provider } => {
                let config = load_effective_config(&overrides)?;
                println!("{}", inspect_provider(&config, &provider)?);
            }
            ProvidersSubcommand::Test { provider } => {
                let config = load_effective_config(&overrides)?;
                println!("{}", doctor_provider(&config, &provider)?);
            }
            ProvidersSubcommand::Doctor { provider } => {
                let config = load_effective_config(&overrides)?;
                if let Some(provider) = provider {
                    println!("{}", doctor_provider(&config, &provider)?);
                } else {
                    for provider in list_providers(&config) {
                        println!("{}", doctor_provider(&config, &provider)?);
                    }
                }
            }
        },
        Command::Models(command) => match command.command {
            ModelsSubcommand::List => {
                let config = load_effective_config(&overrides)?;
                for model in list_models(&config) {
                    println!("{}", model.qualified_id);
                }
            }
            ModelsSubcommand::Inspect { model } => {
                let config = load_effective_config(&overrides)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&inspect_model(&config, &model)?)?
                );
            }
            ModelsSubcommand::Refresh => {
                let config = load_effective_config(&overrides)?;
                println!("{}", refresh_models(&config));
            }
            ModelsSubcommand::Select { model } => {
                let config = load_effective_config(&overrides)?;
                println!("{}", select_model(&config, &model)?);
            }
        },
    }

    Ok(())
}
