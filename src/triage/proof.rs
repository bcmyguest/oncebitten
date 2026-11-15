use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::config::Host;
use crate::triage::host::{Captured, Invocation};
use crate::triage::run::{RunError, Runner};

/// One sentinel hook: a settings file the host reads hooks from, and the
/// marker file its hook creates when it fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentinel {
    pub scope: &'static str,
    pub settings: PathBuf,
    pub marker: PathBuf,
}

/// Why a host failed the proof. Each variant is a refusal (exit 2).
#[derive(Debug)]
pub enum ProofError {
    /// The triage call fired these sentinels: hooks were not off.
    Fired(Vec<&'static str>),
    /// With hooks on, these sentinels stayed silent, so the proof proves nothing.
    Silent(Vec<&'static str>),
    Run(RunError),
    Io(io::Error),
}

impl fmt::Display for ProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProofError::Fired(scopes) => write!(
                f,
                "hooks-off proof failed: sentinel hooks fired in the triage call ({})",
                scopes.join(", ")
            ),
            ProofError::Silent(scopes) => write!(
                f,
                "hooks-off proof inconclusive: sentinels did not fire with hooks on ({})",
                scopes.join(", ")
            ),
            ProofError::Run(err) => write!(f, "hooks-off proof could not run: {err}"),
            ProofError::Io(err) => write!(f, "hooks-off proof could not set up: {err}"),
        }
    }
}

impl std::error::Error for ProofError {}

/// Opencode is accepted without a proof (settled in PLAN.md).
pub fn requires_proof(host: Host) -> bool {
    !matches!(host, Host::Opencode)
}

/// Every settings scope the host reads hooks from. `home` is `None` when the
/// user scope cannot be sandboxed (the real HOME), so only workdir scopes count.
fn scopes(host: Host, home: Option<&Path>, workdir: &Path) -> Vec<(&'static str, PathBuf)> {
    let mut scopes = Vec::new();
    match host {
        Host::Claude => {
            if let Some(home) = home {
                scopes.push((
                    "claude user settings",
                    home.join(".claude").join("settings.json"),
                ));
            }
            scopes.push((
                "claude project settings",
                workdir.join(".claude").join("settings.json"),
            ));
            scopes.push((
                "claude local settings",
                workdir.join(".claude").join("settings.local.json"),
            ));
        }
        Host::Codex => {
            if let Some(home) = home {
                scopes.push(("codex user hooks", home.join(".codex").join("hooks.json")));
            }
            scopes.push((
                "codex project hooks",
                workdir.join(".codex").join("hooks.json"),
            ));
        }
        _ => {}
    }
    scopes
}

/// Every settings scope the host reads hooks from, under `home` and `workdir`.
/// Markers go in `markers`, one file per scope.
pub fn sentinels(host: Host, home: &Path, workdir: &Path, markers: &Path) -> Vec<Sentinel> {
    with_markers(scopes(host, Some(home), workdir), markers)
}

/// The workdir scopes only. Used with the real HOME, which the proof must not write.
pub fn workdir_sentinels(host: Host, workdir: &Path, markers: &Path) -> Vec<Sentinel> {
    with_markers(scopes(host, None, workdir), markers)
}

fn with_markers(scopes: Vec<(&'static str, PathBuf)>, markers: &Path) -> Vec<Sentinel> {
    scopes
        .into_iter()
        .enumerate()
        .map(|(index, (scope, settings))| Sentinel {
            scope,
            settings,
            marker: markers.join(format!("marker-{index}")),
        })
        .collect()
}

/// Writes each sentinel's settings file: one hook per session start and prompt
/// that creates the scope's marker.
pub fn install(sentinels: &[Sentinel]) -> io::Result<()> {
    for sentinel in sentinels {
        if let Some(parent) = sentinel.marker.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Some(parent) = sentinel.settings.parent() {
            fs::create_dir_all(parent)?;
        }
        let command = sentinel_command(&sentinel.marker);
        let entry = json!([{"hooks": [{"type": "command", "command": command}]}]);
        let body = json!({"hooks": {"SessionStart": entry, "UserPromptSubmit": entry}});
        fs::write(&sentinel.settings, body.to_string())?;
    }
    Ok(())
}

fn sentinel_command(marker: &Path) -> String {
    if cfg!(windows) {
        format!("cmd /c type nul > \"{}\"", marker.display())
    } else {
        format!("touch '{}'", marker.display())
    }
}

fn fired(sentinels: &[Sentinel]) -> Vec<&'static str> {
    sentinels
        .iter()
        .filter(|s| s.marker.exists())
        .map(|s| s.scope)
        .collect()
}

fn clear(sentinels: &[Sentinel]) -> io::Result<()> {
    for sentinel in sentinels {
        match fs::remove_file(&sentinel.marker) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Runs the hooks-on control, then the hooks-off triage call, in `workdir`.
/// Every sentinel must fire in the control and none may fire in the triage
/// call. Returns the triage call's output on success. Installs the sentinels
/// first, so the sentinels must live under temp dirs.
pub fn prove(
    runner: &dyn Runner,
    control: &Invocation,
    triage: &Invocation,
    workdir: &Path,
    timeout: Duration,
    sentinels: &[Sentinel],
) -> Result<Captured, ProofError> {
    install(sentinels).map_err(ProofError::Io)?;
    clear(sentinels).map_err(ProofError::Io)?;
    runner
        .run(control, workdir, timeout)
        .map_err(ProofError::Run)?;
    let live = fired(sentinels);
    let silent: Vec<&'static str> = sentinels
        .iter()
        .map(|s| s.scope)
        .filter(|scope| !live.contains(scope))
        .collect();
    if !silent.is_empty() {
        return Err(ProofError::Silent(silent));
    }
    clear(sentinels).map_err(ProofError::Io)?;
    let captured = runner
        .run(triage, workdir, timeout)
        .map_err(ProofError::Run)?;
    let leaked = fired(sentinels);
    if !leaked.is_empty() {
        return Err(ProofError::Fired(leaked));
    }
    Ok(captured)
}
