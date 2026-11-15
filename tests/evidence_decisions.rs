mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use common::{both, code, stdout, Repo};
use oncebitten::config::Inputs;
use oncebitten::evidence::{self, Cluster};
use oncebitten::proof::{self, BackendKind, Replayed};
use oncebitten::proposals::{self, Recorded};
use oncebitten::replay::{LocalRunner, Verdict};
use oncebitten::rule::Rule;
use oncebitten::signal::{self, Signal};
use oncebitten::store::{NewEvent, Outcome, Store};
use oncebitten::triage::verdict::{self as triage_verdict, Answer};
use tempfile::TempDir;

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
const ANSWER: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"docs drift","proposed_fix_kind":"lint","generality":"project","draft":{"id":"docs-with-src","description":"a change under src needs a docs change","mode":"block","engine":"diff","when_changed":["src/**"],"require_changed":["docs/**"],"bad":{"name":"bad.diff","content":"src/lib.rs\n"},"fixed":{"name":"fixed.diff","content":"src/lib.rs\ndocs/a.md\n"},"corpus":[{"name":"readme.diff","content":"README.md\n"}],"rationale":"agents forgot docs twice"}}"#;

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

fn proving(rule: &Rule) -> Replayed {
    proof::replay_rule(
        rule,
        Path::new(CHECKER),
        &LocalRunner,
        BackendKind::OpenShell,
    )
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

#[test]
fn accept_deletes_the_cluster_events_and_keeps_one_provenance_record() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    proposals::record(&store, &sandbox.inputs(), &cluster, &answer(), &proving, 10).unwrap();

    proposals::accept(&store, &cluster.id).unwrap();

    assert!(store.signals().unwrap().is_empty());
    assert_eq!(event_count(&store, "s1"), 0);
    assert_eq!(event_count(&store, "s2"), 0);
    let decision = store.decision(&cluster.id).unwrap().unwrap();
    assert_eq!(decision.outcome, Outcome::Accepted);
    assert_eq!(decision.rule.as_deref(), Some("docs-with-src"));
    assert_eq!(decision.reason, None);
    assert_eq!(decision.occurrences, 2);
    assert_eq!(decision.sessions, 2);
    assert_eq!(decision.first_seen, Some(1));
    assert_eq!(decision.last_seen, Some(2));
    assert!(decision.ts > 10);
    assert_eq!(store.decisions().unwrap().len(), 1);
}

#[test]
fn reject_deletes_the_cluster_events_and_records_the_reason() {
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

    proposals::reject(&store, &cluster.id, " too noisy ", 20).unwrap();

    assert!(store.signals().unwrap().is_empty());
    let decision = store.decision(&cluster.id).unwrap().unwrap();
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.reason.as_deref(), Some("too noisy"));
    assert_eq!(decision.rule.as_deref(), Some("docs-with-src"));
    assert_eq!(decision.ts, 20);
    assert_eq!(
        (decision.occurrences, decision.sessions),
        (2, 2),
        "{decision:?}"
    );
}

#[test]
fn a_decided_cluster_stays_listed_without_its_events() {
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
    proposals::reject(&store, &cluster.id, "noise", 20).unwrap();

    let clusters = evidence::load(&store).unwrap();
    let found = evidence::find(&clusters, &cluster.id).unwrap();
    assert!(found.occurrences.is_empty());
    assert_eq!(found.class, cluster.class);
    assert_eq!(found.source, cluster.source);
    assert_eq!(found.tool, cluster.tool);
    assert_eq!(found.pattern, cluster.pattern);
    assert_eq!(found.status(), "rejected");
    assert_eq!(found.decision.as_ref().unwrap().occurrences, 2);
}

#[test]
fn a_rejected_cluster_is_not_proposed_again_after_its_events_are_gone() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    let inputs = sandbox.inputs();
    proposals::record(&store, &inputs, &cluster, &answer(), &no_sandbox, 10).unwrap();
    proposals::reject(&store, &cluster.id, "noise", 20).unwrap();

    fail(&store, "s3", "cargo doc", 30);
    let event = fail(&store, "s4", "cargo doc", 31);

    assert!(proposals::due(&store, event).unwrap().is_empty());
    let clusters = evidence::load(&store).unwrap();
    let again = evidence::find(&clusters, &cluster.id).unwrap();
    assert_eq!(again.occurrences.len(), 2);
    assert!(again.rejection.is_some());
    assert!(!again.is_candidate());
    let recorded = proposals::record(&store, &inputs, again, &answer(), &no_sandbox, 40).unwrap();
    assert_eq!(recorded, Recorded::Rejected);
}

#[test]
fn an_accepted_cluster_is_not_a_candidate_when_it_recurs() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    proposals::record(&store, &sandbox.inputs(), &cluster, &answer(), &proving, 10).unwrap();
    proposals::accept(&store, &cluster.id).unwrap();

    fail(&store, "s3", "cargo doc", 30);
    let event = fail(&store, "s4", "cargo doc", 31);

    assert!(proposals::due(&store, event).unwrap().is_empty());
    let clusters = evidence::load(&store).unwrap();
    let found = evidence::find(&clusters, &cluster.id).unwrap();
    assert_eq!(found.status(), "accepted");
    assert!(!found.is_candidate());
}

#[test]
fn a_second_decision_adds_the_newer_occurrences_to_the_record() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    proposals::record(&store, &sandbox.inputs(), &cluster, &answer(), &proving, 10).unwrap();
    proposals::accept(&store, &cluster.id).unwrap();
    fail(&store, "s3", "cargo doc", 30);

    proposals::reject(&store, &cluster.id, "changed my mind", 40).unwrap();

    assert!(store.signals().unwrap().is_empty());
    let decision = store.decision(&cluster.id).unwrap().unwrap();
    assert_eq!(decision.outcome, Outcome::Rejected);
    assert_eq!(decision.reason.as_deref(), Some("changed my mind"));
    assert_eq!((decision.occurrences, decision.sessions), (3, 3));
    assert_eq!(
        (decision.first_seen, decision.last_seen),
        (Some(1), Some(30))
    );
}

#[test]
fn deciding_a_cluster_keeps_an_event_another_cluster_still_uses() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let shared = NewEvent {
        ts: 1,
        host: "codex".into(),
        session_id: "s1".into(),
        repo: Some("/r".into()),
        kind: signal::KIND_TOOL_ERROR.into(),
        tool: Some("Bash".into()),
        summary: "git revert HEAD".into(),
    };
    let signals = [
        Signal {
            kind: signal::TOOL_FAILURE,
            detail: "exit 1".into(),
        },
        Signal {
            kind: signal::REVERT,
            detail: "revert".into(),
        },
    ];
    store.record(&shared, &signals).unwrap().unwrap();
    let clusters = evidence::load(&store).unwrap();
    assert_eq!(clusters.len(), 2);
    let failure = clusters
        .iter()
        .find(|c| c.class == evidence::CLASS_FAILURE)
        .unwrap();
    proposals::record(
        &store,
        &sandbox.inputs(),
        failure,
        &answer(),
        &no_sandbox,
        10,
    )
    .unwrap();

    proposals::reject(&store, &failure.id, "noise", 20).unwrap();

    let rows = store.signals().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, signal::REVERT);
    assert_eq!(event_count(&store, "s1"), 1);
}

fn repo_db(repo: &Repo) -> PathBuf {
    repo.root
        .path()
        .join("data")
        .join("oncebitten")
        .join("events.db")
}

fn repo_inputs(repo: &Repo) -> Inputs {
    let vars: BTreeMap<String, String> = [
        ("HOME", repo.root.path().join("home")),
        ("XDG_CONFIG_HOME", repo.root.path().join("xdg-config")),
        ("XDG_DATA_HOME", repo.root.path().join("data")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_string_lossy().into_owned()))
    .collect();
    Inputs {
        cwd: repo.project(),
        vars,
    }
}

#[test]
fn show_prints_the_counts_once_the_events_are_gone() {
    let repo = Repo::new();
    repo.write("README.md", "hello\n");
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    repo.commit_all("base");
    fs::create_dir_all(repo_db(&repo).parent().unwrap()).unwrap();
    let store = Store::open(&repo_db(&repo)).unwrap();
    let cluster = cluster(&store);
    proposals::record(
        &store,
        &repo_inputs(&repo),
        &cluster,
        &answer(),
        &no_sandbox,
        10,
    )
    .unwrap();
    drop(store);

    let reject = repo.run(
        &["proposals", "reject", &cluster.id, "--reason", "noise"],
        &[],
    );
    assert_eq!(code(&reject), 0, "{}", both(&reject));

    let show = repo.run(&["proposals", "show", &cluster.id], &[]);
    assert_eq!(code(&show), 0, "{}", both(&show));
    let text = stdout(&show);
    assert!(
        text.contains("occurrences: 2 in 2 sessions, first seen 1, last seen 2 (events deleted)"),
        "{text}"
    );
    assert!(!text.contains("occurrences: 1, 2"), "{text}");

    let evidence = repo.run(&["evidence", "show", &cluster.id], &[]);
    assert_eq!(code(&evidence), 0, "{}", both(&evidence));
    let text = stdout(&evidence);
    assert!(text.contains("rejected: noise"), "{text}");
    assert!(
        text.contains("occurrences: 2 in 2 sessions, first seen 1, last seen 2 (events deleted)"),
        "{text}"
    );
}
