mod common;

use std::fs;
use std::path::PathBuf;

use common::{both, code, stderr, stdout, Repo};
use jiff::civil::date;
use oncebitten::check::Checked;
use oncebitten::engine::{Finding, Outcome};
use oncebitten::exceptions::{self, Exception};
use oncebitten::proof::{self, BackendKind};
use oncebitten::replay::{LocalRunner, Verdict};
use oncebitten::rule::load_rule;

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");
const BAD: &str = "import datetime\nstamp = datetime.datetime.utcnow()\n";
const PAST: &str = "2000-01-01";
const FUTURE: &str = "2999-12-31";

fn finding(path: Option<&str>) -> Finding {
    Finding {
        path: path.map(PathBuf::from),
        line: None,
        message: "fired".to_owned(),
        text: None,
    }
}

fn exception(paths: &[&str], expires: &str) -> Exception {
    let paths: Vec<String> = paths.iter().map(|p| (*p).to_owned()).collect();
    Exception::new("py-utcnow", &paths, "legacy code", expires).unwrap()
}

fn exceptions_file(rule: &str, paths: &str, expires: &str) -> String {
    format!(
        "[[exception]]\nrule = \"{rule}\"\npaths = {paths}\nreason = \"legacy code\"\nexpires = \"{expires}\"\n"
    )
}

fn check_repo() -> Repo {
    let repo = Repo::new();
    repo.add_example("py-utcnow", "active");
    repo.write("src/app.py", "x = 1\n");
    repo.commit_all("base");
    repo
}

fn gated_repo() -> Repo {
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
    repo.write("src/app.py", "x = 1\n");
    repo.commit_all("base");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo
}

#[test]
fn new_rejects_missing_fields() {
    let paths = vec!["src/**".to_owned()];
    for (rule, paths, reason, expires, wanted) in [
        ("", &paths, "r", FUTURE, "rule is empty"),
        ("py-utcnow", &Vec::new(), "r", FUTURE, "paths is empty"),
        (
            "py-utcnow",
            &vec![" ".to_owned()],
            "r",
            FUTURE,
            "empty glob",
        ),
        ("py-utcnow", &paths, "  ", FUTURE, "reason is empty"),
        (
            "py-utcnow",
            &paths,
            "r",
            "31/12/2999",
            "not a YYYY-MM-DD date",
        ),
        (
            "py-utcnow",
            &paths,
            "r",
            "2999-12-31T00:00",
            "not a YYYY-MM-DD date",
        ),
    ] {
        let error = Exception::new(rule, paths, reason, expires).unwrap_err();
        assert!(format!("{error:#}").contains(wanted), "{error:#}");
    }
}

#[test]
fn live_through_the_expiry_date() {
    let exception = exception(&["src/**"], "2026-06-30");

    assert!(exception.is_live(date(2026, 6, 30)));
    assert!(!exception.is_live(date(2026, 7, 1)));
}

#[test]
fn covers_matching_paths_of_its_rule_only() {
    let exception = exception(&["src/legacy/**", "tools/*.py"], FUTURE);

    assert!(exception.covers("py-utcnow", &finding(Some("src/legacy/a/b.py"))));
    assert!(exception.covers("py-utcnow", &finding(Some("tools/x.py"))));
    assert!(!exception.covers("py-utcnow", &finding(Some("tools/sub/x.py"))));
    assert!(!exception.covers("py-utcnow", &finding(Some("src/app.py"))));
    assert!(!exception.covers("other", &finding(Some("src/legacy/a.py"))));
}

#[test]
fn a_pathless_finding_needs_every_path() {
    assert!(!exception(&["src/**"], FUTURE).covers("py-utcnow", &finding(None)));
    assert!(exception(&["**"], FUTURE).covers("py-utcnow", &finding(None)));
}

#[test]
fn apply_removes_covered_findings_and_counts_them() {
    let checked = vec![
        Checked {
            id: "py-utcnow".to_owned(),
            outcome: Outcome::Fired(vec![
                finding(Some("src/legacy/a.py")),
                finding(Some("src/app.py")),
            ]),
        },
        Checked {
            id: "py-utcnow".to_owned(),
            outcome: Outcome::Fired(vec![finding(Some("src/legacy/b.py"))]),
        },
    ];

    let (checked, excepted) = exceptions::apply(
        checked,
        &[exception(&["src/legacy/**"], FUTURE)],
        date(2026, 1, 1),
    );

    assert_eq!(excepted, 2);
    assert_eq!(
        checked[0].outcome,
        Outcome::Fired(vec![finding(Some("src/app.py"))])
    );
    assert_eq!(checked[1].outcome, Outcome::Clean);
}

#[test]
fn apply_ignores_expired_exceptions() {
    let checked = vec![Checked {
        id: "py-utcnow".to_owned(),
        outcome: Outcome::Fired(vec![finding(Some("src/legacy/a.py"))]),
    }];

    let (checked, excepted) = exceptions::apply(
        checked,
        &[exception(&["src/legacy/**"], PAST)],
        date(2026, 1, 1),
    );

    assert_eq!(excepted, 0);
    assert!(matches!(checked[0].outcome, Outcome::Fired(_)));
}

#[test]
fn read_without_a_file_is_empty() {
    let repo = Repo::new();

    assert!(exceptions::read(&repo.project()).unwrap().is_empty());
}

#[test]
fn read_rejects_unknown_keys_and_missing_expiry() {
    let repo = Repo::new();
    for text in [
        format!(
            "{}scope = \"all\"\n",
            exceptions_file("py-utcnow", "[\"src/**\"]", FUTURE)
        ),
        "[[exception]]\nrule = \"py-utcnow\"\npaths = [\"src/**\"]\nreason = \"r\"\n".to_owned(),
        "[[exception]]\nrule = \"py-utcnow\"\nreason = \"r\"\nexpires = \"2999-12-31\"\n"
            .to_owned(),
    ] {
        repo.write(".oncebitten/exceptions.toml", &text);
        assert!(exceptions::read(&repo.project()).is_err(), "{text}");
    }
}

#[test]
fn check_skips_excepted_findings_and_reports_the_count() {
    let repo = check_repo();
    repo.write(
        ".oncebitten/exceptions.toml",
        &exceptions_file("py-utcnow", "[\"src/legacy/**\"]", FUTURE),
    );
    repo.write("src/legacy/old.py", BAD);

    let output = repo.run(&["check"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(
        stderr(&output).contains("1 violation(s) excepted by .oncebitten/exceptions.toml"),
        "{}",
        both(&output)
    );
}

#[test]
fn check_still_fires_outside_the_excepted_paths() {
    let repo = check_repo();
    repo.write(
        ".oncebitten/exceptions.toml",
        &exceptions_file("py-utcnow", "[\"src/legacy/**\"]", FUTURE),
    );
    repo.write("src/new.py", BAD);

    let output = repo.run(&["check"], &[]);

    assert_eq!(code(&output), 1, "{}", both(&output));
    assert!(stdout(&output).contains("src/new.py"), "{}", both(&output));
}

#[test]
fn check_with_an_invalid_file_could_not_run() {
    let repo = check_repo();
    repo.write(
        ".oncebitten/exceptions.toml",
        &exceptions_file("py-utcnow", "[\"src/**\"]", "soon"),
    );

    let output = repo.run(&["check"], &[]);

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(
        stderr(&output).contains("exceptions.toml"),
        "{}",
        both(&output)
    );
}

#[test]
fn gate_skips_excepted_findings() {
    let repo = gated_repo();
    repo.write(
        ".oncebitten/exceptions.toml",
        &exceptions_file("py-utcnow", "[\"src/legacy/**\"]", FUTURE),
    );
    repo.write("src/legacy/old.py", BAD);
    repo.commit_all("legacy");

    let output = repo.run(&["gate", "pr", "--base", "main"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(
        stderr(&output).contains("1 violation(s) excepted"),
        "{}",
        both(&output)
    );
}

#[test]
fn gate_applies_no_expired_exception_and_names_it() {
    let repo = gated_repo();
    repo.write(
        ".oncebitten/exceptions.toml",
        &exceptions_file("py-utcnow", "[\"src/legacy/**\"]", PAST),
    );
    repo.write("src/legacy/old.py", BAD);
    repo.commit_all("legacy");

    let output = repo.run(&["gate", "pr", "--base", "main"], &[]);

    assert_eq!(code(&output), 1, "{}", both(&output));
    assert!(
        stderr(&output).contains(
            "warning: expired exception: py-utcnow on src/legacy/** until 2000-01-01: legacy code"
        ),
        "{}",
        both(&output)
    );
}

#[test]
fn gate_with_an_invalid_file_could_not_run() {
    let repo = gated_repo();
    repo.write(".oncebitten/exceptions.toml", "[[exception]]\nrule = 1\n");
    repo.commit_all("broken");

    let output = repo.run(&["gate", "pr", "--base", "main"], &[]);

    assert_eq!(code(&output), 2, "{}", both(&output));
}

#[test]
fn baseline_update_leaves_out_excepted_findings() {
    let repo = gated_repo();
    repo.write(
        ".oncebitten/exceptions.toml",
        &exceptions_file("py-utcnow", "[\"src/legacy/**\"]", FUTURE),
    );
    repo.write("src/legacy/old.py", BAD);
    repo.write("src/other.py", BAD);
    repo.commit_all("violations");

    let output = repo.run(&["baseline", "update"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    let text = fs::read_to_string(repo.project().join(".oncebitten/baseline.json")).unwrap();
    assert!(text.contains("src/other.py"), "{text}");
    assert!(!text.contains("src/legacy/old.py"), "{text}");
}

#[test]
fn add_appends_an_entry_that_list_shows() {
    let repo = check_repo();
    repo.write(".oncebitten/exceptions.toml", "# reviewed in #12\n");

    let output = repo.run(
        &[
            "exception",
            "add",
            "py-utcnow",
            "--paths",
            "src/legacy/**",
            "tools/*.py",
            "--reason",
            "vendored until the rewrite",
            "--expires",
            FUTURE,
        ],
        &[],
    );
    assert_eq!(code(&output), 0, "{}", both(&output));

    let text = fs::read_to_string(repo.project().join(".oncebitten/exceptions.toml")).unwrap();
    assert!(text.starts_with("# reviewed in #12\n"), "{text}");
    let read = exceptions::read(&repo.project()).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].paths, ["src/legacy/**", "tools/*.py"]);

    let list = repo.run(&["exception", "list"], &[]);
    assert_eq!(code(&list), 0, "{}", both(&list));
    assert_eq!(
        stdout(&list),
        "py-utcnow  src/legacy/**,tools/*.py  2999-12-31  live  vendored until the rewrite\n"
    );
}

#[test]
fn list_shows_expired_exceptions() {
    let repo = check_repo();
    repo.write(
        ".oncebitten/exceptions.toml",
        &exceptions_file("py-utcnow", "[\"src/**\"]", PAST),
    );

    let output = repo.run(&["exception", "list"], &[]);

    assert_eq!(code(&output), 0, "{}", both(&output));
    assert!(
        stdout(&output).contains("2000-01-01  expired"),
        "{}",
        both(&output)
    );
}

#[test]
fn add_refuses_an_unknown_rule() {
    let repo = check_repo();

    let output = repo.run(
        &[
            "exception",
            "add",
            "no-such-rule",
            "--paths",
            "src/**",
            "--reason",
            "r",
            "--expires",
            FUTURE,
        ],
        &[],
    );

    assert_eq!(code(&output), 2, "{}", both(&output));
    assert!(!repo.project().join(".oncebitten/exceptions.toml").exists());
}

#[test]
fn add_refuses_an_empty_reason_or_missing_paths() {
    let repo = check_repo();
    for args in [
        vec!["--paths", "src/**", "--reason", " ", "--expires", FUTURE],
        vec!["--reason", "r", "--expires", FUTURE],
    ] {
        let mut full = vec!["exception", "add", "py-utcnow"];
        full.extend(args);

        let output = repo.run(&full, &[]);

        assert_ne!(code(&output), 0, "{}", both(&output));
        assert!(!repo.project().join(".oncebitten/exceptions.toml").exists());
    }
}

#[test]
fn add_twice_keeps_both_entries() {
    let repo = check_repo();
    for paths in ["src/a/**", "src/b/**"] {
        let output = repo.run(
            &[
                "exception",
                "add",
                "py-utcnow",
                "--paths",
                paths,
                "--reason",
                "r",
                "--expires",
                FUTURE,
            ],
            &[],
        );
        assert_eq!(code(&output), 0, "{}", both(&output));
    }

    let read = exceptions::read(&repo.project()).unwrap();

    assert_eq!(read.len(), 2);
    assert_eq!(read[1].paths, ["src/b/**"]);
}
