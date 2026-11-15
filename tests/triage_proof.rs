use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use oncebitten::config::Host;
use oncebitten::triage::host::{control_invocation, invocation, Captured, Invocation};
use oncebitten::triage::proof::{install, prove, requires_proof, sentinels, ProofError, Sentinel};
use oncebitten::triage::run::{RunError, Runner};
use tempfile::TempDir;

/// Stands in for the host: the first call is the hooks-on control, the second
/// is the hooks-off triage call. Each call creates the markers listed for it.
struct Fake {
    calls: Cell<usize>,
    control_fires: Vec<PathBuf>,
    triage_fires: Vec<PathBuf>,
}

impl Fake {
    fn new(control_fires: Vec<PathBuf>, triage_fires: Vec<PathBuf>) -> Self {
        Fake {
            calls: Cell::new(0),
            control_fires,
            triage_fires,
        }
    }
}

impl Runner for Fake {
    fn run(
        &self,
        _inv: &Invocation,
        _dir: &Path,
        _timeout: Duration,
    ) -> Result<Captured, RunError> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        let markers = if call == 0 {
            &self.control_fires
        } else {
            &self.triage_fires
        };
        for marker in markers {
            fs::write(marker, "").unwrap();
        }
        Ok(Captured {
            stdout: String::new(),
            answer: None,
        })
    }
}

struct Missing;

impl Runner for Missing {
    fn run(
        &self,
        _inv: &Invocation,
        _dir: &Path,
        _timeout: Duration,
    ) -> Result<Captured, RunError> {
        Err(RunError::Missing("claude".into()))
    }
}

fn dummy() -> Invocation {
    invocation(Host::Claude, "default", "p", Path::new(".")).unwrap()
}

fn claude_setup() -> (TempDir, TempDir, TempDir, Vec<Sentinel>) {
    let home = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    let markers = TempDir::new().unwrap();
    let list = sentinels(Host::Claude, home.path(), work.path(), markers.path());
    (home, work, markers, list)
}

fn run_prove(runner: &dyn Runner, work: &Path, list: &[Sentinel]) -> Result<(), ProofError> {
    prove(
        runner,
        &dummy(),
        &dummy(),
        work,
        Duration::from_secs(5),
        list,
    )
}

#[test]
fn claude_has_three_settings_scopes() {
    let (home, work, _markers, list) = claude_setup();
    let settings: Vec<PathBuf> = list.iter().map(|s| s.settings.clone()).collect();
    assert_eq!(
        settings,
        vec![
            home.path().join(".claude").join("settings.json"),
            work.path().join(".claude").join("settings.json"),
            work.path().join(".claude").join("settings.local.json"),
        ]
    );
}

#[test]
fn codex_has_user_and_project_hooks_files() {
    let home = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    let markers = TempDir::new().unwrap();
    let list = sentinels(Host::Codex, home.path(), work.path(), markers.path());
    let settings: Vec<PathBuf> = list.iter().map(|s| s.settings.clone()).collect();
    assert_eq!(
        settings,
        vec![
            home.path().join(".codex").join("hooks.json"),
            work.path().join(".codex").join("hooks.json"),
        ]
    );
}

#[test]
fn only_opencode_skips_the_proof() {
    assert!(requires_proof(Host::Claude));
    assert!(requires_proof(Host::Codex));
    assert!(!requires_proof(Host::Opencode));
}

#[test]
fn a_host_that_stays_quiet_when_hooks_off_passes() {
    let (_home, work, _markers, list) = claude_setup();
    let fake = Fake::new(list.iter().map(|s| s.marker.clone()).collect(), Vec::new());
    assert!(run_prove(&fake, work.path(), &list).is_ok());
}

#[test]
fn a_sentinel_that_fires_in_the_triage_call_refuses_the_host() {
    let (_home, work, _markers, list) = claude_setup();
    let fake = Fake::new(
        list.iter().map(|s| s.marker.clone()).collect(),
        vec![list[1].marker.clone()],
    );
    match run_prove(&fake, work.path(), &list) {
        Err(ProofError::Fired(scopes)) => assert_eq!(scopes, vec!["claude project settings"]),
        other => panic!("expected Fired, got {other:?}"),
    }
}

#[test]
fn a_sentinel_silent_with_hooks_on_makes_the_proof_inconclusive() {
    let (_home, work, _markers, list) = claude_setup();
    let fake = Fake::new(vec![list[0].marker.clone()], Vec::new());
    match run_prove(&fake, work.path(), &list) {
        Err(ProofError::Silent(scopes)) => assert_eq!(
            scopes,
            vec!["claude project settings", "claude local settings"]
        ),
        other => panic!("expected Silent, got {other:?}"),
    }
}

#[test]
fn a_missing_host_binary_is_a_run_error() {
    let (_home, work, _markers, list) = claude_setup();
    match run_prove(&Missing, work.path(), &list) {
        Err(ProofError::Run(RunError::Missing(_))) => {}
        other => panic!("expected Run(Missing), got {other:?}"),
    }
}

#[test]
fn install_writes_a_session_start_hook_that_names_its_marker() {
    let (_home, _work, _markers, list) = claude_setup();
    install(&list).unwrap();
    let text = fs::read_to_string(&list[0].settings).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    let command = value["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(command.contains(list[0].marker.to_string_lossy().as_ref()));
}

#[test]
fn control_runs_with_hooks_on_and_triage_with_hooks_off() {
    let work = Path::new("/tmp");
    let triage = invocation(Host::Claude, "default", "p", work).unwrap();
    let control = control_invocation(Host::Claude, "default", "p", work).unwrap();
    assert!(triage.args.contains(&"--setting-sources".to_string()));
    assert!(!control.args.contains(&"--setting-sources".to_string()));

    let triage = invocation(Host::Codex, "default", "p", work).unwrap();
    let control = control_invocation(Host::Codex, "default", "p", work).unwrap();
    assert!(triage.args.contains(&"hooks".to_string()));
    assert!(!control.args.contains(&"hooks".to_string()));
}
