use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

use super::{Finding, Outcome};

pub fn run(command: &[String], fired_exit_codes: &[i32], cwd: &Path) -> Outcome {
    run_matching(command, fired_exit_codes, None, cwd)
}

pub fn run_matching(
    command: &[String],
    fired_exit_codes: &[i32],
    fired_output_contains: Option<&str>,
    cwd: &Path,
) -> Outcome {
    let Some((program, args)) = command.split_first() else {
        return Outcome::CouldNotRun("native command is empty".to_owned());
    };
    let output = match Command::new(program).args(args).current_dir(cwd).output() {
        Ok(output) => output,
        Err(error) => return Outcome::CouldNotRun(format!("{program}: {error}")),
    };
    match output.status.code() {
        Some(0) => Outcome::Clean,
        Some(code) if fired_exit_codes.contains(&code) => match fired_output_contains {
            None => Outcome::Fired(vec![finding(message(program, code, &output))]),
            Some(needle) => {
                let matched = matching_lines(&output, needle);
                if matched.is_empty() {
                    Outcome::CouldNotRun(format!(
                        "{program} exited with {code} without output containing `{needle}`"
                    ))
                } else {
                    Outcome::Fired(vec![finding(matched.join("\n"))])
                }
            }
        },
        Some(code) => Outcome::CouldNotRun(format!("{program} exited with {code}")),
        None => Outcome::CouldNotRun(format!("{program} was terminated by a signal")),
    }
}

fn finding(message: String) -> Finding {
    Finding {
        path: None,
        line: None,
        message,
    }
}

fn matching_lines(output: &Output, needle: &str) -> Vec<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout
        .lines()
        .chain(stderr.lines())
        .filter(|line| line.contains(needle))
        .map(rendered)
        .collect()
}

fn rendered(line: &str) -> String {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| value["message"]["rendered"].as_str().map(str::to_owned))
        .unwrap_or_else(|| line.to_owned())
}

fn message(program: &str, code: i32, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let text = text.trim();
    if text.is_empty() {
        format!("{program} exited with {code}")
    } else {
        text.to_owned()
    }
}
