mod common;

use std::fs;
use std::path::PathBuf;

use common::{both, code, Repo};
use oncebitten::proof::{self, BackendKind};
use oncebitten::replay::{LocalRunner, Verdict};
use oncebitten::rule::load_rule;
use serde_json::{json, Value};

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
const CALL: &str = "datetime.datetime.utcnow()";
const NATIVE: &str = r#"id = "git-fails"
description = "fails while git rejects the flag"
mode = "block"
status = "active"
engine = "native"
command = ["git", "--no-such-flag"]
fired_exit_codes = [129]
"#;

fn baseline_path(repo: &Repo) -> PathBuf {
    repo.project().join(".oncebitten").join("baseline.json")
}

fn read_baseline(repo: &Repo) -> Value {
    serde_json::from_str(&fs::read_to_string(baseline_path(repo)).unwrap()).unwrap()
}

fn update(repo: &Repo) {
    let output = repo.run(&["baseline", "update"], &[]);
    assert_eq!(code(&output), 0, "{}", both(&output));
}

fn utcnow_repo() -> Repo {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");
    let rule = load_rule(&repo.rule_dir("py-utcnow")).unwrap();
    let replayed = proof::replay_rule(
        &rule,
        &PathBuf::from(CHECKER),
        &LocalRunner,
        BackendKind::OpenShell,
    );
    assert_eq!(replayed.verdict, Verdict::Proven);
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    repo
}

#[test]
fn update_records_each_distinct_match_sorted() {
    let repo = utcnow_repo();
    repo.write(
        "src/b.py",
        &format!("import datetime\nx = {CALL}\ny = {CALL}\n"),
    );
    repo.write("src/a.py", &format!("import datetime\nz = {CALL}\n"));
    repo.commit_all("violations");

    update(&repo);

    let hash = proof::text_sha256(CALL);
    assert_eq!(
        read_baseline(&repo),
        json!({ "entries": [
            { "rule": "py-utcnow", "path": "src/a.py", "sha256": hash },
            { "rule": "py-utcnow", "path": "src/b.py", "sha256": hash },
        ]})
    );
}

#[test]
fn update_is_stable() {
    let repo = utcnow_repo();
    repo.write("src/a.py", &format!("import datetime\nz = {CALL}\n"));
    repo.add_rule("git-fails", NATIVE);
    repo.commit_all("violations");

    update(&repo);
    let first = fs::read(baseline_path(&repo)).unwrap();
    update(&repo);

    assert_eq!(fs::read(baseline_path(&repo)).unwrap(), first);
    assert!(first.ends_with(b"\n"));
}

#[test]
fn updated_baseline_lets_the_gate_pass() {
    let repo = utcnow_repo();
    repo.write("src/a.py", &format!("import datetime\nz = {CALL}\n"));
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("src/a.py", &format!("import datetime\nw = 1\nz = {CALL}\n"));
    repo.commit_all("move it");
    let before = repo.run(&["gate", "pr", "--base", "main"], &[]);

    update(&repo);
    repo.commit_all("baseline");
    let after = repo.run(&["gate", "pr", "--base", "main"], &[]);

    assert_eq!(code(&before), 1, "{}", both(&before));
    assert_eq!(code(&after), 0, "{}", both(&after));
}

#[test]
fn update_drops_fixed_violations() {
    let repo = utcnow_repo();
    repo.write("src/a.py", &format!("import datetime\nz = {CALL}\n"));
    repo.commit_all("bad");
    update(&repo);
    repo.write("src/a.py", "z = 1\n");
    repo.commit_all("fixed");

    update(&repo);

    assert_eq!(read_baseline(&repo), json!({ "entries": [] }));
}

#[test]
fn update_records_native_rules_by_id_and_skips_diff_rules() {
    let repo = utcnow_repo();
    repo.add_rule("git-fails", NATIVE);
    repo.add_rule(
        "diagram",
        r#"id = "diagram"
description = "update the diagram with the API"
mode = "block"
status = "active"
engine = "diff"
when_changed = ["src/**"]
require_changed = ["docs/diagram.md"]
"#,
    );
    repo.commit_all("rules");

    update(&repo);

    assert_eq!(
        read_baseline(&repo),
        json!({ "entries": [{ "rule": "git-fails" }] })
    );
}

#[test]
fn update_skips_steer_and_proposed_rules() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "proposed");
    repo.add_rule("git-fails", &NATIVE.replace("block", "steer"));
    repo.write("src/a.py", &format!("import datetime\nz = {CALL}\n"));
    repo.commit_all("rules");

    update(&repo);

    assert_eq!(read_baseline(&repo), json!({ "entries": [] }));
}

#[test]
fn update_writes_nothing_when_a_rule_could_not_run() {
    let repo = utcnow_repo();
    repo.add_rule(
        "broken",
        &NATIVE
            .replace("git-fails", "broken")
            .replace("\"git\"", "\"oncebitten-no-such-program\""),
    );
    repo.commit_all("broken");

    let output = repo.run(&["baseline", "update"], &[]);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(both(&output).contains("broken"), "{}", both(&output));
    assert!(!baseline_path(&repo).exists());
}

fn local_baseline_path(repo: &Repo) -> PathBuf {
    repo.project()
        .join(".oncebitten")
        .join("baseline.local.json")
}

#[test]
fn update_splits_local_rule_entries_into_the_local_file() {
    let repo = utcnow_repo();
    repo.add_local_rule("git-fails", NATIVE);
    repo.write("src/a.py", &format!("import datetime\nz = {CALL}\n"));
    repo.commit_all("violations");

    update(&repo);

    let hash = proof::text_sha256(CALL);
    assert_eq!(
        read_baseline(&repo),
        json!({ "entries": [{ "rule": "py-utcnow", "path": "src/a.py", "sha256": hash }] })
    );
    let local: Value =
        serde_json::from_str(&fs::read_to_string(local_baseline_path(&repo)).unwrap()).unwrap();
    assert_eq!(local, json!({ "entries": [{ "rule": "local/git-fails" }] }));
}

#[test]
fn update_creates_no_local_file_without_local_entries() {
    let repo = utcnow_repo();
    repo.write("src/a.py", &format!("import datetime\nz = {CALL}\n"));
    repo.commit_all("violations");

    update(&repo);

    assert!(!local_baseline_path(&repo).exists());
}

#[test]
fn update_ignores_the_local_file_even_for_a_hand_made_local_rule() {
    let repo = utcnow_repo();
    let dir = ".oncebitten/rules.local/git-fails";
    repo.write(&format!("{dir}/rule.toml"), NATIVE);
    repo.write(&format!("{dir}/fixtures/bad.diff"), "bad\n");
    repo.write(&format!("{dir}/fixtures/fixed.diff"), "fixed\n");
    let gitignore = repo.project().join(".oncebitten").join(".gitignore");
    let _ = fs::remove_file(&gitignore);

    update(&repo);

    assert_eq!(
        fs::read_to_string(&gitignore).unwrap(),
        "rules.local/\nbaseline.local.json\n"
    );
    assert!(local_baseline_path(&repo).exists());
}
