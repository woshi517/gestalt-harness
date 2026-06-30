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
    use serde::Serialize;

    #[derive(Clone, Copy, clap::ValueEnum)]
    enum OutputFormat {
        Text,
        Json,
    }

    #[derive(Parser)]
    #[command(name = "gestalt")]
    struct Cli {
        #[arg(long, default_value = "text", global = true)]
        format: OutputFormat,
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

    #[derive(Serialize)]
    struct FeatureDisabledEnvelope<'a> {
        schema_version: u32,
        status: &'static str,
        kind: &'static str,
        data: Option<()>,
        error: FeatureDisabledError<'a>,
        warnings: [(); 0],
    }

    #[derive(Serialize)]
    struct FeatureDisabledError<'a> {
        code: &'static str,
        message: &'a str,
        retryable: bool,
        details: Option<()>,
        correlation_id: Option<()>,
    }

    pub fn main() {
        let cli = Cli::parse();
        if cli.command.is_some() {
            let error = gestalt_core::ConfigError::FeatureDisabled {
                feature: "product-integrations".to_string(),
                operation: "command dispatch".to_string(),
            };
            if matches!(cli.format, OutputFormat::Json) {
                let message = error.to_string();
                let envelope = FeatureDisabledEnvelope {
                    schema_version: 1,
                    status: "error",
                    kind: "error",
                    data: None,
                    error: FeatureDisabledError {
                        code: "FEATURE_DISABLED",
                        message: &message,
                        retryable: false,
                        details: None,
                        correlation_id: None,
                    },
                    warnings: [],
                };
                eprintln!(
                    "{}",
                    serde_json::to_string(&envelope)
                        .unwrap_or_else(|_| r#"{"status":"error"}"#.to_string())
                );
            } else {
                eprintln!("FEATURE_DISABLED: {error}");
            }
            std::process::exit(7);
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
