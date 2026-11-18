mod common;

use std::fs;
use std::path::PathBuf;

use common::{both, code, stderr, stdout, Repo};
use oncebitten::proof::{self, BackendKind};
use oncebitten::replay::{LocalRunner, Verdict};
use oncebitten::rule::{load_rule, Status};

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
const BAD: &str = "import datetime\nstamp = datetime.datetime.utcnow()\n";

fn rule_toml(repo: &Repo) -> String {
    fs::read_to_string(repo.rule_dir("py-utcnow").join("rule.toml")).unwrap()
}

fn prove(repo: &Repo) {
    let rule = load_rule(&repo.rule_dir("py-utcnow")).unwrap();
    let replayed = proof::replay_rule(
        &rule,
        &PathBuf::from(CHECKER),
        &LocalRunner,
        BackendKind::OpenShell,
    );
    assert_eq!(replayed.verdict, Verdict::Proven);
}

/// A proven, active rule on main and a feature branch that violates it.
fn violating_branch() -> Repo {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");
    prove(&repo);
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    repo.write("src/app.py", "x = 1\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("src/app.py", BAD);
    repo.commit_all("violation");
    repo
}

#[test]
fn retire_sets_status_and_reason_and_keeps_comments() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");
    let file = repo.rule_dir("py-utcnow").join("rule.toml");
    let before = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("# owned by the data team\n{before}")).unwrap();

    let output = repo.run(
        &[
            "rules",
            "retire",
            "py-utcnow",
            "--reason",
            "Python 3.12 removed it",
        ],
        &[],
    );

    assert_eq!(code(&output), 0, "{}", both(&output));
    let text = rule_toml(&repo);
    assert!(text.starts_with("# owned by the data team\n"), "{text}");
    assert!(text.contains("status = \"retired\""), "{text}");
    assert!(
        text.contains("retired_reason = \"Python 3.12 removed it\""),
        "{text}"
    );
    assert_eq!(
        load_rule(&repo.rule_dir("py-utcnow")).unwrap().status,
        Status::Retired
    );
}

#[test]
fn retire_needs_a_reason() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");

    let output = repo.run(&["rules", "retire", "py-utcnow", "--reason", " "], &[]);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(rule_toml(&repo).contains("status = \"active\""));
}

#[test]
fn retire_an_unknown_rule_fails() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");

    let output = repo.run(&["rules", "retire", "nope", "--reason", "r"], &[]);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(
        stderr(&output).contains("no rule named `nope`"),
        "{}",
        both(&output)
    );
}

#[test]
fn an_empty_retired_reason_is_invalid() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "retired");
    let file = repo.rule_dir("py-utcnow").join("rule.toml");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, format!("{text}retired_reason = \"\"\n")).unwrap();

    let error = load_rule(&repo.rule_dir("py-utcnow")).unwrap_err();

    assert!(
        format!("{error:#}").contains("retired_reason is empty"),
        "{error:#}"
    );
}

#[test]
fn a_retired_rule_never_runs_in_check() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");
    repo.write("src/app.py", "x = 1\n");
    repo.commit_all("base");
    repo.write("src/app.py", BAD);
    let retire = repo.run(&["rules", "retire", "py-utcnow", "--reason", "r"], &[]);
    assert_eq!(code(&retire), 0, "{}", both(&retire));

    let output = repo.run(&["check"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
}

#[test]
fn a_retired_rule_needs_no_proof_and_never_gates() {
    let repo = violating_branch();
    fs::remove_file(repo.rule_dir("py-utcnow").join("proof.json")).unwrap();
    let retire = repo.run(&["rules", "retire", "py-utcnow", "--reason", "r"], &[]);
    assert_eq!(code(&retire), 0, "{}", both(&retire));
    repo.commit_all("retire");

    let output = repo.run(&["gate", "pr", "--base", "main"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
}

#[test]
fn baseline_update_prunes_a_retired_rule() {
    let repo = violating_branch();
    let update = repo.run(&["baseline", "update"], &[]);
    assert_eq!(code(&update), 0, "{}", both(&update));
    let baseline = repo.project().join(".oncebitten/baseline.json");
    assert!(fs::read_to_string(&baseline).unwrap().contains("py-utcnow"));
    let retire = repo.run(&["rules", "retire", "py-utcnow", "--reason", "r"], &[]);
    assert_eq!(code(&retire), 0, "{}", both(&retire));

    let update = repo.run(&["baseline", "update"], &[]);

    assert_eq!(code(&update), 0, "{}", both(&update));
    let text = fs::read_to_string(&baseline).unwrap();
    assert!(!text.contains("py-utcnow"), "{text}");
}

#[test]
fn rollback_returns_the_rule_to_proposed_and_stops_it_blocking() {
    let repo = violating_branch();
    let gate = repo.run(&["gate", "pr", "--base", "main"], &[]);
    assert_eq!(code(&gate), 1, "{}", both(&gate));

    let output = repo.run(&["rules", "rollback", "py-utcnow"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(rule_toml(&repo).contains("status = \"proposed\""));
    assert!(repo.rule_dir("py-utcnow").join("proof.json").exists());
    assert!(
        stdout(&output).contains("git restore --source <commit> -- .oncebitten/rules/py-utcnow"),
        "{}",
        both(&output)
    );
    repo.commit_all("rollback");
    let gate = repo.run(&["gate", "pr", "--base", "main"], &[]);
    assert_eq!(code(&gate), 0, "{}", both(&gate));
}

#[test]
fn rollback_keeps_the_proof_valid_for_reapproval() {
    let repo = violating_branch();
    let rollback = repo.run(&["rules", "rollback", "py-utcnow"], &[]);
    assert_eq!(code(&rollback), 0, "{}", both(&rollback));
    repo.commit_all("rollback");
    let file = repo.rule_dir("py-utcnow").join("rule.toml");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        text.replace("status = \"proposed\"", "status = \"active\""),
    )
    .unwrap();
    repo.commit_all("reapprove");

    let gate = repo.run(&["gate", "pr", "--base", "main"], &[]);

    assert_eq!(code(&gate), 1, "{}", both(&gate));
    assert!(!stderr(&gate).contains("stale"), "{}", both(&gate));
}

#[test]
fn rollback_refuses_a_rule_that_is_not_active() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "proposed");

    let output = repo.run(&["rules", "rollback", "py-utcnow"], &[]);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(stderr(&output).contains("is proposed"), "{}", both(&output));
}

#[test]
fn retire_works_on_a_local_rule() {
    let repo = Repo::new();
    repo.add_local_example("py-utcnow", "active");

    let output = repo.run(
        &[
            "rules",
            "retire",
            "local/py-utcnow",
            "--reason",
            "not needed",
        ],
        &[],
    );

    assert_eq!(code(&output), 0, "{}", both(&output));
    let rule = load_rule(&repo.local_rule_dir("py-utcnow")).unwrap();
    assert_eq!(rule.status, Status::Retired);
}

#[test]
fn rollback_works_on_a_local_rule_and_prints_no_git_hints() {
    let repo = Repo::new();
    repo.add_local_example("py-utcnow", "active");

    let output = repo.run(&["rules", "rollback", "local/py-utcnow"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    let rule = load_rule(&repo.local_rule_dir("py-utcnow")).unwrap();
    assert_eq!(rule.status, Status::Proposed);
    assert!(!stdout(&output).contains("git "), "{}", both(&output));
    assert!(!stdout(&output).contains("restore"), "{}", both(&output));
}
