mod common;

use common::{both, code, stdout, Repo};
use oncebitten::proof::{self, Validity};
use oncebitten::rule::load_rule;

/// A repo on branch `main` with an active no-sudo steer rule and a rules.lock,
/// plus a `feature` branch checked out.
fn steer_repo(prove: bool) -> Repo {
    let repo = Repo::new();
    repo.add_example("no-sudo", "active");
    if prove {
        let output = repo.run(&["replay", "no-sudo"], &[]);
        assert_eq!(code(&output), 0, "{}", both(&output));
    }
    repo.write("src/app.py", "x = 1\n");
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo
}

#[test]
fn replay_proves_a_steer_rule_with_the_matcher() {
    let repo = steer_repo(true);
    let rule = load_rule(&repo.rule_dir("no-sudo")).unwrap();
    assert_eq!(proof::validity(&rule), Validity::Valid);
    let text = std::fs::read_to_string(repo.rule_dir("no-sudo").join("proof.json")).unwrap();
    assert!(text.contains("\"matcher\""), "{text}");
}

#[test]
fn replay_rejects_a_steer_rule_whose_fixed_fixture_triggers() {
    let repo = Repo::new();
    repo.add_example("no-sudo", "active");
    let fixtures = repo.rule_dir("no-sudo").join("fixtures");
    std::fs::copy(fixtures.join("bad.json"), fixtures.join("fixed.json")).unwrap();

    let output = repo.run(&["replay", "no-sudo"], &[]);

    assert_eq!(code(&output), 1, "{}", both(&output));
    assert!(stdout(&output).contains("triggered"), "{}", both(&output));
}

#[test]
fn gate_passes_with_a_proven_steer_rule() {
    let repo = steer_repo(true);
    repo.write("src/app.py", "x = 2\n");
    repo.commit_all("change");

    let output = repo.run(&["gate", "pr", "--base", "main"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
}

#[test]
fn gate_needs_a_proof_for_an_active_steer_rule() {
    let repo = steer_repo(false);
    repo.write("src/app.py", "x = 2\n");
    repo.commit_all("change");

    let output = repo.run(&["gate", "pr", "--base", "main"], &[]);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(
        both(&output).contains("no-sudo: no replay proof"),
        "{}",
        both(&output)
    );
}

#[test]
fn check_ignores_steer_rules() {
    let repo = steer_repo(false);
    repo.write("src/app.py", "x = 2\n");

    let output = repo.run(&["check"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(!both(&output).contains("no-sudo"), "{}", both(&output));
}

#[test]
fn rules_list_shows_steer_mode() {
    let repo = steer_repo(false);

    let output = repo.run(&["rules", "list"], &[]);

    assert!(
        stdout(&output).contains("no-sudo  steer  active"),
        "{}",
        both(&output)
    );
}

fn forge_proof(repo: &Repo, checker_sha256: Option<String>) {
    let dir = repo.rule_dir("no-sudo");
    let rule = load_rule(&dir).unwrap();
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_oncebitten"));
    let proof = proof::Proof {
        rule: rule.id.clone(),
        dir_sha256: proof::dir_hash(&dir).unwrap(),
        checker_version: "0.0.0".to_owned(),
        checker_sha256: checker_sha256.unwrap_or_else(|| proof::file_sha256(exe).unwrap()),
        backend: proof::MATCHER.to_owned(),
        timestamp: 0,
        result: proof::PROVEN.to_owned(),
    };
    proof::write(&dir, &proof).unwrap();
}

fn gate_pr_after_edit(repo: &Repo) -> std::process::Output {
    repo.write("src/app.py", "x = 2\n");
    repo.commit_all("change");
    repo.run(&["gate", "pr", "--base", "main"], &[])
}

#[test]
fn gate_replays_a_steer_rule_instead_of_trusting_its_proof() {
    let repo = Repo::new();
    repo.add_example("no-sudo", "active");
    repo.write(
        ".oncebitten/rules/no-sudo/fixtures/fixed.json",
        "{\"event\": \"tool\", \"tool\": \"Bash\", \"command\": \"sudo id\"}\n",
    );
    forge_proof(&repo, None);
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    assert_eq!(
        proof::validity(&load_rule(&repo.rule_dir("no-sudo")).unwrap()),
        Validity::Valid
    );

    let output = gate_pr_after_edit(&repo);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(both(&output).contains("triggered"), "{}", both(&output));
}

#[test]
fn gate_accepts_a_steer_proof_made_by_another_binary_when_replay_proves() {
    let repo = steer_repo(false);
    forge_proof(&repo, Some("0".repeat(64)));
    repo.commit_all("proof from another binary");

    let output = gate_pr_after_edit(&repo);

    assert_eq!(code(&output), 0, "{}", both(&output));
}
