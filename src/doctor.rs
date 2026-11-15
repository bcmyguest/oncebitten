//! `oncebitten doctor`: read-only checks of an installation and a repository,
//! one line each, with a fix for every warning and failure.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use crate::config::{self, Inputs, Platform};
use crate::engine::native::{find_on_path, spawn_path};
use crate::exceptions;
use crate::gate;
use crate::hooks::{self, Scope};
use crate::host::{Host, ALL_HOSTS};
use crate::lockfile;
use crate::proof::{self, Validity};
use crate::rule::{Mode, Status};
use crate::rules::{self, Failure};
use crate::store::{self, Probe};

/// The first release with the `pre-push` stage name the gate hook uses.
pub const MIN_PRE_COMMIT: (u64, u64, u64) = (3, 2, 0);
const PRE_COMMIT: &str = "pre-commit";
const PRE_COMMIT_CONFIG: &str = ".pre-commit-config.yaml";
const GATE_HOOK_IDS: [&str; 2] = ["oncebitten-gate", "oncebitten-gate-system"];
const OPENSHELL: &str = "openshell";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Fail => "fail",
        }
    }
}

/// One check's verdict. `fix` is set for every warning and failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub level: Level,
    pub check: String,
    pub message: String,
    pub fix: Option<String>,
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<4}  {}: {}",
            self.level.as_str(),
            self.check,
            self.message
        )?;
        if let Some(fix) = &self.fix {
            write!(f, " (fix: {fix})")?;
        }
        Ok(())
    }
}

fn ok(check: impl Into<String>, message: impl Into<String>) -> Line {
    Line {
        level: Level::Ok,
        check: check.into(),
        message: message.into(),
        fix: None,
    }
}

fn warn(check: impl Into<String>, message: impl Into<String>, fix: impl Into<String>) -> Line {
    Line {
        level: Level::Warn,
        check: check.into(),
        message: message.into(),
        fix: Some(fix.into()),
    }
}

fn fail(check: impl Into<String>, message: impl Into<String>, fix: impl Into<String>) -> Line {
    Line {
        level: Level::Fail,
        check: check.into(),
        message: message.into(),
        fix: Some(fix.into()),
    }
}

/// Exit 1 when any check failed, else 0.
pub fn exit_code(lines: &[Line]) -> i32 {
    i32::from(lines.iter().any(|line| line.level == Level::Fail))
}

/// Runs every check. Reads files and runs `pre-commit --version` and
/// `openshell status`; writes nothing.
pub fn run(inputs: &Inputs, gateway: Option<&str>) -> Vec<Line> {
    let mut lines = Vec::new();
    let config_loads = match config::load(inputs) {
        Ok(loaded) => {
            lines.push(ok("config", "loads"));
            for warning in &loaded.warnings {
                lines.push(warn("config", warning, "see oncebitten config path"));
            }
            true
        }
        Err(error) => {
            lines.push(fail(
                "config",
                format!("{error:#}"),
                "correct the file named above; oncebitten config path lists them",
            ));
            false
        }
    };
    let repo = config::repo_root(&inputs.cwd);
    lines.push(pre_commit());
    match &repo {
        Some(repo) => {
            lines.push(pre_commit_config(repo));
            if config_loads {
                lines.push(lockfile_matches(inputs, repo));
                lines.extend(rules_and_proofs(inputs));
            }
            lines.extend(exception_lines(repo));
        }
        None => lines.push(fail(
            "repository",
            "not inside a git repository",
            "run oncebitten doctor from a git checkout",
        )),
    }
    let project = repo.as_deref().unwrap_or(&inputs.cwd);
    lines.extend(capture_hooks(inputs, project));
    lines.push(evidence_store(inputs));
    lines.push(openshell(gateway));
    lines
}

fn pre_commit() -> Line {
    let fix = "install pre-commit 3.2.0 or later, e.g. pipx install pre-commit";
    let output = match probe(PRE_COMMIT, &["--version"]) {
        Ok(output) => output,
        Err(error) => return fail(PRE_COMMIT, format!("{error:#}"), fix),
    };
    let Some(version) = parse_version(&output) else {
        return fail(
            PRE_COMMIT,
            format!("cannot read a version in `{}`", output.trim()),
            fix,
        );
    };
    let (major, minor, patch) = version;
    if version < MIN_PRE_COMMIT {
        return fail(
            PRE_COMMIT,
            format!("version {major}.{minor}.{patch} lacks the pre-push stage name"),
            fix,
        );
    }
    ok(PRE_COMMIT, format!("version {major}.{minor}.{patch}"))
}

/// The first `X.Y.Z` in `text`, such as `pre-commit 4.6.0`.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    text.split_whitespace().find_map(|word| {
        let mut parts = word.split('.').map(|part| part.parse::<u64>().ok());
        match (parts.next(), parts.next(), parts.next()) {
            (Some(Some(major)), Some(Some(minor)), Some(Some(patch))) => {
                Some((major, minor, patch))
            }
            _ => None,
        }
    })
}

fn pre_commit_config(repo: &Path) -> Line {
    let check = "pre-push hook";
    let fix = "add the oncebitten-gate hook to .pre-commit-config.yaml (docs/gate.md), \
               then run pre-commit install --hook-type pre-push";
    let path = repo.join(PRE_COMMIT_CONFIG);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return fail(check, format!("no {PRE_COMMIT_CONFIG}"), fix);
    };
    match gate_hook_id(&text) {
        Some(id) => ok(check, format!("{id} in {PRE_COMMIT_CONFIG}")),
        None => fail(
            check,
            format!("{PRE_COMMIT_CONFIG} lists neither oncebitten-gate nor oncebitten-gate-system"),
            fix,
        ),
    }
}

/// The oncebitten gate hook id a `.pre-commit-config.yaml` lists, read as
/// text: the file is YAML, and this needs one `id:` line, not a YAML parser.
pub fn gate_hook_id(text: &str) -> Option<&'static str> {
    text.lines().find_map(|line| {
        let line = line.split('#').next().unwrap_or_default().trim();
        let value = line
            .trim_start_matches('-')
            .trim_start()
            .strip_prefix("id:")?;
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        GATE_HOOK_IDS.into_iter().find(|id| *id == value)
    })
}

fn lockfile_matches(inputs: &Inputs, repo: &Path) -> Line {
    let check = "rules.lock";
    let sync = "run oncebitten rules sync and commit .oncebitten/rules.lock";
    let path = lockfile::path(repo);
    if !path.exists() {
        return warn(check, "missing; gate pr exits 2 without it", sync);
    }
    let result = config::load(inputs).and_then(|loaded| {
        let lock = lockfile::read(repo)?;
        rules::pair_locked(&loaded.config.rules.sources, &lock).map(|pinned| pinned.len())
    });
    match result {
        Ok(count) => ok(check, format!("pins all {count} configured source(s)")),
        Err(error) => fail(check, format!("{error:#}"), sync),
    }
}

fn rules_and_proofs(inputs: &Inputs) -> Vec<Line> {
    let resolved = match rules::load(inputs, false) {
        Ok(resolved) => resolved,
        Err(Failure::Invalid(errors)) => {
            return errors
                .iter()
                .map(|error| {
                    fail(
                        "rules",
                        format!("{error:#}"),
                        "correct the rule; oncebitten rules validate lists every error",
                    )
                })
                .collect();
        }
        Err(Failure::CouldNotRun(error)) => {
            return vec![fail(
                "rules",
                format!("{error:#}"),
                "run oncebitten rules sync",
            )]
        }
    };
    let active: Vec<_> = resolved
        .iter()
        .filter(|entry| entry.rule.status == Status::Active)
        .collect();
    let mut lines = vec![ok(
        "rules",
        format!(
            "{} rule(s) validate, {} active",
            resolved.len(),
            active.len()
        ),
    )];
    for entry in active {
        if !gate::needs_proof(&entry.rule.check) {
            continue;
        }
        let check = format!("proof {}", entry.id);
        let fix = format!("run oncebitten replay {} and commit proof.json", entry.id);
        match proof::validity(&entry.rule) {
            Validity::Valid => lines.push(ok(check, "valid")),
            invalid if entry.rule.mode == Mode::Block => {
                lines.push(fail(check, gate::describe(&entry.id, &invalid), fix))
            }
            invalid => lines.push(warn(check, gate::describe(&entry.id, &invalid), fix)),
        }
    }
    lines
}

fn exception_lines(repo: &Path) -> Vec<Line> {
    let check = "exceptions";
    let found = match exceptions::read(repo) {
        Ok(found) => found,
        Err(error) => {
            return vec![fail(
                check,
                format!("{error:#}"),
                "correct .oncebitten/exceptions.toml; see docs/lifecycle.md",
            )]
        }
    };
    let today = exceptions::today();
    let expired: Vec<_> = found.iter().filter(|e| !e.is_live(today)).collect();
    if expired.is_empty() {
        return vec![ok(check, format!("{} live", found.len()))];
    }
    expired
        .into_iter()
        .map(|exception| {
            warn(
                check,
                format!("expired: {}", exceptions::describe(exception)),
                "fix the violations and delete the entry, or set a later expires",
            )
        })
        .collect()
}

fn capture_hooks(inputs: &Inputs, project: &Path) -> Vec<Line> {
    let Some(home) = config::home_dir(&inputs.vars, Platform::current()) else {
        return vec![warn(
            "capture",
            "no absolute home directory in HOME, USERPROFILE or HOMEDRIVE+HOMEPATH",
            "set HOME",
        )];
    };
    ALL_HOSTS
        .into_iter()
        .map(|host| capture_hook(inputs, host, &home, project))
        .collect()
}

fn capture_hook(inputs: &Inputs, host: Host, home: &Path, project: &Path) -> Line {
    let check = format!("capture {}", host.name());
    let (program, dir) = host_markers(host, home);
    if find_on_path(program).is_none() && !dir.is_dir() {
        return ok(check, "not installed");
    }
    let fix = format!(
        "oncebitten hooks install --host {} --scope user",
        host.name()
    );
    let targets = [Scope::User, Scope::Project].map(|scope| {
        hooks::install_target(host, scope, home, project, &inputs.vars)
            .and_then(|path| hooks::is_installed(host, &path).map(|found| (found, path)))
    });
    let mut errors = Vec::new();
    for target in targets {
        match target {
            Ok((true, path)) => return ok(check, format!("hook in {}", path.display())),
            Ok((false, _)) => {}
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    match errors.is_empty() {
        true => warn(check, "host found, capture hook not installed", fix),
        false => warn(check, errors.join("; "), fix),
    }
}

/// A host's executable and user directory; either one marks it installed.
fn host_markers(host: Host, home: &Path) -> (&'static str, PathBuf) {
    match host {
        Host::Claude => ("claude", home.join(".claude")),
        Host::Codex => ("codex", home.join(".codex")),
        Host::Cursor => ("cursor-agent", home.join(".cursor")),
        Host::Copilot => ("copilot", home.join(".copilot")),
        Host::OpenCode => ("opencode", home.join(".config").join("opencode")),
        Host::Pi => ("pi", home.join(".pi")),
    }
}

fn evidence_store(inputs: &Inputs) -> Line {
    let check = "evidence store";
    let path = match store::events_db_path_for(&inputs.vars, Platform::current()) {
        Ok(path) => path,
        Err(error) => return fail(check, format!("{error:#}"), "set XDG_DATA_HOME or HOME"),
    };
    let shown = path.display();
    match store::probe(&path) {
        Ok(Probe::Current) => ok(check, format!("opens: {shown}")),
        Ok(Probe::Missing) => ok(check, format!("empty; the first capture creates {shown}")),
        Ok(Probe::Older(version)) => ok(
            check,
            format!("schema {version} at {shown}; the next capture upgrades it"),
        ),
        Ok(Probe::Newer(version)) => fail(
            check,
            format!("schema {version} at {shown} comes from a newer oncebitten"),
            "upgrade oncebitten",
        ),
        Err(error) => fail(
            check,
            format!("{error:#}"),
            format!("move {shown} aside; capture starts a new store"),
        ),
    }
}

fn openshell(gateway: Option<&str>) -> Line {
    let check = "openshell";
    let install = if cfg!(windows) {
        "OpenShell runs on Windows only inside WSL 2: install WSL 2, then OpenShell \
         and oncebitten inside it (docs/rules.md#replay)"
    } else {
        "install the OpenShell CLI (docs/rules.md#replay); replay --local works without it"
    };
    if find_on_path(OPENSHELL).is_none() {
        return warn(check, "not on PATH; replay needs it", install);
    }
    let mut args = Vec::new();
    if let Some(endpoint) = gateway {
        args.extend(["--gateway-endpoint", endpoint]);
    }
    args.push("status");
    match probe(OPENSHELL, &args) {
        Ok(_) => ok(check, "gateway reachable"),
        Err(error) => warn(
            check,
            format!("gateway unreachable: {error:#}"),
            "run openshell doctor to diagnose the gateway, or pass --gateway <endpoint>",
        ),
    }
}

/// Runs `program args` with a deadline and returns its stdout. A missing
/// program, a non-zero exit or a timeout is an error.
fn probe(program: &str, args: &[&str]) -> Result<String> {
    let mut child = Command::new(spawn_path(program))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| anyhow!("{program}: {error}"))?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "{program} gave no answer in {}s",
                PROBE_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(POLL);
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        pipe.read_to_string(&mut stdout)?;
    }
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr)?;
    }
    if !status.success() {
        let detail = stderr
            .lines()
            .chain(stdout.lines())
            .find(|l| !l.trim().is_empty());
        return Err(anyhow!(
            "{program} {} exited with {status}{}",
            args.join(" "),
            detail
                .map(|d| format!(": {}", d.trim()))
                .unwrap_or_default()
        ));
    }
    Ok(stdout)
}
