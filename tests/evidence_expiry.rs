use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use oncebitten::config::{self, Inputs};
use oncebitten::evidence::{self, Cluster};
use oncebitten::proof::Replayed;
use oncebitten::proposals;
use oncebitten::replay::Verdict;
use oncebitten::rule::Rule;
use oncebitten::signal;
use oncebitten::store::{NewEvent, Store};
use oncebitten::triage::verdict::{self as triage_verdict, Answer};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_oncebitten");
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const ANSWER: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"docs drift","proposed_fix_kind":"lint","generality":"project","draft":{"id":"docs-with-src","description":"a change under src needs a docs change","mode":"block","engine":"diff","when_changed":["src/**"],"require_changed":["docs/**"],"bad":{"name":"bad.diff","content":"src/lib.rs\n"},"fixed":{"name":"fixed.diff","content":"src/lib.rs\ndocs/a.md\n"},"corpus":[{"name":"readme.diff","content":"README.md\n"}],"rationale":"agents forgot docs twice"}}"#;

#[test]
fn expire_deletes_old_events_and_their_signals_only() {
    let store = Store::open_in_memory().unwrap();
    fail(&store, "s1", "cargo doc", 100);
    fail(&store, "s2", "cargo doc", 200);

    assert_eq!(store.expire(150).unwrap(), 1);

    assert_eq!(event_count(&store, "s1"), 0);
    assert_eq!(event_count(&store, "s2"), 1);
    let rows = store.signals().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event.ts, 200);
    assert_eq!(store.expire(150).unwrap(), 0);
}

#[test]
fn expire_keeps_the_events_of_a_waiting_proposal() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer(),
        &no_sandbox,
        10,
    )
    .unwrap();
    fail(&store, "s3", "npm run lint", 3);

    assert_eq!(store.expire(1000).unwrap(), 1);

    assert_eq!(event_count(&store, "s3"), 0);
    let clusters = evidence::load(&store).unwrap();
    assert_eq!(
        evidence::find(&clusters, &cluster.id)
            .unwrap()
            .occurrences
            .len(),
        2
    );
}

#[test]
fn ttl_days_defaults_to_30_and_rejects_zero() {
    let sandbox = Sandbox::new();
    let inputs = sandbox.inputs();
    assert_eq!(config::load(&inputs).unwrap().config.evidence.ttl_days, 30);

    let user = PathBuf::from(&inputs.vars["XDG_CONFIG_HOME"]).join("oncebitten");
    fs::create_dir_all(&user).unwrap();
    fs::write(user.join("config.toml"), "[evidence]\nttl_days = 7\n").unwrap();
    assert_eq!(config::load(&inputs).unwrap().config.evidence.ttl_days, 7);

    fs::write(user.join("config.toml"), "[evidence]\nttl_days = 0\n").unwrap();
    let error = config::load(&inputs).unwrap_err();
    assert!(format!("{error:#}").contains("ttl_days"), "{error:#}");
}

fn capture(home: &Path, payload: &str, config: Option<&str>) {
    if let Some(text) = config {
        let dir = home.join("config").join("oncebitten");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), text).unwrap();
    }
    let mut child = Command::new(BIN)
        .args(["capture", "--host", "claude"])
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env_remove("ONCEBITTEN_TRIAGE")
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn capture_expires_events_older_than_the_ttl() {
    let home = TempDir::new().unwrap();
    let db = home
        .path()
        .join("data")
        .join("oncebitten")
        .join("events.db");
    let store = Store::open(&db).unwrap();
    fail(&store, "old", "cargo doc", now_ms() - 8 * DAY_MS);
    fail(&store, "recent", "cargo doc", now_ms() - 6 * DAY_MS);
    drop(store);

    let payload = r#"{"session_id":"s9","cwd":"/nowhere","hook_event_name":"UserPromptSubmit","prompt":"hello"}"#;
    capture(home.path(), payload, Some("[evidence]\nttl_days = 7\n"));

    let store = Store::open(&db).unwrap();
    assert_eq!(event_count(&store, "old"), 0);
    assert_eq!(event_count(&store, "recent"), 1);
}

#[test]
fn capture_keeps_events_inside_the_default_ttl() {
    let home = TempDir::new().unwrap();
    let db = home
        .path()
        .join("data")
        .join("oncebitten")
        .join("events.db");
    let store = Store::open(&db).unwrap();
    fail(&store, "old", "cargo doc", now_ms() - 29 * DAY_MS);
    fail(&store, "older", "cargo doc", now_ms() - 31 * DAY_MS);
    drop(store);

    let payload = r#"{"session_id":"s9","cwd":"/nowhere","hook_event_name":"UserPromptSubmit","prompt":"hello"}"#;
    capture(home.path(), payload, None);

    let store = Store::open(&db).unwrap();
    assert_eq!(event_count(&store, "old"), 1);
    assert_eq!(event_count(&store, "older"), 0);
}

/// A git repo (marked by `.git`) with an isolated user config and data root.
struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("repo").join(".git")).unwrap();
        Sandbox { root }
    }

    fn inputs(&self) -> Inputs {
        let home = self.root.path().join("home");
        let vars: BTreeMap<String, String> = [
            ("HOME", home.clone()),
            ("USERPROFILE", home.clone()),
            ("XDG_CONFIG_HOME", home.join("config")),
            ("XDG_DATA_HOME", home.join("data")),
            ("APPDATA", home.join("appdata")),
            ("LOCALAPPDATA", home.join("localappdata")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_string_lossy().into_owned()))
        .collect();
        Inputs {
            cwd: self.root.path().join("repo"),
            vars,
        }
    }
}

fn fail(store: &Store, session: &str, command: &str, ts: i64) -> i64 {
    let event = NewEvent {
        ts,
        host: "codex".into(),
        session_id: session.into(),
        repo: Some("/r".into()),
        kind: signal::KIND_TOOL_ERROR.into(),
        tool: Some("Bash".into()),
        summary: command.into(),
    };
    let history = store.session_history("codex", session, 500).unwrap();
    let signals = signal::derive(&event, &history);
    store.record(&event, &signals).unwrap().unwrap()
}

fn cluster(store: &Store) -> Cluster {
    fail(store, "s1", "cargo doc", 1);
    fail(store, "s2", "cargo doc", 2);
    evidence::load(store).unwrap().remove(0)
}

fn answer() -> Answer {
    triage_verdict::parse_answer(ANSWER).unwrap()
}

fn no_sandbox(_rule: &Rule) -> Replayed {
    Replayed {
        verdict: Verdict::CouldNotRun("openshell is not installed".into()),
        written: None,
    }
}

fn event_count(store: &Store, session: &str) -> usize {
    store.session_history("codex", session, 500).unwrap().len()
}
