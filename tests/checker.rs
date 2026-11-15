use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn example(path: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(path)
        .display()
        .to_string()
}

fn run_checker(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oncebitten-checker"))
        .args(args)
        .output()
        .unwrap()
}

fn one_json_line(output: &Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout: {stdout:?}");
    serde_json::from_str(lines[0]).unwrap()
}

#[test]
fn bad_fixture_is_fired_with_a_match_and_exit_0() {
    let bad = example("py-utcnow/fixtures/bad.py");
    let output = run_checker(&[&example("py-utcnow/check.yml"), &bad]);

    assert_eq!(output.status.code(), Some(0));
    let report = one_json_line(&output);
    assert_eq!(report["status"], "fired");
    assert_eq!(report["file"], bad);
    assert_eq!(report["matches"][0]["rule"], "py-utcnow");
    assert_eq!(report["matches"][0]["line"], 2);
}

#[test]
fn fixed_fixture_is_clean_with_exit_0() {
    let fixed = example("py-utcnow/fixtures/fixed.py");
    let output = run_checker(&[&example("py-utcnow/check.yml"), &fixed]);

    assert_eq!(output.status.code(), Some(0));
    let report = one_json_line(&output);
    assert_eq!(report["status"], "clean");
    assert_eq!(report["matches"], serde_json::json!([]));
}

#[test]
fn missing_input_is_an_error_with_exit_0() {
    let missing = example("py-utcnow/fixtures/nope.py");
    let output = run_checker(&[&example("py-utcnow/check.yml"), &missing]);

    assert_eq!(output.status.code(), Some(0));
    let report = one_json_line(&output);
    assert_eq!(report["status"], "error");
    assert!(report["reason"].as_str().unwrap().contains("input"));
}

#[test]
fn missing_rule_file_is_an_error_with_exit_0() {
    let output = run_checker(&[
        &example("py-utcnow/nope.yml"),
        &example("py-utcnow/fixtures/bad.py"),
    ]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(one_json_line(&output)["status"], "error");
}

#[test]
fn file_with_no_applicable_rule_is_an_error() {
    let output = run_checker(&[
        &example("py-utcnow/check.yml"),
        &example("rust-no-unwrap/fixtures/src/main.rs"),
    ]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(one_json_line(&output)["status"], "error");
}

#[test]
fn wrong_usage_is_an_error_with_exit_0() {
    let output = run_checker(&[]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(one_json_line(&output)["status"], "error");
}
