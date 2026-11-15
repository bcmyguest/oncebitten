use std::fs;
use std::path::Path;

use oncebitten::engine::{native, Outcome};
use tempfile::TempDir;

fn exit_with(code: i32, stderr: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![
            "cmd".to_owned(),
            "/C".to_owned(),
            format!("echo {stderr} 1>&2 & exit {code}"),
        ]
    } else {
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            format!("echo {stderr} >&2; exit {code}"),
        ]
    }
}

fn exit_if_marker_exists() -> Vec<String> {
    if cfg!(windows) {
        vec![
            "cmd".to_owned(),
            "/C".to_owned(),
            "if exist marker.txt (exit 0) else (exit 5)".to_owned(),
        ]
    } else {
        vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "test -f marker.txt || exit 5".to_owned(),
        ]
    }
}

fn run(command: &[String], fired: &[i32], cwd: &Path) -> Outcome {
    native::run(command, fired, cwd)
}

#[test]
fn zero_exit_is_clean() {
    let dir = TempDir::new().unwrap();

    assert_eq!(
        run(&exit_with(0, "ignored"), &[3], dir.path()),
        Outcome::Clean
    );
}

#[test]
fn listed_exit_code_fires_with_the_linter_output() {
    let dir = TempDir::new().unwrap();

    match run(&exit_with(3, "unwrap used"), &[3], dir.path()) {
        Outcome::Fired(findings) => {
            assert_eq!(findings.len(), 1);
            assert_eq!(findings[0].path, None);
            assert_eq!(findings[0].line, None);
            assert_eq!(findings[0].message, "unwrap used");
        }
        other => panic!("expected Fired, got {other:?}"),
    }
}

#[test]
fn unlisted_non_zero_exit_could_not_run() {
    let dir = TempDir::new().unwrap();

    assert!(matches!(
        run(&exit_with(4, "boom"), &[3], dir.path()),
        Outcome::CouldNotRun(_)
    ));
}

#[test]
fn non_zero_exit_with_no_listed_codes_could_not_run() {
    let dir = TempDir::new().unwrap();

    assert!(matches!(
        run(&exit_with(3, "boom"), &[], dir.path()),
        Outcome::CouldNotRun(_)
    ));
}

#[test]
fn missing_binary_could_not_run() {
    let dir = TempDir::new().unwrap();
    let command = vec!["oncebitten-no-such-linter".to_owned()];

    assert!(matches!(
        run(&command, &[1], dir.path()),
        Outcome::CouldNotRun(_)
    ));
}

#[test]
fn empty_command_could_not_run() {
    let dir = TempDir::new().unwrap();

    assert!(matches!(
        run(&[], &[1], dir.path()),
        Outcome::CouldNotRun(_)
    ));
}

#[test]
fn command_runs_in_the_given_directory() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("marker.txt"), "").unwrap();

    assert_eq!(
        run(&exit_if_marker_exists(), &[5], dir.path()),
        Outcome::Clean
    );
}

#[cfg(unix)]
#[test]
fn signal_termination_could_not_run() {
    let dir = TempDir::new().unwrap();
    let command = vec!["sh".to_owned(), "-c".to_owned(), "kill -9 $$".to_owned()];

    assert!(matches!(
        run(&command, &[1], dir.path()),
        Outcome::CouldNotRun(_)
    ));
}

fn run_matching(command: &[String], fired: &[i32], needle: &str, cwd: &Path) -> Outcome {
    native::run_matching(command, fired, Some(needle), cwd)
}

#[test]
fn listed_exit_code_without_the_expected_output_could_not_run() {
    let dir = TempDir::new().unwrap();

    assert!(matches!(
        run_matching(
            &exit_with(3, "compile error"),
            &[3],
            "unwrap used",
            dir.path()
        ),
        Outcome::CouldNotRun(_)
    ));
}

#[test]
fn listed_exit_code_with_the_expected_output_fires() {
    let dir = TempDir::new().unwrap();

    match run_matching(
        &exit_with(3, "unwrap used"),
        &[3],
        "unwrap used",
        dir.path(),
    ) {
        Outcome::Fired(findings) => assert_eq!(findings[0].message, "unwrap used"),
        other => panic!("expected Fired, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn matched_json_diagnostic_is_reported_as_its_rendered_text() {
    let dir = TempDir::new().unwrap();
    let json = r#"{"reason":"compiler-message","message":{"code":{"code":"x::y"},"rendered":"error: found it"}}"#;
    let command = vec![
        "sh".to_owned(),
        "-c".to_owned(),
        format!("printf '%s\\n' '{json}'; exit 3"),
    ];

    match run_matching(&command, &[3], r#""code":"x::y""#, dir.path()) {
        Outcome::Fired(findings) => assert_eq!(findings[0].message, "error: found it"),
        other => panic!("expected Fired, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn unlisted_exit_code_with_the_expected_output_could_not_run() {
    let dir = TempDir::new().unwrap();

    assert!(matches!(
        run_matching(
            &exit_with(4, "unwrap used"),
            &[3],
            "unwrap used",
            dir.path()
        ),
        Outcome::CouldNotRun(_)
    ));
}
