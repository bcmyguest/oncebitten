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
    proposed_repo_in(false)
}

/// `proposed_repo` with the rule in the gitignored local layer when `local`.
fn proposed_repo_in(local: bool) -> Repo {
    let repo = Repo::new();
    let dir = if local {
        repo.add_local_example("py-utcnow", "proposed")
    } else {
        repo.add_example("py-utcnow", "proposed")
    };
    let rule = load_rule(&dir).unwrap();
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
            dir: Some(dir),
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

#[test]
fn accept_from_another_repo_baselines_the_proposals_own_repo() {
    let repo = proposed_repo();
    let elsewhere = repo.root.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&elsewhere)
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");

    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_oncebitten"));
    command
        .args(["proposals", "accept", "c1"])
        .current_dir(&elsewhere)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", repo.root.path().join("home"))
        .env("XDG_CONFIG_HOME", repo.root.path().join("xdg-config"))
        .env("XDG_DATA_HOME", repo.root.path().join("data"))
        .env(
            "GIT_CONFIG_GLOBAL",
            repo.root.path().join("empty.gitconfig"),
        );
    for var in ["SYSTEMROOT", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(var) {
            command.env(var, value);
        }
    }
    let accept = command.output().unwrap();
    assert_eq!(code(&accept), 0, "{}", both(&accept));

    assert!(
        !elsewhere.join(".oncebitten").join("baseline.json").exists(),
        "baseline written into the cwd repo"
    );
    let entries = baseline(&repo)["entries"].clone();
    assert!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["rule"] == "py-utcnow" && entry["path"] == "src/old.py"),
        "{entries}"
    );
}

#[test]
fn accept_baselines_a_local_rule_into_the_local_file() {
    let repo = proposed_repo_in(true);
    let oncebitten = repo.project().join(".oncebitten");

    let accept = repo.run(&["proposals", "accept", "c1"], &[]);
    assert_eq!(code(&accept), 0, "{}", both(&accept));

    assert!(!oncebitten.join("baseline.json").exists());
    let text = fs::read_to_string(oncebitten.join("baseline.local.json")).unwrap();
    let local: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(local["entries"].as_array().map(Vec::len), Some(1), "{text}");
    assert_eq!(local["entries"][0]["rule"], "local/py-utcnow", "{text}");
    assert_eq!(local["entries"][0]["path"], "src/old.py", "{text}");
    assert!(
        stdout_of(&accept).contains("baseline.local.json"),
        "{}",
        both(&accept)
    );

    repo.write(
        "src/old.py",
        &format!("import datetime\n# note\nx = {CALL}\n"),
    );
    repo.commit_all("touch old hit");
    let push = repo.run(&["gate", "pre-push", "--base", "main"], &[]);
    assert_eq!(code(&push), 0, "{}", both(&push));
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}
