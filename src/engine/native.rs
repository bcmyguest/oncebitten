use std::path::Path;
use std::process::{Command, Output};

use super::{Finding, Outcome};

pub fn run(command: &[String], fired_exit_codes: &[i32], cwd: &Path) -> Outcome {
    let Some((program, args)) = command.split_first() else {
        return Outcome::CouldNotRun("native command is empty".to_owned());
    };
    let output = match Command::new(program).args(args).current_dir(cwd).output() {
        Ok(output) => output,
        Err(error) => return Outcome::CouldNotRun(format!("{program}: {error}")),
    };
    match output.status.code() {
        Some(0) => Outcome::Clean,
        Some(code) if fired_exit_codes.contains(&code) => Outcome::Fired(vec![Finding {
            path: None,
            line: None,
            message: message(program, code, &output),
        }]),
        Some(code) => Outcome::CouldNotRun(format!("{program} exited with {code}")),
        None => Outcome::CouldNotRun(format!("{program} was terminated by a signal")),
    }
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
