use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::config;
use crate::host::{self, Host};
use crate::signal;
use crate::store::{self, Store};

const HISTORY_LIMIT: usize = 500;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const TRIAGE_ENV: &str = "ONCEBITTEN_TRIAGE";
/// Windows process creation flags that free the triage child from the hook's
/// console and Ctrl+C group.
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub fn run(host: &str, payload: &str, db: &Path, ts: i64) -> Result<Option<i64>> {
    run_with(host, payload, db, ts, signal::DEFAULT_CORRECTION_LEXICON)
}

pub fn run_with<P: AsRef<str>>(
    host: &str,
    payload: &str,
    db: &Path,
    ts: i64,
    lexicon: &[P],
) -> Result<Option<i64>> {
    record(host, payload, db, ts, lexicon, &[], None)
}

/// `run_with`, and first deletes events older than `ttl_days` before `ts`.
pub fn run_expiring<P: AsRef<str>>(
    host: &str,
    payload: &str,
    db: &Path,
    ts: i64,
    lexicon: &[P],
    test_commands: &[String],
    ttl_days: u32,
) -> Result<Option<i64>> {
    record(
        host,
        payload,
        db,
        ts,
        lexicon,
        test_commands,
        Some(ttl_days),
    )
}

fn record<P: AsRef<str>>(
    host: &str,
    payload: &str,
    db: &Path,
    ts: i64,
    lexicon: &[P],
    test_commands: &[String],
    ttl_days: Option<u32>,
) -> Result<Option<i64>> {
    let host = Host::parse(host)?;
    let Some(event) = host::normalize(host, payload, ts)? else {
        return Ok(None);
    };
    let store = Store::open(db)?;
    if let Some(days) = ttl_days {
        store.expire(ts - i64::from(days) * DAY_MS)?;
    }
    let history = store.session_history(&event.host, &event.session_id, HISTORY_LIMIT)?;
    let signals = signal::derive_configured(&event, &history, lexicon, test_commands);
    let capture = store::Capture {
        git_head: event.repo.as_deref().and_then(git_head),
        tool_use_id: tool_use_id(payload),
    };
    store.record_captured(&event, &signals, &capture)
}

fn tool_use_id(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    value["tool_use_id"].as_str().map(str::to_string)
}

/// The commit `HEAD` names in `repo`, from `git rev-parse`. `None` when git is
/// missing or the repo has no commit yet. Capture never fails on it.
fn git_head(repo: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--verify", "-q", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8(output.stdout).ok()?;
    let head = head.trim();
    (!head.is_empty()).then(|| head.to_string())
}

pub fn run_from_process(host: &str) -> Result<()> {
    if std::env::var(TRIAGE_ENV).as_deref() == Ok("1") {
        // Drain stdin so the host never sees a broken pipe from this hook.
        let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
        return Ok(());
    }
    let mut payload = String::new();
    std::io::stdin()
        .read_to_string(&mut payload)
        .context("cannot read hook payload from stdin")?;
    let db = store::events_db_path()?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the unix epoch")?
        .as_millis() as i64;
    let loaded = config::load(&config::Inputs::from_process()?)?;
    let lexicon: Vec<String> = loaded
        .config
        .capture
        .correction_phrases
        .into_iter()
        .chain(loaded.config.capture.extra_correction_phrases)
        .collect();
    let test_commands = loaded.config.capture.test_commands;
    let ttl_days = loaded.config.evidence.ttl_days;
    let recorded = run_expiring(host, &payload, &db, ts, &lexicon, &test_commands, ttl_days)?;
    if let (Some(event_id), true) = (recorded, loaded.config.triage.background) {
        if let Err(error) = start_background_triage(&db, event_id, &loaded.config.triage) {
            eprintln!("oncebitten capture: background triage not started: {error:#}");
        }
    }
    Ok(())
}

/// Starts one detached `oncebitten triage --queue` per cluster this event
/// brought to its threshold. Capture never waits for it.
fn start_background_triage(db: &Path, event_id: i64, triage: &config::Triage) -> Result<()> {
    let store = Store::open(db)?;
    let due = crate::proposals::due(&store, event_id)?;
    if due.is_empty() {
        return Ok(());
    }
    let exe = crate::hooks::current_exe()?;
    for item in due {
        let mut command = Command::new(&exe);
        command.args(["triage", &item.cluster, "--queue"]);
        let host = config::Host::parse(&item.host)
            .filter(|host| crate::triage::flow::target(triage, Some(*host)).is_ok());
        if let Some(host) = host {
            command.args(["--host", host.as_str()]);
        }
        command
            .current_dir(&item.repo)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detach(&mut command);
        command
            .spawn()
            .with_context(|| format!("spawning triage for {}", item.cluster))?;
    }
    Ok(())
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}
