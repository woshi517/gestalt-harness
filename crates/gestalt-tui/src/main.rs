use std::path::PathBuf;

use clap::Parser;
use gestalt_app::config::{load_effective_config, CliOverrides};

#[derive(Parser)]
#[command(name = "gestalt-tui")]
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
    #[arg(long)]
    profile: Option<String>,
    #[arg(long)]
    api_key: Option<String>,
    #[arg(long)]
    context_window: Option<usize>,
    #[arg(long)]
    verbose: bool,
    #[arg(long)]
    quiet: bool,
    #[arg(long)]
    no_color: bool,
    #[arg(long)]
    run: Option<String>,
    #[arg(long)]
    prompt: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let config = load_effective_config(&CliOverrides {
        provider: cli.provider,
        model: cli.model,
        mode: cli.mode,
        max_turns: cli.max_turns,
        workspace: cli.workspace,
        profile: cli.profile,
        skills: Vec::new(),
        context_window_override: cli.context_window,
    })?;

    gestalt_tui::run_tui(
        &config,
        cli.run,
        cli.prompt,
        cli.api_key,
        gestalt_core::CancelToken::new(),
    )
    .await?;

    Ok(())
}
