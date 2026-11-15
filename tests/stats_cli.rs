mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::{both, code, stdout, Repo};
use oncebitten::config::Inputs;
use oncebitten::evidence;
use oncebitten::proof::Replayed;
use oncebitten::proposals::{self, Recorded};
use oncebitten::replay::Verdict;
use oncebitten::rule::Rule;
use oncebitten::signal;
use oncebitten::store::{NewEvent, Store};
use oncebitten::triage::verdict;
use serde_json::Value;

const ANSWER: &str = r#"{"repeatable":true,"user_correction":false,"root_cause":"docs drift","proposed_fix_kind":"lint","generality":"project","draft":{"id":"docs-with-src","description":"a change under src needs a docs change","mode":"block","engine":"diff","when_changed":["src/**"],"require_changed":["docs/**"],"bad":{"name":"bad.diff","content":"src/lib.rs\n"},"fixed":{"name":"fixed.diff","content":"src/lib.rs\ndocs/a.md\n"},"corpus":[{"name":"readme.diff","content":"README.md\n"}],"rationale":"agents forgot docs twice"}}"#;

fn db(repo: &Repo) -> PathBuf {
    repo.root
        .path()
        .join("data")
        .join("oncebitten")
        .join("events.db")
}

fn inputs(repo: &Repo) -> Inputs {
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

fn no_sandbox(_rule: &Rule) -> Replayed {
    Replayed {
        verdict: Verdict::CouldNotRun("openshell is not installed".into()),
        written: None,
    }
}

fn committed_repo() -> Repo {
    let repo = Repo::new();
    repo.write("README.md", "hello\n");
    repo.commit_all("base");
    repo
}

/// Captures one failing command in two sessions, then queues an unproven
/// draft for the resulting cluster. Returns the cluster id.
fn propose(repo: &Repo) -> String {
    std::fs::create_dir_all(db(repo).parent().unwrap()).unwrap();
    let store = Store::open(&db(repo)).unwrap();
    for (session, ts) in [("s1", 1), ("s2", 2)] {
        let event = NewEvent {
            ts,
            host: "pi".into(),
            session_id: session.into(),
            repo: Some(repo.project().to_string_lossy().into_owned()),
            kind: signal::KIND_TOOL_ERROR.into(),
            tool: Some("Bash".into()),
            summary: "cargo doc".into(),
        };
        let history = store.session_history("pi", session, 500).unwrap();
        let signals = signal::derive(&event, &history);
        store.record(&event, &signals).unwrap();
    }
    let cluster = evidence::load(&store).unwrap().remove(0);
    let answer = verdict::parse_answer(ANSWER).unwrap();
    let recorded =
        proposals::record(&store, &inputs(repo), &cluster, &answer, &no_sandbox, 10).unwrap();
    assert_eq!(recorded, Recorded::Proposed { proven: false });
    cluster.id
}

fn stats_json(repo: &Repo, extra: &[&str]) -> Value {
    let mut args = vec!["stats", "--json"];
    args.extend_from_slice(extra);
    let output = repo.run(&args, &[]);
    assert_eq!(code(&output), 0, "{}", both(&output));
    serde_json::from_str(&stdout(&output))
        .unwrap_or_else(|error| panic!("stats --json is not JSON ({error}): {}", stdout(&output)))
}

fn row<'a>(report: &'a Value, kind: &str) -> &'a Value {
    report["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == kind)
        .unwrap_or_else(|| panic!("no row for {kind}: {report}"))
}

#[test]
fn no_store_prints_no_data_and_creates_nothing() {
    let repo = committed_repo();
    let text = repo.run(&["stats"], &[]);
    assert_eq!(code(&text), 0, "{}", both(&text));
    assert!(stdout(&text).contains("no data"), "{}", both(&text));
    let json = repo.run(&["stats", "--json"], &[]);
    assert_eq!(code(&json), 0, "{}", both(&json));
    assert!(!db(&repo).exists());
}

#[test]
fn text_report_prints_every_stage_and_marks_triage_runs_n_a() {
    let repo = committed_repo();
    propose(&repo);
    let output = repo.run(&["stats"], &[]);
    assert_eq!(code(&output), 0, "{}", both(&output));
    let text = stdout(&output);
    assert!(text.contains("failure"), "{text}");
    assert!(text.contains("signals captured"), "{text}");
    assert!(text.contains("clusters at threshold"), "{text}");
    assert!(text.contains("triage runs"), "{text}");
    assert!(text.contains("n/a"), "{text}");
    assert!(text.contains("rule-shaped drafts"), "{text}");
    assert!(text.contains("proven"), "{text}");
    assert!(text.contains("accepted"), "{text}");
    assert!(text.contains("rejected"), "{text}");
    assert!(text.contains("waiting"), "{text}");
}

#[test]
fn json_counts_a_waiting_draft_through_the_funnel() {
    let repo = committed_repo();
    propose(&repo);
    let report = stats_json(&repo, &[]);
    let failure = row(&report, "failure");
    assert_eq!(failure["signals_captured"], 2, "{report}");
    assert_eq!(failure["clusters_at_threshold"], 1, "{report}");
    assert_eq!(failure["triage_runs"], Value::Null, "{report}");
    assert_eq!(failure["drafts"], 1, "{report}");
    assert_eq!(failure["proven"], 0, "{report}");
    assert_eq!(failure["accepted"], 0, "{report}");
    assert_eq!(failure["rejected"], 0, "{report}");
    assert_eq!(failure["waiting"], 1, "{report}");
    let revert = row(&report, "revert");
    assert_eq!(revert["signals_captured"], 0, "{report}");
    assert_eq!(revert["drafts"], 0, "{report}");
}

#[test]
fn a_rejected_draft_moves_from_waiting_to_rejected() {
    let repo = committed_repo();
    let id = propose(&repo);
    let reject = repo.run(&["proposals", "reject", &id, "--reason", "noise"], &[]);
    assert_eq!(code(&reject), 0, "{}", both(&reject));
    let report = stats_json(&repo, &[]);
    let failure = row(&report, "failure");
    assert_eq!(failure["drafts"], 1, "{report}");
    assert_eq!(failure["rejected"], 1, "{report}");
    assert_eq!(failure["waiting"], 0, "{report}");
}

#[test]
fn since_leaves_out_records_older_than_the_window() {
    let repo = committed_repo();
    propose(&repo);
    let report = stats_json(&repo, &["--since", "1"]);
    assert_eq!(report["since_days"], 1, "{report}");
    let failure = row(&report, "failure");
    assert_eq!(failure["signals_captured"], 0, "{report}");
    assert_eq!(failure["drafts"], 0, "{report}");
}

/// Writes a store file at `version` with one table, as an older or newer build
/// would leave it. Returns the file bytes.
fn write_store_at_version(repo: &Repo, version: i64) -> Vec<u8> {
    std::fs::create_dir_all(db(repo).parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(db(repo)).unwrap();
    conn.execute_batch("CREATE TABLE marker (x INTEGER);")
        .unwrap();
    conn.pragma_update(None, "user_version", version).unwrap();
    drop(conn);
    std::fs::read(db(repo)).unwrap()
}

#[test]
fn stats_leaves_a_store_at_another_schema_unchanged() {
    for version in [3, 99] {
        let repo = committed_repo();
        let before = write_store_at_version(&repo, version);
        let output = repo.run(&["stats"], &[]);
        assert_eq!(code(&output), 0, "{}", both(&output));
        assert!(
            stdout(&output).contains("store version differs"),
            "{}",
            both(&output)
        );
        assert_eq!(
            std::fs::read(db(&repo)).unwrap(),
            before,
            "stats changed a store at schema {version}"
        );
    }
}

#[test]
fn a_waiting_draft_without_evidence_counts_as_unknown() {
    let repo = committed_repo();
    propose(&repo);
    // Expiry spares the events of a waiting draft, so drop them directly.
    let conn = rusqlite::Connection::open(db(&repo)).unwrap();
    conn.execute_batch("DELETE FROM signals; DELETE FROM events;")
        .unwrap();
    drop(conn);
    let report = stats_json(&repo, &[]);
    let unknown = row(&report, "unknown");
    assert_eq!(unknown["drafts"], 1, "{report}");
    assert_eq!(unknown["waiting"], 1, "{report}");
    assert_eq!(row(&report, "failure")["drafts"], 0, "{report}");
}
