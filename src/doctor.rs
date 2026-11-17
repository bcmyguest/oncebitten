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
use crate::proof::Validity;
use crate::rule::{Check, Status};
use crate::rules::{self, Failure};
use crate::signal::ALL_SIGNALS;
use crate::store::{self, Probe};

/// The first release with the `pre-push` stage name the gate hook uses.
pub const MIN_PRE_COMMIT: (u64, u64, u64) = (3, 2, 0);
const PRE_COMMIT: &str = "pre-commit";
const PRE_COMMIT_CONFIG: &str = ".pre-commit-config.yaml";
/// The first prek release with the `pre-push` hook type (prek CHANGELOG, 0.1.5).
pub const MIN_PREK: (u64, u64, u64) = (0, 1, 5);
const PREK: &str = "prek";
const UVX: &str = "uvx";
const PRE_COMMIT_FIX: &str = "install pre-commit 3.2.0 or later (pip install pre-commit; \
     https://pre-commit.com/#install), or prek (uv tool install prek), \
     or run it through uv with uvx prek install --hook-type pre-push \
     (uv: https://docs.astral.sh/uv/getting-started/installation/)";
const GATE_HOOK_IDS: [&str; 3] = [
    "oncebitten-gate",
    "oncebitten-gate-system",
    "oncebitten-gate-python",
];
/// Tokens that precede the program name in `uvx oncebitten` or `cargo run --bin oncebitten`.
const LAUNCHERS: [&str; 4] = ["uvx", "npx", "run", "--bin"];
const OPENSHELL: &str = "openshell";
const OPENSHELL_INSTALL: &str =
    "curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh \
     (https://docs.nvidia.com/openshell/latest/about/installation)";
/// Printed when a rule that replays in OpenShell is drafted without the CLI.
pub const OPENSHELL_MISSING: &str = "openshell is not on PATH: this rule stays unproven until \
     oncebitten replay runs it in OpenShell; see oncebitten doctor";
const DOCKER: &str = "docker";
/// Install commands for programs oncebitten or its rules run, from each
/// tool's official install page.
const INSTALLERS: &[(&str, &str)] = &[
    (
        "cargo",
        "install Rust with curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh \
         (https://rustup.rs)",
    ),
    (
        "ruff",
        "uv tool install ruff@latest (https://docs.astral.sh/ruff/installation/)",
    ),
    (
        DOCKER,
        "install Docker: https://docs.docker.com/get-started/get-docker/",
    ),
    (
        "claude",
        "curl -fsSL https://claude.ai/install.sh | bash (https://code.claude.com/docs/en/setup)",
    ),
    (
        "codex",
        "npm install -g @openai/codex (https://github.com/openai/codex)",
    ),
    (
        "opencode",
        "curl -fsSL https://opencode.ai/v2/install | bash (https://opencode.ai/docs/)",
    ),
    (
        "pi",
        "curl -fsSL https://pi.dev/install.sh | sh \
         (https://github.com/badlogic/pi-mono/tree/main/packages/coding-agent)",
    ),
];
const NO_GATEWAY: &str = "No gateway configured";
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

/// Runs every check. Reads files and runs `pre-commit --version`,
/// `prek --version` and `openshell status`; writes and installs nothing.
pub fn run(inputs: &Inputs, gateway: Option<&str>) -> Vec<Line> {
    let mut lines = Vec::new();
    let loaded = match config::load(inputs) {
        Ok(loaded) => {
            lines.push(ok("config", "loads"));
            for warning in &loaded.warnings {
                lines.push(warn("config", warning, "see oncebitten config path"));
            }
            Some(loaded)
        }
        Err(error) => {
            lines.push(fail(
                "config",
                format!("{error:#}"),
                "correct the file named above; oncebitten config path lists them",
            ));
            None
        }
    };
    let config_loads = loaded.is_some();
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
    if let Some(loaded) = &loaded {
        lines.extend(triage_hosts(loaded));
    }
    lines
}

/// The hook runner: pre-commit, else prek, else uvx, which runs either.
fn pre_commit() -> Line {
    let pre_commit = match runner_version(PRE_COMMIT, MIN_PRE_COMMIT, "the pre-push stage name") {
        Ok(version) => return ok(PRE_COMMIT, format!("version {version}")),
        Err(problem) => problem,
    };
    let prek = match runner_version(PREK, MIN_PREK, "the pre-push hook type") {
        Ok(version) => return ok(PRE_COMMIT, format!("prek version {version}")),
        Err(problem) => problem,
    };
    if find_on_path(UVX).is_some() {
        return ok(
            PRE_COMMIT,
            "uvx on PATH; the hook runs as uvx prek or uvx pre-commit",
        );
    }
    fail(
        PRE_COMMIT,
        format!("{pre_commit}; {}; uvx not on PATH", named(PREK, &prek)),
        PRE_COMMIT_FIX,
    )
}

/// `problem` led by `program`: probe errors name it already, version errors do not.
fn named(program: &str, problem: &str) -> String {
    match problem.starts_with(program) {
        true => problem.to_owned(),
        false => format!("{program} {problem}"),
    }
}

/// `program --version` as `X.Y.Z` when at least `min`, else what is wrong.
fn runner_version(program: &str, min: (u64, u64, u64), lacks: &str) -> Result<String, String> {
    let output = probe(program, &["--version"]).map_err(|error| format!("{error:#}"))?;
    let Some(version) = parse_version(&output) else {
        return Err(format!("cannot read a version in `{}`", output.trim()));
    };
    let (major, minor, patch) = version;
    let shown = format!("{major}.{minor}.{patch}");
    if version < min {
        return Err(format!("version {shown} lacks {lacks}"));
    }
    Ok(shown)
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
               then run pre-commit install --hook-type pre-push \
               (with prek: prek install --hook-type pre-push)";
    let path = repo.join(PRE_COMMIT_CONFIG);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return fail(check, format!("no {PRE_COMMIT_CONFIG}"), fix);
    };
    match gate_hook(&text) {
        Some(id) => ok(check, format!("{id} in {PRE_COMMIT_CONFIG}")),
        None => fail(
            check,
            format!(
                "{PRE_COMMIT_CONFIG} lists none of oncebitten-gate, oncebitten-gate-system \
                 or oncebitten-gate-python, and no hook whose entry runs oncebitten gate"
            ),
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

/// The id of the hook that runs the gate: a published id, or a hook whose
/// `entry:` runs `oncebitten gate` (a local hook, `cargo run -- gate`, `uvx`).
/// Read as text, as `gate_hook_id` is: no YAML crate is a dependency.
pub fn gate_hook(text: &str) -> Option<String> {
    if let Some(id) = gate_hook_id(text) {
        return Some(id.to_string());
    }
    let mut current = "local hook".to_string();
    for line in text.lines() {
        let line = line.split(" #").next().unwrap_or_default().trim();
        let line = line.trim_start_matches('-').trim_start();
        if let Some(id) = line.strip_prefix("id:") {
            current = unquote(id).to_string();
        } else if let Some(entry) = line.strip_prefix("entry:") {
            if entry_runs_gate(unquote(entry)) {
                return Some(current);
            }
        }
    }
    None
}

fn unquote(value: &str) -> &str {
    value.trim().trim_matches(|c| c == '"' || c == '\'')
}

/// True when the command runs the `oncebitten` program (first token, or after a
/// launcher such as `uvx` or `--bin`) and its next argument, past any `--`, is `gate`.
fn entry_runs_gate(entry: &str) -> bool {
    let tokens: Vec<&str> = entry.split_whitespace().collect();
    tokens.iter().enumerate().any(|(i, token)| {
        let name = token.rsplit(['/', '\\']).next().unwrap_or_default();
        let name = name.strip_suffix(".exe").unwrap_or(name);
        let launched = i == 0 || LAUNCHERS.contains(&tokens[i - 1]);
        name == "oncebitten"
            && launched
            && tokens[i + 1..]
                .iter()
                .find(|next| **next != "--")
                .is_some_and(|next| *next == "gate")
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
    let mut programs: Vec<(&str, &str)> = Vec::new();
    let mut replay_needed = false;
    for entry in active {
        if let Check::Native { command } = &entry.rule.check {
            if let Some(program) = command.first() {
                if !programs.iter().any(|(seen, _)| seen == program) {
                    programs.push((program, &entry.id));
                }
            }
        }
        if !gate::needs_proof(&entry.rule.check) {
            continue;
        }
        let check = format!("proof {}", entry.id);
        let fix = format!("run oncebitten replay {} and commit proof.json", entry.id);
        let validity = gate::proof_validity(&entry.rule);
        replay_needed |= validity != Validity::Valid;
        match validity {
            Validity::Valid => lines.push(ok(check, "valid")),
            invalid if gate::proved(std::slice::from_ref(entry)).len() == 1 => {
                lines.push(fail(check, gate::describe(&entry.id, &invalid), fix))
            }
            invalid => lines.push(warn(check, gate::describe(&entry.id, &invalid), fix)),
        }
    }
    for (program, id) in programs {
        // A path such as ./scripts/lint lives in the repository, not on PATH.
        if program.contains(['/', '\\']) {
            continue;
        }
        lines.push(tool(program, &format!("rule {id} runs it")));
    }
    if replay_needed {
        lines.push(tool(DOCKER, "replay builds its sandbox image with it"));
    }
    lines
}

/// `ok` when `program` is on PATH, else `warn` with its install command.
/// Doctor only reports; it never installs.
fn tool(program: &str, why: &str) -> Line {
    if find_on_path(program).is_some() {
        return ok(program, format!("on PATH; {why}"));
    }
    warn(
        program,
        format!("not on PATH; {why}"),
        install_hint(program),
    )
}

fn install_hint(program: &str) -> String {
    INSTALLERS
        .iter()
        .find(|(name, _)| *name == program)
        .map_or_else(
            || format!("install {program} and put it on PATH"),
            |(_, hint)| (*hint).to_owned(),
        )
}

/// One line per triage host the configuration names; the built-in default
/// target counts only when a layer sets it.
fn triage_hosts(loaded: &config::Loaded) -> Vec<Line> {
    let triage = &loaded.config.triage;
    let explicit_default = loaded
        .origins
        .get("triage.default")
        .is_some_and(|origin| *origin != config::Origin::Default);
    let mut hosts = Vec::new();
    if explicit_default {
        hosts.push(triage.default.host);
    }
    for target in [&triage.claude, &triage.codex, &triage.pi, &triage.opencode]
        .into_iter()
        .flatten()
    {
        if !hosts.contains(&target.host) {
            hosts.push(target.host);
        }
    }
    hosts
        .into_iter()
        .map(|host| {
            let program = host.as_str();
            let check = format!("triage {program}");
            if find_on_path(program).is_some() {
                return ok(check, format!("{program} on PATH"));
            }
            warn(
                check,
                format!("{program} not on PATH; triage runs it"),
                install_hint(program),
            )
        })
        .collect()
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
    let mut lines = Vec::new();
    for host in ALL_HOSTS {
        lines.push(capture_hook(inputs, host, &home, project));
        if host_installed(host, &home) {
            lines.push(host_signals(host));
        }
    }
    lines
}

fn host_installed(host: Host, home: &Path) -> bool {
    let (program, dir) = host_markers(host, home);
    find_on_path(program).is_some() || dir.is_dir()
}

/// The signals a host can produce, from `Host::capabilities`; the rest need
/// events the host never reports.
fn host_signals(host: Host) -> Line {
    let can = host.capabilities().signals();
    let cannot: Vec<&str> = ALL_SIGNALS
        .into_iter()
        .filter(|signal| !can.contains(signal))
        .collect();
    let mut message = format!("produces {}", can.join(", "));
    if !cannot.is_empty() {
        message.push_str(&format!("; cannot produce {}", cannot.join(", ")));
    }
    ok(format!("signals {}", host.name()), message)
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

/// The stderr line for a draft that needs OpenShell: a PATH lookup, no process.
pub fn openshell_notice(needs_sandbox: bool, on_path: bool) -> Option<&'static str> {
    (needs_sandbox && !on_path).then_some(OPENSHELL_MISSING)
}

/// Whether the OpenShell CLI is on `PATH`.
pub fn openshell_on_path() -> bool {
    find_on_path(OPENSHELL).is_some()
}

fn openshell(gateway: Option<&str>) -> Line {
    let check = "openshell";
    let install = if cfg!(windows) {
        format!(
            "OpenShell runs on Windows only inside WSL 2: install WSL 2, then OpenShell \
             ({OPENSHELL_INSTALL}) and oncebitten inside it (docs/rules.md#replay)"
        )
    } else {
        format!(
            "install the OpenShell CLI: {OPENSHELL_INSTALL}; see docs/rules.md#replay; \
             replay --local works without it"
        )
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
        // `openshell status` exits 0 when no gateway is configured.
        Ok(out) if out.contains(NO_GATEWAY) => warn(
            check,
            "no gateway configured; replay fails with No active gateway",
            "register one with openshell gateway add <endpoint>, or pass --gateway <endpoint>",
        ),
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
