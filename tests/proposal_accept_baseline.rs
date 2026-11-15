mod common;

use std::fs;
use std::path::PathBuf;

use common::{both, code, Repo};
use oncebitten::proof::{self, BackendKind};
use oncebitten::proposals::State;
use oncebitten::replay::{LocalRunner, Verdict};
use oncebitten::rule::load_rule;
use oncebitten::store::{Kind, ProposalRow, Store};
use serde_json::Value;

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
const CALL: &str = "datetime.datetime.utcnow()";

fn baseline(repo: &Repo) -> Value {
    let path = repo.project().join(".oncebitten").join("baseline.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// A repo with a proposed `py-utcnow` rule that replays as proven, and one
/// committed file that already contains a hit.
fn proposed_repo() -> Repo {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "proposed");
    let rule = load_rule(&repo.rule_dir("py-utcnow")).unwrap();
    let replayed = proof::replay_rule(
        &rule,
        &PathBuf::from(CHECKER),
        &LocalRunner,
        BackendKind::OpenShell,
    );
    assert_eq!(replayed.verdict, Verdict::Proven);
    repo.write("src/old.py", &format!("import datetime\nx = {CALL}\n"));
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    let store = Store::open(&db(&repo)).unwrap();
    store
        .insert_proposal(&ProposalRow {
            cluster: "c1".into(),
            ts: 1,
            repo: repo.project().to_string_lossy().into_owned(),
            kind: Kind::Proposal,
            state: State::Waiting,
            rule: Some("py-utcnow".into()),
            dir: Some(repo.rule_dir("py-utcnow")),
            proven: true,
            note: None,
            verdict: "{}".into(),
            rationale: None,
            occurrences: vec![],
            target: None,
            commit: None,
            pr_url: None,
            hits: None,
        })
        .unwrap();
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo
}

fn db(repo: &Repo) -> PathBuf {
    repo.root
        .path()
        .join("data")
        .join("oncebitten")
        .join("events.db")
}

#[test]
fn accept_baselines_existing_hits_so_the_gate_passes_but_a_new_hit_fails() {
    let repo = proposed_repo();

    let other = serde_json::json!({
        "entries": [{"rule": "other-rule", "path": "src/x.py", "sha256": "abc"}]
    });
    fs::create_dir_all(repo.project().join(".oncebitten")).unwrap();
    fs::write(
        repo.project().join(".oncebitten").join("baseline.json"),
        other.to_string(),
    )
    .unwrap();

    let accept = repo.run(&["proposals", "accept", "c1"], &[]);
    assert_eq!(code(&accept), 0, "{}", both(&accept));

    let entries = baseline(&repo)["entries"].clone();
    assert_eq!(entries.as_array().map(Vec::len), Some(2), "{entries}");
    let ours = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["rule"] == "py-utcnow")
        .unwrap();
    assert_eq!(ours["path"], "src/old.py", "{entries}");
    assert!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["rule"] == "other-rule"),
        "{entries}"
    );

    // Touch the file that holds the old hit: the gate must pass.
    repo.write(
        "src/old.py",
        &format!("import datetime\n# note\nx = {CALL}\n"),
    );
    repo.commit_all("touch old hit");
    let touched = repo.run(&["gate", "pr", "--base", "main"], &[]);
    assert_eq!(code(&touched), 0, "{}", both(&touched));

    // A new hit in a new file: the gate must fail.
    repo.write("src/new.py", &format!("import datetime\ny = {CALL}\n"));
    repo.commit_all("new hit");
    let fresh = repo.run(&["gate", "pr", "--base", "main"], &[]);
    assert_eq!(code(&fresh), 1, "{}", both(&fresh));
}
