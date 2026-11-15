use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::config::Inputs;
use oncebitten::evidence::{self, Cluster};
use oncebitten::proof::{self, BackendKind, Replayed, Validity};
use oncebitten::proposals::{self, Kind, Recorded, State};
use oncebitten::replay::{LocalRunner, Verdict};
use oncebitten::rule::{self, Rule, Status};
use oncebitten::signal;
use oncebitten::store::{NewEvent, Store};
use oncebitten::triage::verdict::{self as triage_verdict, Answer};
use tempfile::TempDir;

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
const VERDICT: &str = r#""repeatable":true,"user_correction":false,"root_cause":"docs drift","proposed_fix_kind":"lint","generality":"project""#;

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

    fn repo(&self) -> PathBuf {
        self.root.path().join("repo")
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
            cwd: self.repo(),
            vars,
        }
    }

    fn rules_dir(&self) -> PathBuf {
        self.repo().join(".oncebitten").join("rules")
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
    let clusters = evidence::load(store).unwrap();
    assert_eq!(clusters.len(), 1);
    clusters.into_iter().next().unwrap()
}

fn draft_json(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "description": "a change under src needs a docs change",
        "mode": "block",
        "engine": "diff",
        "when_changed": ["src/**"],
        "require_changed": ["docs/**"],
        "bad": {"name": "bad.diff", "content": "src/lib.rs\n"},
        "fixed": {"name": "fixed.diff", "content": "src/lib.rs\ndocs/a.md\n"},
        "corpus": [{"name": "readme-only.diff", "content": "README.md\n"}],
        "rationale": "agents forgot docs twice"
    })
}

fn answer_with(draft: Option<serde_json::Value>) -> Answer {
    let text = match draft {
        Some(draft) => format!("{{{VERDICT},\"draft\":{draft}}}"),
        None => format!("{{{VERDICT}}}"),
    };
    triage_verdict::parse_answer(&text).unwrap()
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

#[test]
fn a_draft_becomes_a_proven_proposed_rule_with_provenance() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    let answer = answer_with(Some(draft_json("docs-with-src")));

    let recorded =
        proposals::record(&store, &sandbox.inputs(), &cluster, &answer, &proving, 10).unwrap();
    assert_eq!(recorded, Recorded::Proposed { proven: true });

    let dir = sandbox.rules_dir().join("docs-with-src");
    let rule = rule::load_rule(&dir).unwrap();
    assert_eq!(rule.status, Status::Proposed);
    assert_eq!(proof::validity(&rule), Validity::Valid);

    let row = store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(row.kind, Kind::Proposal);
    assert_eq!(row.state, State::Waiting);
    assert_eq!(row.rule.as_deref(), Some("docs-with-src"));
    assert!(row.proven);
    assert_eq!(row.rationale.as_deref(), Some("agents forgot docs twice"));
    let ids: Vec<i64> = cluster.occurrences.iter().map(|o| o.event.id).collect();
    assert_eq!(row.occurrences, ids);
    assert_eq!(row.occurrences.len(), 2);
}

#[test]
fn the_same_cluster_is_never_proposed_twice() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    let inputs = sandbox.inputs();
    proposals::record(
        &store,
        &inputs,
        &cluster,
        &answer_with(Some(draft_json("docs-with-src"))),
        &no_sandbox,
        10,
    )
    .unwrap();
    let again = proposals::record(
        &store,
        &inputs,
        &cluster,
        &answer_with(Some(draft_json("other-id"))),
        &no_sandbox,
        11,
    )
    .unwrap();
    assert_eq!(again, Recorded::AlreadyQueued);
    assert!(!sandbox.rules_dir().join("other-id").exists());
    assert_eq!(store.proposals().unwrap().len(), 1);
}

#[test]
fn without_a_sandbox_the_proposal_stays_proposed_and_unproven() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    let recorded = proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer_with(Some(draft_json("docs-with-src"))),
        &no_sandbox,
        10,
    )
    .unwrap();
    assert_eq!(recorded, Recorded::Proposed { proven: false });
    let dir = sandbox.rules_dir().join("docs-with-src");
    assert!(!dir.join(proof::PROOF_FILE).exists());
    let row = store.proposal(&cluster.id).unwrap().unwrap();
    assert!(!row.proven);
    assert!(row
        .note
        .as_deref()
        .unwrap()
        .contains("openshell is not installed"));
}

#[test]
fn a_verdict_without_a_draft_queues_a_finding_and_writes_no_rule() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    let recorded = proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer_with(None),
        &no_sandbox,
        10,
    )
    .unwrap();
    assert_eq!(recorded, Recorded::Finding);
    assert!(!sandbox.rules_dir().exists());
    let row = store.proposal(&cluster.id).unwrap().unwrap();
    assert_eq!(row.kind, Kind::Finding);
    assert!(row.verdict.contains("docs drift"));
}

#[test]
fn a_draft_whose_id_is_taken_is_not_written() {
    let sandbox = Sandbox::new();
    let existing = sandbox.rules_dir().join("docs-with-src");
    fs::create_dir_all(&existing).unwrap();
    fs::write(existing.join("keep.txt"), "mine").unwrap();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    let result = proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer_with(Some(draft_json("docs-with-src"))),
        &no_sandbox,
        10,
    );
    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(existing.join("keep.txt")).unwrap(),
        "mine"
    );
    assert!(store.proposal(&cluster.id).unwrap().is_none());
}

#[test]
fn accept_flips_a_proven_proposal_to_active_and_keeps_the_proof_valid() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer_with(Some(draft_json("docs-with-src"))),
        &proving,
        10,
    )
    .unwrap();
    proposals::accept(&store, &cluster.id).unwrap();
    let rule = rule::load_rule(&sandbox.rules_dir().join("docs-with-src")).unwrap();
    assert_eq!(rule.status, Status::Active);
    assert_eq!(proof::validity(&rule), Validity::Valid);
    assert_eq!(
        store.proposal(&cluster.id).unwrap().unwrap().state,
        State::Accepted
    );
}

#[test]
fn accept_refuses_an_unproven_proposal_and_a_finding() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer_with(Some(draft_json("docs-with-src"))),
        &no_sandbox,
        10,
    )
    .unwrap();
    let error = proposals::accept(&store, &cluster.id).unwrap_err();
    assert!(format!("{error:#}").contains("proof"), "{error:#}");
    let rule = rule::load_rule(&sandbox.rules_dir().join("docs-with-src")).unwrap();
    assert_eq!(rule.status, Status::Proposed);

    let other = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = self::cluster(&store);
    proposals::record(
        &store,
        &other.inputs(),
        &cluster,
        &answer_with(None),
        &no_sandbox,
        10,
    )
    .unwrap();
    assert!(proposals::accept(&store, &cluster.id).is_err());
    assert!(proposals::accept(&store, "000000000000").is_err());
}

#[test]
fn accept_works_on_a_crlf_checkout() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer_with(Some(draft_json("docs-with-src"))),
        &proving,
        10,
    )
    .unwrap();
    let file = sandbox.rules_dir().join("docs-with-src").join("rule.toml");
    let text = fs::read_to_string(&file).unwrap().replace('\n', "\r\n");
    fs::write(&file, text).unwrap();
    proposals::accept(&store, &cluster.id).unwrap();
    let rule = rule::load_rule(&sandbox.rules_dir().join("docs-with-src")).unwrap();
    assert_eq!(rule.status, Status::Active);
    assert_eq!(proof::validity(&rule), Validity::Valid);
}

#[test]
fn reject_removes_the_proposed_rule_and_stops_the_cluster_from_reproposing() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    let inputs = sandbox.inputs();
    proposals::record(
        &store,
        &inputs,
        &cluster,
        &answer_with(Some(draft_json("docs-with-src"))),
        &no_sandbox,
        10,
    )
    .unwrap();
    proposals::reject(&store, &cluster.id, "too noisy", 20).unwrap();
    assert!(!sandbox.rules_dir().join("docs-with-src").exists());
    assert_eq!(
        store.proposal(&cluster.id).unwrap().unwrap().state,
        State::Rejected
    );
    let reloaded = evidence::find(&evidence::load(&store).unwrap(), &cluster.id)
        .unwrap()
        .clone();
    assert!(reloaded.rejection.is_some());
    let again = proposals::record(
        &store,
        &inputs,
        &reloaded,
        &answer_with(Some(draft_json("docs-with-src"))),
        &no_sandbox,
        30,
    )
    .unwrap();
    assert_eq!(again, Recorded::Rejected);
    assert!(!sandbox.rules_dir().join("docs-with-src").exists());
}

#[test]
fn reject_keeps_a_rule_that_is_no_longer_proposed() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer_with(Some(draft_json("docs-with-src"))),
        &proving,
        10,
    )
    .unwrap();
    proposals::accept(&store, &cluster.id).unwrap();
    proposals::reject(&store, &cluster.id, "changed my mind", 20).unwrap();
    assert!(sandbox
        .rules_dir()
        .join("docs-with-src")
        .join("rule.toml")
        .is_file());
}

#[test]
fn the_notice_counts_waiting_proposals_for_this_repo_only() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    assert_eq!(proposals::notice(&store, &sandbox.repo()).unwrap(), None);
    let cluster = cluster(&store);
    proposals::record(
        &store,
        &sandbox.inputs(),
        &cluster,
        &answer_with(Some(draft_json("docs-with-src"))),
        &no_sandbox,
        10,
    )
    .unwrap();
    assert_eq!(
        proposals::notice(&store, &sandbox.repo())
            .unwrap()
            .as_deref(),
        Some("1 rule proposal waiting: oncebitten proposals list")
    );
    let other = Sandbox::new();
    assert_eq!(proposals::notice(&store, &other.repo()).unwrap(), None);
    proposals::reject(&store, &cluster.id, "no", 20).unwrap();
    assert_eq!(proposals::notice(&store, &sandbox.repo()).unwrap(), None);
}

#[test]
fn the_notice_uses_the_plural_for_several() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    fail(&store, "s1", "cargo doc", 1);
    fail(&store, "s2", "cargo doc", 2);
    fail(&store, "s1", "npm run lint", 3);
    fail(&store, "s2", "npm run lint", 4);
    let clusters = evidence::load(&store).unwrap();
    assert_eq!(clusters.len(), 2);
    for (index, cluster) in clusters.iter().enumerate() {
        proposals::record(
            &store,
            &sandbox.inputs(),
            cluster,
            &answer_with(Some(draft_json(&format!("rule-{index}")))),
            &no_sandbox,
            10,
        )
        .unwrap();
    }
    assert_eq!(
        proposals::notice(&store, &sandbox.repo())
            .unwrap()
            .as_deref(),
        Some("2 rule proposals waiting: oncebitten proposals list")
    );
}

#[test]
fn a_triage_lock_admits_one_holder_until_released_or_stale() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.try_lock_triage("c1", 1_000, 60_000).unwrap());
    assert!(!store.try_lock_triage("c1", 2_000, 60_000).unwrap());
    assert!(store.try_lock_triage("c2", 2_000, 60_000).unwrap());
    store.unlock_triage("c1").unwrap();
    assert!(store.try_lock_triage("c1", 3_000, 60_000).unwrap());
    assert!(store.try_lock_triage("c1", 70_000, 60_000).unwrap());
}

#[test]
fn a_cluster_reaches_its_threshold_once() {
    let store = Store::open_in_memory().unwrap();
    let first = fail(&store, "s1", "cargo doc", 1);
    let clusters = evidence::load(&store).unwrap();
    assert!(evidence::reached_threshold(&clusters, first).is_empty());
    let second = fail(&store, "s2", "cargo doc", 2);
    let clusters = evidence::load(&store).unwrap();
    let reached = evidence::reached_threshold(&clusters, second);
    assert_eq!(reached.len(), 1);
    assert_eq!(reached[0].id, clusters[0].id);
    let third = fail(&store, "s2", "cargo doc", 3);
    let fourth = fail(&store, "s3", "cargo doc", 4);
    let clusters = evidence::load(&store).unwrap();
    assert!(evidence::reached_threshold(&clusters, third).is_empty());
    assert!(evidence::reached_threshold(&clusters, fourth).is_empty());
}

#[test]
fn the_store_keeps_its_schema_version_with_the_queue_tables() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("events.db");
    let store = Store::open(&path).unwrap();
    assert!(store.proposals().unwrap().is_empty());
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 4);
}

/// A host that answers every call with `answer` and never runs a model.
struct FakeHost {
    answer: String,
    calls: std::cell::Cell<usize>,
}

impl FakeHost {
    fn new(answer: String) -> Self {
        FakeHost {
            answer,
            calls: std::cell::Cell::new(0),
        }
    }
}

impl oncebitten::triage::run::Runner for FakeHost {
    fn run(
        &self,
        _inv: &oncebitten::triage::host::Invocation,
        dir: &Path,
        _timeout: std::time::Duration,
    ) -> Result<oncebitten::triage::host::Captured, oncebitten::triage::run::RunError> {
        // The control call proves hooks fire; the triage call fires none.
        if self.calls.get() == 0 {
            let markers = dir.join("markers");
            fs::create_dir_all(&markers).unwrap();
            fs::write(markers.join("marker-0"), "").unwrap();
        }
        self.calls.set(self.calls.get() + 1);
        Ok(oncebitten::triage::host::Captured {
            stdout: self.answer.clone(),
            answer: Some(self.answer.clone()),
        })
    }
}

fn fail_in(store: &Store, session: &str, repo: &Path, ts: i64) -> i64 {
    let event = NewEvent {
        ts,
        host: "codex".into(),
        session_id: session.into(),
        repo: Some(repo.to_string_lossy().into_owned()),
        kind: signal::KIND_TOOL_ERROR.into(),
        tool: Some("Bash".into()),
        summary: "cargo doc".into(),
    };
    let history = store.session_history("codex", session, 500).unwrap();
    let signals = signal::derive(&event, &history);
    store.record(&event, &signals).unwrap().unwrap()
}

#[test]
fn due_names_the_cluster_and_repo_an_event_brought_to_its_threshold() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let sub = sandbox.repo().join("src");
    fs::create_dir_all(&sub).unwrap();
    let first = fail_in(&store, "s1", &sub, 1);
    assert!(proposals::due(&store, first).unwrap().is_empty());
    let second = fail_in(&store, "s2", &sub, 2);
    let due = proposals::due(&store, second).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(
        fs::canonicalize(&due[0].repo).unwrap(),
        fs::canonicalize(sandbox.repo()).unwrap()
    );
    let cluster = evidence::load(&store).unwrap().remove(0);
    assert_eq!(due[0].cluster, cluster.id);
}

#[test]
fn due_skips_events_outside_a_repo_and_queued_or_rejected_clusters() {
    // A relative repo is no repo: it would resolve against capture's own cwd.
    let store = Store::open_in_memory().unwrap();
    fail_in(&store, "s1", Path::new("relative"), 1);
    let second = fail_in(&store, "s2", Path::new("relative"), 2);
    assert!(proposals::due(&store, second).unwrap().is_empty());

    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    fail_in(&store, "s1", &sandbox.repo(), 1);
    let second = fail_in(&store, "s2", &sandbox.repo(), 2);
    let cluster = evidence::load(&store).unwrap().remove(0);
    evidence::reject(&store, &cluster.id, 3, "noise").unwrap();
    assert!(proposals::due(&store, second).unwrap().is_empty());
}

#[test]
fn queued_triage_asks_the_host_once_and_records_the_draft() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    let answer = format!("{{{VERDICT},\"draft\":{}}}", draft_json("docs-with-src"));
    let host = FakeHost::new(answer);
    let ask = |cluster: &Cluster| {
        oncebitten::triage::flow::run_answer(
            &host,
            oncebitten::config::Host::Codex,
            "small",
            &oncebitten::triage::flow::prompt_input(cluster),
            oncebitten::triage::flow::DEFAULT_TIMEOUT,
        )
    };
    let ran = proposals::triage_queued(
        &store,
        &sandbox.inputs(),
        &cluster.id,
        &ask,
        &no_sandbox,
        10,
    )
    .unwrap();
    assert_eq!(
        ran,
        proposals::Ran::Recorded(Recorded::Proposed { proven: false })
    );
    assert!(sandbox.rules_dir().join("docs-with-src").is_dir());
    let calls = host.calls.get();
    assert!(calls > 0);

    let again = proposals::triage_queued(
        &store,
        &sandbox.inputs(),
        &cluster.id,
        &ask,
        &no_sandbox,
        20,
    )
    .unwrap();
    assert_eq!(again, proposals::Ran::Skipped);
    assert_eq!(host.calls.get(), calls);
}

#[test]
fn queued_triage_waits_for_a_held_lock_and_releases_its_own() {
    let sandbox = Sandbox::new();
    let store = Store::open_in_memory().unwrap();
    let cluster = cluster(&store);
    let ask = |_: &Cluster| -> anyhow::Result<Answer> { Ok(answer_with(None)) };
    assert!(store.try_lock_triage(&cluster.id, 5, 60_000).unwrap());
    let ran = proposals::triage_queued(
        &store,
        &sandbox.inputs(),
        &cluster.id,
        &ask,
        &no_sandbox,
        10,
    )
    .unwrap();
    assert_eq!(ran, proposals::Ran::Locked);
    store.unlock_triage(&cluster.id).unwrap();

    let failing = |_: &Cluster| -> anyhow::Result<Answer> { anyhow::bail!("host timed out") };
    assert!(proposals::triage_queued(
        &store,
        &sandbox.inputs(),
        &cluster.id,
        &failing,
        &no_sandbox,
        20
    )
    .is_err());
    assert!(store.try_lock_triage(&cluster.id, 30, 60_000).unwrap());
}

#[test]
fn background_triage_is_on_by_default_and_a_config_key_turns_it_off() {
    let sandbox = Sandbox::new();
    let loaded = oncebitten::config::load(&sandbox.inputs()).unwrap();
    assert!(loaded.config.triage.background);
    let project = sandbox.repo().join(".oncebitten");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        project.join("config.toml"),
        "[triage]\r\nbackground = false\r\n",
    )
    .unwrap();
    let loaded = oncebitten::config::load(&sandbox.inputs()).unwrap();
    assert!(!loaded.config.triage.background);
}
