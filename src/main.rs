use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use oncebitten::baseline;
use oncebitten::capture;
use oncebitten::check::{self, Checked};
use oncebitten::config::{self, Inputs, Platform};
use oncebitten::engine::Outcome;
use oncebitten::evidence::{self, Cluster};
use oncebitten::exceptions::{self, Exception};
use oncebitten::gate::{self, Stage};
use oncebitten::hooks::{self, Scope};
use oncebitten::host::Host;
use oncebitten::proof::{self, BackendKind};
use oncebitten::replay::openshell::OpenShell;
use oncebitten::replay::{Backend, LocalRunner, Verdict};
use oncebitten::rules::{self, Failure};
use oncebitten::store::{self, Store};
use oncebitten::triage;

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
    /// Inspect mistake clusters built from captured signals
    Evidence {
        #[command(subcommand)]
        action: EvidenceAction,
    },
    /// Ask the configured host for a one-shot verdict on an evidence cluster; exit 0 verdict, 2 could not run
    Triage {
        /// Cluster id from `oncebitten evidence list`
        id: String,
        /// Host to ask (default: the configured triage default)
        #[arg(long)]
        host: Option<String>,
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
    /// Block a push or pull request on new violations of active block rules
    Gate {
        #[command(subcommand)]
        stage: GateStage,
    },
    /// Manage .oncebitten/exceptions.toml: findings a rule lets through on named paths until a date
    Exception {
        #[command(subcommand)]
        action: ExceptionAction,
    },
    /// Manage .oncebitten/baseline.json, the violations the gate lets through
    Baseline {
        #[command(subcommand)]
        action: BaselineAction,
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
enum EvidenceAction {
    /// Print candidate clusters: id, class, status, sessions, occurrences, tool, pattern
    List {
        /// Include clusters below the threshold and rejected clusters
        #[arg(long)]
        all: bool,
    },
    /// Print one cluster and the provenance of each occurrence
    Show { id: String },
    /// Record that a cluster is not a rule, so it is never a candidate again
    Reject {
        id: String,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum ExceptionAction {
    /// Let a rule's findings through on the given paths until a date
    Add {
        /// Qualified rule id, as `rules list` prints it
        rule: String,
        /// Globs of repo-relative paths; `**` covers every path and pathless findings
        #[arg(long, required = true, num_args = 1..)]
        paths: Vec<String>,
        /// Why the rule does not apply here
        #[arg(long)]
        reason: String,
        /// Last day (UTC) the exception applies, as YYYY-MM-DD
        #[arg(long)]
        expires: String,
    },
    /// Print every exception as rule, paths, expires, live or expired, and reason
    List,
}

#[derive(Subcommand)]
enum BaselineAction {
    /// Replace the baseline with the current violations of active block rules in every tracked file
    Update,
}

#[derive(Subcommand)]
enum GateStage {
    /// The pre-push hook: checks what the push sends; a missing or stale proof is a warning
    PrePush {
        /// Check changes since the merge-base with this ref instead of the pushed range
        #[arg(long)]
        base: Option<String>,
    },
    /// CI: checks the pull request against the merge-base with its target; needs rules.lock and valid proofs
    Pr {
        /// The pull request's target branch, such as origin/main
        #[arg(long)]
        base: String,
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
    Sync {
        /// Check out the commits in rules.lock, fetching only a commit the clone lacks; never writes the lock; exit 2 if it cannot
        #[arg(long)]
        frozen: bool,
    },
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
        Command::Evidence { action } => run_evidence(action),
        Command::Triage { id, host } => run_triage(&id, host.as_deref()),
        Command::Config { action } => run_config(action),
        Command::Hooks { action } => run_hooks(action),
        Command::Rules { action } => run_rules(action),
        Command::Check { base } => run_check(&base),
        Command::Gate { stage } => run_gate(stage),
        Command::Exception { action } => run_exception(action),
        Command::Baseline {
            action: BaselineAction::Update,
        } => run_baseline_update(),
        Command::Replay {
            rule,
            checker,
            gateway,
            local,
        } => run_replay(&rule, checker, gateway, local),
    }
}

fn run_triage(id: &str, host: Option<&str>) -> Result<()> {
    match triage_verdict(id, host) {
        Ok(verdict) => {
            println!("{}", serde_json::to_string_pretty(&verdict)?);
            Ok(())
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::exit(2)
        }
    }
}

fn triage_verdict(id: &str, host: Option<&str>) -> Result<triage::verdict::Verdict> {
    let inputs = Inputs::from_process()?;
    let loaded = config::load(&inputs)?;
    let store = Store::open(&store::events_db_path_for(
        &inputs.vars,
        Platform::current(),
    )?)?;
    let clusters = evidence::load(&store)?;
    let cluster = evidence::find(&clusters, id)?;
    let host = host
        .map(|name| config::Host::parse(name).with_context(|| format!("unknown host `{name}`")))
        .transpose()?;
    let (host, agent) = triage::flow::target(&loaded.config.triage, host)?;
    let input = triage::flow::prompt_input(cluster);
    triage::flow::run(
        &triage::run::ProcessRunner,
        host,
        &agent,
        &input,
        triage::flow::DEFAULT_TIMEOUT,
    )
}

fn run_evidence(action: EvidenceAction) -> Result<()> {
    let inputs = Inputs::from_process()?;
    let store = Store::open(&store::events_db_path_for(
        &inputs.vars,
        Platform::current(),
    )?)?;
    match action {
        EvidenceAction::List { all } => {
            for cluster in evidence::load(&store)? {
                if all || cluster.is_candidate() {
                    println!("{}", cluster_line(&cluster));
                }
            }
        }
        EvidenceAction::Show { id } => {
            let clusters = evidence::load(&store)?;
            let cluster = evidence::find(&clusters, &id)?;
            println!("{}", cluster_line(cluster));
            println!("source: {}", cluster.source.as_str());
            println!("repos: {}", cluster.repos().join(", "));
            if let Some(rejection) = &cluster.rejection {
                println!("rejected: {}", rejection.reason);
            }
            for occurrence in &cluster.occurrences {
                let event = &occurrence.event;
                println!(
                    "{}  {}  {}  {}  event {}  {}  {}  head {}",
                    event.ts,
                    event.host,
                    event.session_id,
                    event.repo.as_deref().unwrap_or("-"),
                    event.id,
                    occurrence.signals.join(","),
                    event.summary,
                    event.git_head.as_deref().unwrap_or("-")
                );
            }
        }
        EvidenceAction::Reject { id, reason } => {
            evidence::reject(&store, &id, now_millis()?, &reason)?;
            println!("rejected {id}");
        }
    }
    Ok(())
}

fn cluster_line(cluster: &Cluster) -> String {
    format!(
        "{}  {}  {}  sessions={}  occurrences={}  {}  {}",
        cluster.id,
        cluster.class,
        cluster.status(),
        cluster.sessions(),
        cluster.occurrences.len(),
        cluster.tool.as_deref().unwrap_or("-"),
        cluster.pattern
    )
}

fn now_millis() -> Result<i64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the unix epoch")?
        .as_millis() as i64)
}

fn run_rules(action: RulesAction) -> Result<()> {
    let inputs = Inputs::from_process()?;
    match action {
        RulesAction::Sync { frozen: true } => {
            if let Err(failure) = rules::sync_frozen(&inputs) {
                exit_with(failure);
            }
        }
        RulesAction::Sync { frozen: false } => rules::sync(&inputs)?,
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
        Ok(ran) => {
            let code = report_checks(&ran.checked);
            report_excepted(ran.excepted);
            code
        }
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

fn run_gate(stage: GateStage) -> Result<()> {
    let inputs = Inputs::from_process()?;
    let (stage, base) = match &stage {
        GateStage::PrePush { base } => (Stage::PrePush, base.as_deref()),
        GateStage::Pr { base } => (Stage::Pr, Some(base.as_str())),
    };
    let report = match gate::run(&inputs, stage, base) {
        Ok(report) => report,
        Err(failure) => exit_could_not_run(failure),
    };
    for (id, validity) in &report.proofs {
        let level = match stage {
            Stage::Pr => "error",
            Stage::PrePush => "warning",
        };
        eprintln!("{level}: {}", gate::describe(id, validity));
    }
    report_checks(&report.checked);
    if report.known > 0 {
        eprintln!(
            "{} known violation(s) matched .oncebitten/baseline.json",
            report.known
        );
    }
    report_excepted(report.excepted);
    for exception in &report.expired {
        eprintln!(
            "warning: expired exception: {}",
            exceptions::describe(exception)
        );
    }
    std::process::exit(report.exit_code(stage))
}

fn report_excepted(excepted: usize) {
    if excepted > 0 {
        eprintln!("{excepted} violation(s) excepted by .oncebitten/exceptions.toml");
    }
}

fn run_exception(action: ExceptionAction) -> Result<()> {
    let inputs = Inputs::from_process()?;
    let Some(repo) = config::repo_root(&inputs.cwd) else {
        exit_could_not_run(Failure::CouldNotRun(anyhow::anyhow!(
            "not inside a git repository"
        )))
    };
    match action {
        ExceptionAction::Add {
            rule,
            paths,
            reason,
            expires,
        } => {
            let resolved = match rules::load(&inputs, false) {
                Ok(resolved) => resolved,
                Err(failure) => exit_could_not_run(failure),
            };
            if !resolved.iter().any(|entry| entry.id == rule) {
                eprintln!("error: no rule named `{rule}`; see oncebitten rules list");
                std::process::exit(2)
            }
            let exception = Exception::new(&rule, &paths, &reason, &expires)?;
            let path = exceptions::add(&repo, &exception)?;
            println!(
                "added {} to {}",
                exceptions::describe(&exception),
                path.display()
            );
        }
        ExceptionAction::List => {
            let found = match exceptions::read(&repo) {
                Ok(found) => found,
                Err(error) => exit_could_not_run(Failure::CouldNotRun(error)),
            };
            let today = exceptions::today();
            for exception in &found {
                let state = if exception.is_live(today) {
                    "live"
                } else {
                    "expired"
                };
                println!(
                    "{}  {}  {}  {state}  {}",
                    exception.rule,
                    exception.paths.join(","),
                    exception.expires,
                    exception.reason
                );
            }
        }
    }
    Ok(())
}

fn run_baseline_update() -> Result<()> {
    let inputs = Inputs::from_process()?;
    let (repo, current) = match gate::current_baseline(&inputs) {
        Ok(found) => found,
        Err(failure) => exit_could_not_run(failure),
    };
    let path = match baseline::write(&repo, &current) {
        Ok(path) => path,
        Err(error) => exit_could_not_run(Failure::CouldNotRun(error)),
    };
    println!(
        "wrote {} entries to {}",
        current.entries.len(),
        path.display()
    );
    Ok(())
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
