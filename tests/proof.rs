mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{both, code, copy_dir, stdout, Repo};
use oncebitten::proof::{self, BackendKind, Proof, Validity, PROOF_FILE};
use oncebitten::replay::{Backend, LocalRunner, RawRun, ReplaySpec, Verdict};
use oncebitten::rule::load_rule;
use tempfile::TempDir;

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");

/// A temp copy of the shipped py-utcnow example: rule, fixtures and corpus.
fn utcnow_dir() -> (TempDir, PathBuf) {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("py-utcnow");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/py-utcnow"),
        &dir,
    );
    (root, dir)
}

fn checker() -> PathBuf {
    PathBuf::from(CHECKER)
}

struct Failing;

impl Backend for Failing {
    fn run(&self, _spec: &ReplaySpec, _files: &[PathBuf]) -> Result<Vec<RawRun>, String> {
        Err("gateway unreachable".to_owned())
    }
}

#[test]
fn hash_ignores_the_proof_file() {
    let (_root, dir) = utcnow_dir();
    let before = proof::dir_hash(&dir).unwrap();
    fs::write(dir.join(PROOF_FILE), "{}").unwrap();
    assert_eq!(proof::dir_hash(&dir).unwrap(), before);
}

#[test]
fn hash_counts_a_nested_proof_file() {
    let (_root, dir) = utcnow_dir();
    let before = proof::dir_hash(&dir).unwrap();
    fs::write(dir.join("corpus").join(PROOF_FILE), "{}").unwrap();
    assert_ne!(proof::dir_hash(&dir).unwrap(), before);
}

#[test]
fn hash_changes_with_content_and_with_renames() {
    let (_root, dir) = utcnow_dir();
    let before = proof::dir_hash(&dir).unwrap();
    fs::write(dir.join("corpus").join("extra.py"), "x = 1\n").unwrap();
    let added = proof::dir_hash(&dir).unwrap();
    assert_ne!(added, before);
    fs::rename(
        dir.join("corpus").join("extra.py"),
        dir.join("corpus").join("other.py"),
    )
    .unwrap();
    assert_ne!(proof::dir_hash(&dir).unwrap(), added);
}

#[test]
fn hash_reads_crlf_as_lf() {
    let root = TempDir::new().unwrap();
    let unix = root.path().join("unix");
    let windows = root.path().join("windows");
    fs::create_dir_all(&unix).unwrap();
    fs::create_dir_all(&windows).unwrap();
    fs::write(unix.join("a.py"), "x = 1\ny = 2\n").unwrap();
    fs::write(windows.join("a.py"), "x = 1\r\ny = 2\r\n").unwrap();
    assert_eq!(
        proof::dir_hash(&unix).unwrap(),
        proof::dir_hash(&windows).unwrap()
    );
}

#[test]
fn missing_proof_is_missing() {
    let (_root, dir) = utcnow_dir();
    let rule = load_rule(&dir).unwrap();
    assert_eq!(proof::validity(&rule), Validity::Missing);
}

#[test]
fn proven_openshell_replay_writes_a_valid_proof() {
    let (_root, dir) = utcnow_dir();
    let rule = load_rule(&dir).unwrap();

    let replayed = proof::replay_rule(&rule, &checker(), &LocalRunner, BackendKind::OpenShell);

    assert_eq!(replayed.verdict, Verdict::Proven);
    assert_eq!(replayed.written, Some(dir.join(PROOF_FILE)));
    let written = proof::read(&dir).unwrap().unwrap();
    assert_eq!(written.rule, "py-utcnow");
    assert_eq!(written.backend, "openshell");
    assert_eq!(written.result, "proven");
    assert_eq!(written.dir_sha256, proof::dir_hash(&dir).unwrap());
    assert_eq!(
        written.checker_sha256,
        proof::file_sha256(&checker()).unwrap()
    );
    assert_eq!(proof::validity(&rule), Validity::Valid);
}

#[test]
fn local_replay_reports_but_writes_no_proof() {
    let (_root, dir) = utcnow_dir();
    let rule = load_rule(&dir).unwrap();

    let replayed = proof::replay_rule(&rule, &checker(), &LocalRunner, BackendKind::Local);

    assert_eq!(replayed.verdict, Verdict::Proven);
    assert_eq!(replayed.written, None);
    assert!(!dir.join(PROOF_FILE).exists());
}

#[test]
fn edited_rule_makes_the_proof_stale() {
    let (_root, dir) = utcnow_dir();
    let rule = load_rule(&dir).unwrap();
    proof::replay_rule(&rule, &checker(), &LocalRunner, BackendKind::OpenShell);

    fs::write(dir.join("fixtures").join("fixed.py"), "stamp = 1\n").unwrap();

    assert!(
        matches!(proof::validity(&rule), Validity::Stale(_)),
        "{:?}",
        proof::validity(&rule)
    );
}

fn write_proof(dir: &Path, edit: impl FnOnce(&mut Proof)) {
    let mut proof = Proof {
        rule: "py-utcnow".to_owned(),
        dir_sha256: proof::dir_hash(dir).unwrap(),
        checker_version: "0.0.0".to_owned(),
        checker_sha256: "0".repeat(64),
        backend: "openshell".to_owned(),
        timestamp: 0,
        result: "proven".to_owned(),
    };
    edit(&mut proof);
    proof::write(dir, &proof).unwrap();
}

#[test]
fn local_backend_proof_is_not_proven() {
    let (_root, dir) = utcnow_dir();
    write_proof(&dir, |proof| proof.backend = "local".to_owned());
    let rule = load_rule(&dir).unwrap();
    assert!(matches!(proof::validity(&rule), Validity::NotProven(ref r) if r.contains("local")));
}

#[test]
fn proof_for_another_rule_is_stale() {
    let (_root, dir) = utcnow_dir();
    write_proof(&dir, |proof| proof.rule = "py-no-print".to_owned());
    let rule = load_rule(&dir).unwrap();
    assert!(matches!(proof::validity(&rule), Validity::Stale(ref r) if r.contains("py-no-print")));
}

#[test]
fn unreadable_proof_is_not_proven() {
    let (_root, dir) = utcnow_dir();
    fs::write(dir.join(PROOF_FILE), "not json").unwrap();
    let rule = load_rule(&dir).unwrap();
    assert!(
        matches!(proof::validity(&rule), Validity::NotProven(ref r) if r.contains("unreadable"))
    );
}

#[test]
fn rejected_replay_removes_the_old_proof() {
    let (_root, dir) = utcnow_dir();
    write_proof(&dir, |_| {});
    fs::write(
        dir.join("check.yml"),
        "id: never\nlanguage: python\nrule:\n  pattern: nothing_matches_this()\n",
    )
    .unwrap();
    let rule = load_rule(&dir).unwrap();

    let replayed = proof::replay_rule(&rule, &checker(), &LocalRunner, BackendKind::OpenShell);

    assert!(matches!(replayed.verdict, Verdict::Rejected(_)));
    assert!(!dir.join(PROOF_FILE).exists());
}

#[test]
fn backend_failure_keeps_no_new_proof() {
    let (_root, dir) = utcnow_dir();
    let rule = load_rule(&dir).unwrap();

    let replayed = proof::replay_rule(&rule, &checker(), &Failing, BackendKind::OpenShell);

    assert!(matches!(replayed.verdict, Verdict::CouldNotRun(ref r) if r.contains("gateway")));
    assert!(!dir.join(PROOF_FILE).exists());
}

#[test]
fn non_ast_grep_rule_could_not_run() {
    let (_root, dir) = utcnow_dir();
    let rule_dir = dir.parent().unwrap().join("no-tool");
    fs::create_dir_all(rule_dir.join("fixtures")).unwrap();
    fs::write(
        rule_dir.join("rule.toml"),
        "id = \"no-tool\"\ndescription = \"x\"\nmode = \"block\"\nstatus = \"active\"\n\
         engine = \"native\"\ncommand = [\"true\"]\n",
    )
    .unwrap();
    fs::write(rule_dir.join("fixtures").join("bad.txt"), "").unwrap();
    fs::write(rule_dir.join("fixtures").join("fixed.txt"), "").unwrap();
    let rule = load_rule(&rule_dir).unwrap();

    let replayed = proof::replay_rule(&rule, &checker(), &LocalRunner, BackendKind::OpenShell);

    assert!(matches!(replayed.verdict, Verdict::CouldNotRun(ref r) if r.contains("ast-grep")));
}

fn replay_repo(status: &str) -> Repo {
    let repo = Repo::new();
    repo.add_example("py-utcnow", status);
    repo.commit_all("rules");
    repo
}

#[test]
fn cli_local_replay_exits_0_and_writes_no_proof() {
    let repo = replay_repo("proposed");

    let output = repo.run(
        &["replay", "py-utcnow", "--local", "--checker", CHECKER],
        &[],
    );

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(
        stdout(&output).contains("no proof written"),
        "{}",
        both(&output)
    );
    assert!(!repo.rule_dir("py-utcnow").join(PROOF_FILE).exists());
}

#[test]
fn cli_rejected_replay_exits_1() {
    let repo = replay_repo("proposed");
    repo.write(
        ".oncebitten/rules/py-utcnow/check.yml",
        "id: never\nlanguage: python\nrule:\n  pattern: nothing_matches_this()\n",
    );

    let output = repo.run(
        &["replay", "py-utcnow", "--local", "--checker", CHECKER],
        &[],
    );

    assert_eq!(code(&output), 1, "{}", both(&output));
    assert!(
        stdout(&output).contains("py-utcnow: rejected"),
        "{}",
        both(&output)
    );
}

#[test]
fn cli_unknown_rule_exits_2() {
    let repo = replay_repo("proposed");

    let output = repo.run(
        &["replay", "no-such-rule", "--local", "--checker", CHECKER],
        &[],
    );

    assert_eq!(code(&output), 2, "{}", both(&output));
}

#[test]
fn cli_unreachable_gateway_exits_2_without_a_proof() {
    let repo = replay_repo("proposed");

    let output = repo.run(
        &[
            "replay",
            "py-utcnow",
            "--checker",
            CHECKER,
            "--gateway",
            "http://127.0.0.1:1",
        ],
        &[],
    );

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(!repo.rule_dir("py-utcnow").join(PROOF_FILE).exists());
}

/// Needs `ONCEBITTEN_OPENSHELL_GATEWAY`, `ONCEBITTEN_CHECKER` (static musl
/// build) and Docker, like tests/replay_openshell.rs.
#[test]
#[ignore = "needs a live OpenShell gateway, Docker and a musl checker build"]
fn cli_openshell_replay_writes_a_valid_proof() {
    let gateway =
        std::env::var("ONCEBITTEN_OPENSHELL_GATEWAY").expect("set ONCEBITTEN_OPENSHELL_GATEWAY");
    let checker = std::env::var("ONCEBITTEN_CHECKER").expect("set ONCEBITTEN_CHECKER");
    let repo = replay_repo("active");

    let output = repo.run(
        &[
            "replay",
            "py-utcnow",
            "--checker",
            &checker,
            "--gateway",
            &gateway,
        ],
        &[],
    );

    assert_eq!(code(&output), 0, "{}", both(&output));
    let rule = load_rule(&repo.rule_dir("py-utcnow")).unwrap();
    assert_eq!(proof::validity(&rule), Validity::Valid);
    let written = proof::read(&repo.rule_dir("py-utcnow")).unwrap().unwrap();
    assert_eq!(
        written.checker_sha256,
        proof::file_sha256(Path::new(&checker)).unwrap()
    );
}

fn edit_rule_toml(dir: &Path, from: &str, to: &str) {
    let path = dir.join("rule.toml");
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{text}");
    fs::write(&path, text.replacen(from, to, 1)).unwrap();
}

#[test]
fn approving_a_proposed_rule_keeps_its_proof_valid() {
    let (_root, dir) = utcnow_dir();
    let proposed = load_rule(&dir).unwrap();
    proof::replay_rule(&proposed, &checker(), &LocalRunner, BackendKind::OpenShell);
    assert_eq!(proof::validity(&proposed), Validity::Valid);

    edit_rule_toml(&dir, "status = \"proposed\"", "status = \"active\"");

    let approved = load_rule(&dir).unwrap();
    assert_eq!(proof::validity(&approved), Validity::Valid);
}

#[test]
fn changing_a_rule_field_other_than_status_makes_the_proof_stale() {
    let (_root, dir) = utcnow_dir();
    let rule = load_rule(&dir).unwrap();
    proof::replay_rule(&rule, &checker(), &LocalRunner, BackendKind::OpenShell);

    edit_rule_toml(
        &dir,
        "Use datetime.now(UTC)",
        "Use datetime.now(timezone.utc)",
    );

    let edited = load_rule(&dir).unwrap();
    assert!(
        matches!(proof::validity(&edited), Validity::Stale(_)),
        "{:?}",
        proof::validity(&edited)
    );
}

fn arch_dir() -> (TempDir, PathBuf) {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("arch-diagram");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/arch-diagram"),
        &dir,
    );
    (root, dir)
}

#[test]
fn diff_rule_proven_openshell_replay_writes_a_valid_proof() {
    let (_root, dir) = arch_dir();
    let rule = load_rule(&dir).unwrap();

    let replayed = proof::replay_rule(&rule, &checker(), &LocalRunner, BackendKind::OpenShell);

    assert_eq!(replayed.verdict, Verdict::Proven);
    assert!(replayed.written.is_some());
    assert_eq!(proof::validity(&rule), Validity::Valid);
}

#[test]
#[ignore = "needs a live OpenShell gateway, Docker and a musl checker build"]
fn cli_openshell_diff_replay_writes_a_valid_proof() {
    let gateway =
        std::env::var("ONCEBITTEN_OPENSHELL_GATEWAY").expect("set ONCEBITTEN_OPENSHELL_GATEWAY");
    let checker = std::env::var("ONCEBITTEN_CHECKER").expect("set ONCEBITTEN_CHECKER");
    let repo = Repo::new();
    repo.add_example("arch-diagram", "active");
    repo.commit_all("rules");

    let output = repo.run(
        &[
            "replay",
            "arch-diagram",
            "--checker",
            &checker,
            "--gateway",
            &gateway,
        ],
        &[],
    );

    assert_eq!(code(&output), 0, "{}", both(&output));
    let rule = load_rule(&repo.rule_dir("arch-diagram")).unwrap();
    assert_eq!(proof::validity(&rule), Validity::Valid);
}

#[test]
fn cli_replay_stale_proves_every_rule_needing_a_proof() {
    let repo = Repo::new();
    repo.add_example("no-sudo", "active");
    repo.add_example("py-utcnow", "proposed");
    repo.add_example("py-no-print", "retired");

    let output = repo.run(&["replay", "--stale", "--local", "--checker", CHECKER], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    let out = stdout(&output);
    assert!(out.contains("no-sudo: proven\n"), "{out}");
    assert!(out.contains("py-utcnow: proven\n"), "{out}");
    assert!(!out.contains("py-no-print"), "{out}");
    let rule = load_rule(&repo.rule_dir("no-sudo")).unwrap();
    assert_eq!(proof::validity(&rule), Validity::Valid);

    // The steer proof now stands; only the local-replayed rule is still stale.
    let again = repo.run(&["replay", "--stale", "--local", "--checker", CHECKER], &[]);
    let out = stdout(&again);
    assert!(!out.contains("no-sudo"), "{out}");
    assert!(out.contains("py-utcnow: proven"), "{out}");
}

#[test]
fn cli_replay_stale_exits_1_on_a_rejection_and_0_when_nothing_is_stale() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "proposed");
    repo.write(
        ".oncebitten/rules/py-utcnow/check.yml",
        "id: never\nlanguage: python\nrule:\n  pattern: nothing_matches_this()\n",
    );
    let output = repo.run(&["replay", "--stale", "--local", "--checker", CHECKER], &[]);
    assert_eq!(code(&output), 1, "{}", both(&output));
    assert!(
        stdout(&output).contains("py-utcnow: rejected: "),
        "{}",
        both(&output)
    );

    let empty = Repo::new();
    let output = empty.run(&["replay", "--stale", "--local"], &[]);
    assert_eq!(code(&output), 0, "{}", both(&output));
}

#[test]
fn cli_replay_stale_exits_2_when_a_rule_could_not_run() {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "proposed");
    let output = repo.run(
        &[
            "replay",
            "--stale",
            "--checker",
            CHECKER,
            "--gateway",
            "http://127.0.0.1:1",
        ],
        &[],
    );
    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(
        stdout(&output).contains("py-utcnow: could not run: "),
        "{}",
        both(&output)
    );
}

#[test]
fn validity_with_a_checker_compares_the_recorded_hash() {
    let (_root, dir) = utcnow_dir();
    let rule = load_rule(&dir).unwrap();
    proof::replay_rule(&rule, &checker(), &LocalRunner, BackendKind::OpenShell);
    let other = _root.path().join("other-checker");
    fs::write(&other, "not the checker").unwrap();

    assert_eq!(
        proof::validity_with(&rule, Some(&checker())),
        Validity::Valid
    );
    assert!(matches!(
        proof::validity_with(&rule, Some(&other)),
        Validity::Stale(ref reason) if reason.contains("checker")
    ));
    assert_eq!(proof::validity_with(&rule, None), Validity::Valid);
}

#[test]
fn cli_replay_stale_picks_up_a_local_rule_with_a_missing_proof() {
    let repo = Repo::new();
    repo.add_local_example("py-utcnow", "proposed");

    let output = repo.run(&["replay", "--stale", "--local", "--checker", CHECKER], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(
        stdout(&output).contains("py-utcnow: proven"),
        "{}",
        both(&output)
    );
}
