use agent_stats_core::backend::{FetchRequest, fetch_usage};
use agent_stats_core::config::AppConfig;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent-stats-cli", version, about = "Agent Stats CLI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Usage {
        #[arg(long)]
        status: bool,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        pretty: bool,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    Dump,
    Validate,
}

fn main() {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Commands::Usage {
        status: false,
        provider: None,
        source: None,
        pretty: false,
    });

    match command {
        Commands::Usage {
            status,
            provider,
            source,
            pretty,
        } => {
            if let Err(error) = run_usage(status, provider, source, pretty) {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
        Commands::Config { command } => {
            if let Err(error) = run_config(command) {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
    }
}

fn run_usage(
    status: bool,
    provider: Option<String>,
    source: Option<String>,
    pretty: bool,
) -> anyhow::Result<()> {
    let request = FetchRequest {
        include_status: status,
        provider,
        source,
        pretty,
    };
    let result = fetch_usage(request)?;
    let output = if pretty {
        serde_json::to_string_pretty(&result.payloads)?
    } else {
        serde_json::to_string(&result.payloads)?
    };
    println!("{output}");
    Ok(())
}

fn run_config(command: ConfigCommand) -> anyhow::Result<()> {
    match command {
        ConfigCommand::Dump => {
            let config = AppConfig::load_or_default()?;
            let output = serde_json::to_string_pretty(&config)?;
            println!("{output}");
        }
        ConfigCommand::Validate => {
            let _config = AppConfig::load()?;
            println!("Config OK");
        }
    }
    Ok(())
}
