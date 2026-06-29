#[cfg(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
))]
mod full;

#[cfg(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    full::main()
}

#[cfg(not(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
)))]
mod minimal {
    use clap::{Parser, Subcommand};

    #[derive(Parser)]
    #[command(name = "gestalt")]
    struct Cli {
        #[command(subcommand)]
        command: Option<Command>,
    }

    #[derive(Subcommand)]
    enum Command {
        Run(Args),
        Chat(Args),
        Tui(Args),
        Replay(Args),
        Cost(Args),
        Config(Args),
        Auth(Args),
        Providers(Args),
        Models(Args),
        Connect(Args),
        Disconnect(Args),
        Profiles(Args),
        Init(Args),
        Status(Args),
        Workspace(Args),
        Runs(Args),
        Trace(Args),
        Sessions(Args),
        Export(Args),
        Verify(Args),
        Policy(Args),
        Context(Args),
        Tools(Args),
        Runtime(Args),
        Extension(Args),
        Skill(Args),
        Doctor(Args),
    }

    #[derive(clap::Args)]
    #[command(trailing_var_arg = true)]
    struct Args {
        #[arg(allow_hyphen_values = true)]
        arguments: Vec<String>,
    }

    pub fn main() {
        let cli = Cli::parse();
        if cli.command.is_some() {
            let error = gestalt_core::ConfigError::FeatureDisabled {
                feature: "product-integrations".to_string(),
                operation: "command dispatch".to_string(),
            };
            eprintln!("FEATURE_DISABLED: {error}");
            std::process::exit(2);
        }
    }
}

#[cfg(not(all(
    feature = "providers",
    feature = "tools",
    feature = "trace",
    feature = "mcp",
    feature = "skills",
    feature = "verify"
)))]
fn main() {
    minimal::main();
}
