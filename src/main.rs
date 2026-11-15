use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use oncebitten::capture;
use oncebitten::check::{self, Checked};
use oncebitten::config::{self, Inputs, Platform};
use oncebitten::engine::Outcome;
use oncebitten::hooks::{self, Scope};
use oncebitten::host::Host;
use oncebitten::proof::{self, BackendKind};
use oncebitten::replay::openshell::OpenShell;
use oncebitten::replay::{Backend, LocalRunner, Verdict};
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
    /// Sync, list and validate rules
    Rules {
        #[command(subcommand)]
        action: RulesAction,
    },
    /// Run each active rule against changes since a revision; exit 0 clean, 1 fired, 2 could not run
    Check {
        /// Revision the working tree is compared with
        #[arg(long, default_value = "HEAD")]
        base: String,
    },
    /// Replay an ast-grep rule against its fixtures and corpus; exit 0 proven, 1 rejected, 2 could not run
    Replay {
        /// Qualified rule id, as `rules list` prints it
        rule: String,
        /// The oncebitten-checker binary to bake into the sandbox image (default: next to oncebitten)
        #[arg(long)]
        checker: Option<PathBuf>,
        /// OpenShell gateway endpoint (default: the openshell CLI's own)
        #[arg(long)]
        gateway: Option<String>,
        /// Run the checker on the host without a sandbox; reports only, writes no proof
        #[arg(long)]
        local: bool,
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
        Command::Capture { host } => {
            if let Err(err) = capture::run_from_process(&host) {
                eprintln!("oncebitten capture: {err:#}");
            }
            Ok(())
        }
        Command::Config { action } => run_config(action),
        Command::Hooks { action } => run_hooks(action),
        Command::Rules { action } => run_rules(action),
        Command::Check { base } => run_check(&base),
        Command::Replay {
            rule,
            checker,
            gateway,
            local,
        } => run_replay(&rule, checker, gateway, local),
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

fn run_check(base: &str) -> Result<()> {
    let inputs = Inputs::from_process()?;
    let code = match check::run(&inputs, base) {
        Ok(checked) => report_checks(&checked),
        Err(Failure::Invalid(errors)) => {
            for error in errors {
                eprintln!("error: {error:#}");
            }
            2
        }
        Err(Failure::CouldNotRun(error)) => {
            eprintln!("error: {error:#}");
            2
        }
    };
    std::process::exit(code)
}

fn run_replay(
    id: &str,
    checker: Option<PathBuf>,
    gateway: Option<String>,
    local: bool,
) -> Result<()> {
    let inputs = Inputs::from_process()?;
    let rules = match rules::load(&inputs, false) {
        Ok(rules) => rules,
        Err(failure) => exit_could_not_run(failure),
    };
    let Some(resolved) = rules.iter().find(|resolved| resolved.id == id) else {
        eprintln!("error: no rule named `{id}`; see oncebitten rules list");
        std::process::exit(2)
    };
    let checker = match checker {
        Some(path) => path,
        None => default_checker()?,
    };
    let (backend, kind): (Box<dyn Backend>, BackendKind) = if local {
        (Box::new(LocalRunner), BackendKind::Local)
    } else {
        (Box::new(OpenShell::new(gateway)), BackendKind::OpenShell)
    };
    let replayed = proof::replay_rule(&resolved.rule, &checker, backend.as_ref(), kind);
    let code = match &replayed.verdict {
        Verdict::Proven => {
            match &replayed.written {
                Some(path) => println!("{id}: proven; wrote {}", path.display()),
                None => println!(
                    "{id}: proven by the {} runner; no proof written",
                    kind.as_str()
                ),
            }
            0
        }
        Verdict::Rejected(reason) => {
            println!("{id}: rejected: {reason}");
            1
        }
        Verdict::CouldNotRun(reason) => {
            eprintln!("{id}: could not run: {reason}");
            2
        }
    };
    std::process::exit(code)
}

fn default_checker() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the oncebitten binary")?;
    Ok(exe.with_file_name(format!(
        "oncebitten-checker{}",
        std::env::consts::EXE_SUFFIX
    )))
}

fn exit_could_not_run(failure: Failure) -> ! {
    match failure {
        Failure::Invalid(errors) => {
            for error in errors {
                eprintln!("error: {error:#}");
            }
        }
        Failure::CouldNotRun(error) => eprintln!("error: {error:#}"),
    }
    std::process::exit(2)
}

fn report_checks(checked: &[Checked]) -> i32 {
    for item in checked {
        match &item.outcome {
            Outcome::Clean => {}
            Outcome::Fired(findings) => {
                for finding in findings {
                    println!("{}", check::format_finding(&item.id, finding));
                }
            }
            Outcome::CouldNotRun(reason) => eprintln!("{}: could not run: {reason}", item.id),
        }
    }
    check::exit_code(checked)
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

fn run_hooks(action: HooksAction) -> Result<()> {
    let inputs = Inputs::from_process()?;
    let (install, host, scope) = match action {
        HooksAction::Install { host, scope } => (true, host, scope),
        HooksAction::Uninstall { host, scope } => (false, host, scope),
    };
    let host = Host::parse(&host)?;
    let home = config::home_dir(&inputs.vars, Platform::current())
        .context("no absolute home directory in HOME, USERPROFILE or HOMEDRIVE+HOMEPATH")?;
    let path = hooks::install_target(host, scope, &home, &inputs.cwd, &inputs.vars)?;
    let changed = if install {
        hooks::install_exe(host, &path, &hooks::current_exe()?, Platform::current())?
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
