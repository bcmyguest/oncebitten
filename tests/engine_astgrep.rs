use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::engine::{astgrep, Outcome};
use tempfile::TempDir;

const PY_CHECK: &str = "id: py-utcnow\nlanguage: python\nmessage: use datetime.now(UTC)\n\
                        severity: error\nrule:\n  pattern: datetime.utcnow()\n";

const LANGUAGES: &[(&str, &str, &str, &str)] = &[
    ("python", "py", "print($X)", "print(1)\n"),
    ("rust", "rs", "dbg!($X)", "fn main() { dbg!(1); }\n"),
    ("javascript", "js", "console.log($X)", "console.log(1);\n"),
    (
        "typescript",
        "ts",
        "console.log($X)",
        "const n: number = 1;\nconsole.log(n);\n",
    ),
];

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path
}

fn check_for(language: &str, pattern: &str) -> String {
    format!("id: probe\nlanguage: {language}\nmessage: probe\nrule:\n  pattern: {pattern}\n")
}

#[test]
fn python_match_fires_with_path_line_and_message() {
    let dir = TempDir::new().unwrap();
    let check = write(dir.path(), "check.yml", PY_CHECK);
    let src = write(
        dir.path(),
        "a.py",
        "import datetime\n\nnow = datetime.utcnow()\n",
    );

    match astgrep::run(&check, std::slice::from_ref(&src)) {
        Outcome::Fired(findings) => {
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].path.as_deref(), Some(src.as_path()));
            assert_eq!(findings[0].line, Some(3));
            assert_eq!(findings[0].message, "use datetime.now(UTC)");
        }
        other => panic!("expected Fired, got {other:?}"),
    }
}

#[test]
fn file_without_a_match_is_clean() {
    let dir = TempDir::new().unwrap();
    let check = write(dir.path(), "check.yml", PY_CHECK);
    let src = write(dir.path(), "a.py", "now = datetime.now(UTC)\n");

    assert_eq!(astgrep::run(&check, &[src]), Outcome::Clean);
}

#[test]
fn each_supported_language_fires_on_its_own_files() {
    for (language, ext, pattern, source) in LANGUAGES {
        let dir = TempDir::new().unwrap();
        let check = write(dir.path(), "check.yml", &check_for(language, pattern));
        let src = write(dir.path(), &format!("a.{ext}"), source);

        assert!(
            matches!(astgrep::run(&check, &[src]), Outcome::Fired(_)),
            "{language} did not fire"
        );
    }
}

#[test]
fn files_in_another_language_are_skipped() {
    let dir = TempDir::new().unwrap();
    let check = write(dir.path(), "check.yml", PY_CHECK);
    let src = write(dir.path(), "a.rs", "fn main() { datetime.utcnow(); }\n");

    assert_eq!(astgrep::run(&check, &[src]), Outcome::Clean);
}

#[test]
fn malformed_check_file_could_not_run() {
    let dir = TempDir::new().unwrap();
    let check = write(dir.path(), "check.yml", "id: broken\nrule: [\n");
    let src = write(dir.path(), "a.py", "print(1)\n");

    assert!(matches!(
        astgrep::run(&check, &[src]),
        Outcome::CouldNotRun(_)
    ));
}

#[test]
fn language_without_a_grammar_could_not_run() {
    let dir = TempDir::new().unwrap();
    let check = write(dir.path(), "check.yml", &check_for("cobol", "x"));
    let src = write(dir.path(), "a.py", "print(1)\n");

    assert!(matches!(
        astgrep::run(&check, &[src]),
        Outcome::CouldNotRun(_)
    ));
}

#[test]
fn check_file_with_no_rules_could_not_run() {
    let dir = TempDir::new().unwrap();
    let check = write(dir.path(), "check.yml", "");
    let src = write(dir.path(), "a.py", "print(1)\n");

    assert!(matches!(
        astgrep::run(&check, &[src]),
        Outcome::CouldNotRun(_)
    ));
}

#[test]
fn missing_check_file_could_not_run() {
    let dir = TempDir::new().unwrap();
    let src = write(dir.path(), "a.py", "print(1)\n");

    assert!(matches!(
        astgrep::run(&dir.path().join("check.yml"), &[src]),
        Outcome::CouldNotRun(_)
    ));
}

#[test]
fn unreadable_source_file_could_not_run() {
    let dir = TempDir::new().unwrap();
    let check = write(dir.path(), "check.yml", PY_CHECK);

    assert!(matches!(
        astgrep::run(&check, &[dir.path().join("gone.py")]),
        Outcome::CouldNotRun(_)
    ));
}
