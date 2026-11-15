mod common;

use std::fs;
use std::path::PathBuf;

use common::{both, code, stderr, stdout, Repo};
use oncebitten::proof::{self, BackendKind, PROOF_FILE};
use oncebitten::replay::{LocalRunner, Verdict};
use oncebitten::rule::load_rule;

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
const BAD: &str = "import datetime\nstamp = datetime.datetime.utcnow()\n";
const OTHER_BAD: &str = "import datetime\nwhen = datetime.utcnow()\n";

/// Writes a proof for a project rule. The local runner stands in for the
/// sandbox here; tests/proof.rs covers the real OpenShell path.
fn prove(repo: &Repo, id: &str) {
    let rule = load_rule(&repo.rule_dir(id)).unwrap();
    let replayed = proof::replay_rule(
        &rule,
        &PathBuf::from(CHECKER),
        &LocalRunner,
        BackendKind::OpenShell,
    );
    assert_eq!(replayed.verdict, Verdict::Proven);
}

/// A repo on branch `main` with an active, proven py-utcnow rule and a
/// rules.lock, plus a `feature` branch checked out.
fn gated_repo() -> Repo {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");
    prove(&repo, "py-utcnow");
    repo.write("src/app.py", "x = 1\n");
    let sync = repo.run(&["rules", "sync"], &[]);
    assert_eq!(code(&sync), 0, "{}", both(&sync));
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo
}

fn pr(repo: &Repo) -> std::process::Output {
    repo.run(&["gate", "pr", "--base", "main"], &[])
}

fn write_baseline(repo: &Repo, entries: serde_json::Value) {
    let text = serde_json::to_string_pretty(&serde_json::json!({ "entries": entries })).unwrap();
    repo.write(".oncebitten/baseline.json", &text);
}

#[test]
fn pr_is_clean_without_violations() {
    let repo = gated_repo();
    repo.write("src/app.py", "x = 2\n");
    repo.commit_all("change");

    let output = pr(&repo);

    assert_eq!(code(&output), 0, "{}", both(&output));
}

#[test]
fn pr_fails_on_a_new_violation() {
    let repo = gated_repo();
    repo.write("src/app.py", BAD);
    repo.commit_all("bad");

    let output = pr(&repo);

    assert_eq!(code(&output), 1, "{}", both(&output));
    let expected = PathBuf::from("src").join("app.py");
    assert!(
        stdout(&output).contains("py-utcnow:")
            && stdout(&output).contains(&expected.display().to_string()),
        "{}",
        both(&output)
    );
}

#[test]
fn pr_ignores_violations_outside_the_changed_paths() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");
    prove(&repo, "py-utcnow");
    repo.write("src/old.py", BAD);
    repo.run(&["rules", "sync"], &[]);
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("src/new.py", "x = 1\n");
    repo.commit_all("clean change");

    let output = pr(&repo);

    assert_eq!(code(&output), 0, "{}", both(&output));
}

#[test]
fn pr_checks_only_committed_changes() {
    let repo = gated_repo();
    repo.write("src/untracked.py", BAD);

    let output = pr(&repo);

    assert_eq!(code(&output), 0, "{}", both(&output));
}

#[test]
fn pr_with_a_missing_proof_exits_2_naming_the_rule() {
    let repo = gated_repo();
    fs::remove_file(repo.rule_dir("py-utcnow").join(PROOF_FILE)).unwrap();
    repo.commit_all("drop proof");

    let output = pr(&repo);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(
        stderr(&output).contains("error: py-utcnow: no replay proof"),
        "{}",
        both(&output)
    );
}

#[test]
fn pr_with_a_stale_proof_exits_2() {
    let repo = gated_repo();
    repo.write(
        ".oncebitten/rules/py-utcnow/corpus/more.py",
        "import datetime\n",
    );
    repo.commit_all("edit rule");

    let output = pr(&repo);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(
        stderr(&output).contains("py-utcnow: stale replay proof"),
        "{}",
        both(&output)
    );
}

#[test]
fn proposed_rules_need_no_proof() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "proposed");
    repo.run(&["rules", "sync"], &[]);
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("src/app.py", BAD);
    repo.commit_all("bad");

    let output = pr(&repo);

    assert_eq!(code(&output), 0, "{}", both(&output));
}

#[test]
fn pr_needs_rules_lock() {
    let repo = gated_repo();
    fs::remove_file(repo.project().join(".oncebitten").join("rules.lock")).unwrap();
    repo.commit_all("drop lock");

    let output = pr(&repo);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(stderr(&output).contains("rules.lock"), "{}", both(&output));
}

#[test]
fn pr_needs_base() {
    let repo = gated_repo();

    let output = repo.run(&["gate", "pr"], &[]);

    assert_eq!(code(&output), 2, "{}", both(&output));
}

#[test]
fn pre_push_warns_on_a_missing_proof_and_continues() {
    let repo = gated_repo();
    fs::remove_file(repo.rule_dir("py-utcnow").join(PROOF_FILE)).unwrap();
    repo.write("src/app.py", BAD);
    repo.commit_all("bad");

    let output = repo.run(&["gate", "pre-push", "--base", "main"], &[]);

    assert_eq!(code(&output), 1, "{}", both(&output));
    assert!(
        stderr(&output).contains("warning: py-utcnow: no replay proof"),
        "{}",
        both(&output)
    );
}

#[test]
fn pre_push_warns_and_passes_when_clean() {
    let repo = gated_repo();
    fs::remove_file(repo.rule_dir("py-utcnow").join(PROOF_FILE)).unwrap();
    repo.commit_all("drop proof");

    let output = repo.run(&["gate", "pre-push", "--base", "main"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(stderr(&output).contains("warning:"), "{}", both(&output));
}

/// pre-commit sets these refs in the pre-push stage. The gate reads the
/// working tree, like pre-commit's own hooks, so the range picks the paths.
#[test]
fn pre_push_reads_the_pre_commit_refs() {
    let repo = gated_repo();
    let main = repo.git(&["rev-parse", "main"]);
    repo.write("src/app.py", BAD);
    let bad = repo.commit_all("bad");
    repo.write("src/other.py", "y = 1\n");
    let head = repo.commit_all("other");

    let pushed_bad = repo.run(
        &["gate", "pre-push"],
        &[("PRE_COMMIT_FROM_REF", &main), ("PRE_COMMIT_TO_REF", &bad)],
    );
    let pushed_other = repo.run(
        &["gate", "pre-push"],
        &[("PRE_COMMIT_FROM_REF", &bad), ("PRE_COMMIT_TO_REF", &head)],
    );

    assert_eq!(code(&pushed_bad), 1, "{}", both(&pushed_bad));
    assert_eq!(code(&pushed_other), 0, "{}", both(&pushed_other));
}

#[test]
fn pre_push_falls_back_to_the_upstream() {
    let repo = gated_repo();
    repo.git(&["branch", "-q", "--set-upstream-to", "main"]);
    repo.write("src/app.py", BAD);
    repo.commit_all("bad");

    let output = repo.run(&["gate", "pre-push"], &[]);

    assert_eq!(code(&output), 1, "{}", both(&output));
}

#[test]
fn pre_push_without_base_or_upstream_checks_all_tracked_files() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");
    prove(&repo, "py-utcnow");
    repo.write("src/old.py", BAD);
    repo.commit_all("base");

    let output = repo.run(&["gate", "pre-push"], &[]);

    assert_eq!(code(&output), 1, "{}", both(&output));
}

#[test]
fn baselined_violation_passes_and_a_new_one_fails() {
    let repo = gated_repo();
    repo.write("src/app.py", BAD);
    write_baseline(
        &repo,
        serde_json::json!([{
            "rule": "py-utcnow",
            "path": "src/app.py",
            "sha256": proof::text_sha256("datetime.datetime.utcnow()"),
        }]),
    );
    repo.commit_all("known");

    let known = pr(&repo);
    repo.write("src/app.py", &format!("{BAD}{OTHER_BAD}"));
    repo.commit_all("new");
    let new = pr(&repo);

    assert_eq!(code(&known), 0, "{}", both(&known));
    assert!(
        stderr(&known).contains("1 known violation"),
        "{}",
        both(&known)
    );
    assert_eq!(code(&new), 1, "{}", both(&new));
    assert!(stdout(&new).contains(":4:"), "{}", both(&new));
    assert!(!stdout(&new).contains(":2:"), "{}", both(&new));
}

#[test]
fn baseline_entry_is_tied_to_its_path() {
    let repo = gated_repo();
    repo.write("src/moved.py", BAD);
    write_baseline(
        &repo,
        serde_json::json!([{
            "rule": "py-utcnow",
            "path": "src/app.py",
            "sha256": proof::text_sha256("datetime.datetime.utcnow()"),
        }]),
    );
    repo.commit_all("elsewhere");

    let output = pr(&repo);

    assert_eq!(code(&output), 1, "{}", both(&output));
}

#[test]
fn unreadable_baseline_exits_2() {
    let repo = gated_repo();
    repo.write(".oncebitten/baseline.json", "not json");
    repo.commit_all("broken");

    let output = pr(&repo);

    assert_eq!(code(&output), 2, "{}", both(&output));
}

const NATIVE: &str = r#"id = "git-fails"
description = "fails while git rejects the flag"
mode = "block"
status = "active"
engine = "native"
command = ["git", "--no-such-flag"]
fired_exit_codes = [129]
"#;

#[test]
fn native_rule_is_baselined_by_rule_alone_and_needs_no_proof() {
    let repo = gated_repo();
    repo.add_rule("git-fails", NATIVE);
    repo.commit_all("native");
    let fired = pr(&repo);

    write_baseline(&repo, serde_json::json!([{ "rule": "git-fails" }]));
    repo.commit_all("baseline");
    let known = pr(&repo);

    assert_eq!(code(&fired), 1, "{}", both(&fired));
    assert!(!stderr(&fired).contains("proof"), "{}", both(&fired));
    assert_eq!(code(&known), 0, "{}", both(&known));
}

#[test]
fn diff_rule_violations_are_never_baselined() {
    let repo = gated_repo();
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
    repo.write("src/app.py", "x = 9\n");
    write_baseline(&repo, serde_json::json!([{ "rule": "diagram" }]));
    repo.commit_all("diff");

    let output = pr(&repo);

    assert_eq!(code(&output), 1, "{}", both(&output));
    assert!(stdout(&output).contains("diagram:"), "{}", both(&output));
}

#[test]
fn steer_rules_do_not_gate() {
    let repo = gated_repo();
    repo.add_rule("git-fails", &NATIVE.replace("block", "steer"));
    repo.commit_all("steer");

    let output = pr(&repo);

    assert_eq!(code(&output), 0, "{}", both(&output));
}

#[test]
fn rule_fixtures_do_not_trip_the_gate() {
    let repo = gated_repo();
    repo.add_rule("git-fails", &NATIVE.replace("block", "steer"));
    let fixtures = repo.rule_dir("git-fails").join("fixtures");
    fs::remove_file(fixtures.join("bad.diff")).unwrap();
    fs::write(fixtures.join("bad.py"), BAD).unwrap();
    repo.commit_all("add a rule with a bad fixture");

    let output = pr(&repo);

    assert_eq!(code(&output), 0, "{}", both(&output));
}

/// The pre-commit hook and every CI template must call commands that exist.
#[test]
fn hook_and_templates_call_real_gate_commands() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let hooks = fs::read_to_string(root.join(".pre-commit-hooks.yaml")).unwrap();
    let entry = hooks
        .lines()
        .find_map(|line| line.trim().strip_prefix("entry: "))
        .unwrap();
    let mut args: Vec<&str> = entry.split_whitespace().skip(1).collect();
    assert_eq!(args, ["gate", "pre-push"]);
    assert!(hooks.contains("stages: [pre-push]"));
    args.push("--help");
    let repo = Repo::new();
    let help = repo.run(&args, &[]);
    assert_eq!(code(&help), 0, "{}", both(&help));

    for template in ["github-actions.yml", "gitlab-ci.yml", "azure-pipelines.yml"] {
        let text = fs::read_to_string(root.join("templates").join(template)).unwrap();
        assert!(
            text.contains("oncebitten gate pr --base "),
            "{template} must run the pr gate"
        );
    }
    let help = repo.run(&["gate", "pr", "--help"], &[]);
    assert_eq!(code(&help), 0, "{}", both(&help));
}
