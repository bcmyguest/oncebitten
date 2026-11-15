use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use oncebitten::capture;
use oncebitten::config::{self, Inputs, Platform};
use oncebitten::hooks::{self, Scope};
use oncebitten::host::Host;

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
    /// Record an agent hook payload from stdin (always exits 0)
    Capture {
        /// Agent host that sent the payload
        #[arg(long)]
        host: String,
    },
    /// Inspect the layered configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Install or remove the capture hooks in an agent's settings
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
}

#[derive(Subcommand)]
enum HooksAction {
    /// Add the capture hooks (idempotent)
    Install {
        #[arg(long)]
        host: String,
        #[arg(long, value_enum)]
        scope: Scope,
    },
    /// Remove only oncebitten's capture hooks
    Uninstall {
        #[arg(long)]
        host: String,
        #[arg(long, value_enum)]
        scope: Scope,
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
        Command::Capture { host } => {
            if let Err(err) = capture::run_from_process(&host) {
                eprintln!("oncebitten capture: {err:#}");
            }
            Ok(())
        }
        Command::Config { action } => run_config(action),
        Command::Hooks { action } => run_hooks(action),
    }
}

fn run_hooks(action: HooksAction) -> Result<()> {
    let inputs = Inputs::from_process()?;
    let (install, host, scope) = match action {
        HooksAction::Install { host, scope } => (true, host, scope),
        HooksAction::Uninstall { host, scope } => (false, host, scope),
    };
    let host = Host::parse(&host)?;
    let home = config::home_dir(&inputs.vars, Platform::current())
        .context("no absolute home directory in HOME, USERPROFILE or HOMEDRIVE+HOMEPATH")?;
    let path = hooks::settings_path(scope, &home, &inputs.cwd);
    let changed = if install {
        let exe = hooks::current_exe()?;
        let shell = hooks::default_shell(Platform::current());
        hooks::install_with(host, &path, &hooks::capture_command(&exe, host, shell))?
    } else {
        hooks::uninstall(host, &path)?
    };
    let state = if changed { "changed" } else { "unchanged" };
    println!("{state}  {}", path.display());
    Ok(())
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
