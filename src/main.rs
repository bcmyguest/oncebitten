use anyhow::Result;
use clap::{Parser, Subcommand};
use oncebitten::config::{self, Inputs};
use oncebitten::rules::{self, Failure};

#[derive(Parser)]
#[command(
    name = "oncebitten",
    version,
    about = "Turns repeated agent mistakes into git-enforced rules"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect the layered configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Sync, list and validate rules
    Rules {
        #[command(subcommand)]
        action: RulesAction,
    },
}

#[derive(Subcommand)]
enum RulesAction {
    /// Clone or fetch each source and write .oncebitten/rules.lock
    Sync,
    /// Print each resolved rule as id, mode, status and origin
    List {
        /// Check sources against rules.lock without fetching; exit 2 if they differ
        #[arg(long)]
        ci: bool,
    },
    /// Load and resolve every rule; exit 1 on rule errors, 2 if the check cannot run
    Validate {
        /// Check sources against rules.lock without fetching; exit 2 if they differ
        #[arg(long)]
        ci: bool,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the merged configuration as TOML
    Show {
        /// Annotate each value with the layer it came from
        #[arg(long)]
        origin: bool,
    },
    /// Print the configuration file paths
    Path,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Config { action } => run_config(action),
        Command::Rules { action } => run_rules(action),
    }
}

fn run_rules(action: RulesAction) -> Result<()> {
    let inputs = Inputs::from_process()?;
    match action {
        RulesAction::Sync => rules::sync(&inputs)?,
        RulesAction::List { ci } => match rules::load(&inputs, ci) {
            Ok(resolved) => {
                for entry in &resolved {
                    println!(
                        "{}  {}  {}  {}",
                        entry.id,
                        entry.rule.mode.as_str(),
                        entry.rule.status.as_str(),
                        entry.layer.label()
                    );
                }
            }
            Err(failure) => exit_with(failure),
        },
        RulesAction::Validate { ci } => {
            if let Err(failure) = rules::load(&inputs, ci) {
                exit_with(failure);
            }
        }
    }
    Ok(())
}

fn exit_with(failure: Failure) -> ! {
    match failure {
        Failure::Invalid(errors) => {
            for error in errors {
                eprintln!("error: {error:#}");
            }
            std::process::exit(1)
        }
        Failure::CouldNotRun(error) => {
            eprintln!("error: {error:#}");
            std::process::exit(2)
        }
    }
}

fn run_config(action: ConfigAction) -> Result<()> {
    let inputs = Inputs::from_process()?;
    match action {
        ConfigAction::Show { origin } => {
            let loaded = config::load(&inputs)?;
            for warning in &loaded.warnings {
                eprintln!("warning: {warning}");
            }
            print!("{}", config::render(&loaded, origin)?);
        }
        ConfigAction::Path => {
            let paths = config::config_paths(&inputs)?;
            let missing = if paths.user.is_file() {
                ""
            } else {
                " (missing)"
            };
            println!("user     {}{missing}", paths.user.display());
            match paths.project {
                Some(path) => println!("project  {}", path.display()),
                None => println!("project  none"),
            }
        }
    }
    Ok(())
}
