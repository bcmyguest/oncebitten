use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
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
    let output = match Command::new(spawn_path(program))
        .args(args)
        .current_dir(cwd)
        .output()
    {
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

/// Finds `program` in `dirs` by trying each extension in `pathext`, in order.
/// A name that already has an extension is tried as given first. An
/// extensionless name is never tried bare, so a POSIX shell script beside a
/// `.cmd` shim does not win on Windows.
pub fn resolve_in(program: &str, dirs: &[PathBuf], pathext: &[String]) -> Option<PathBuf> {
    let has_extension = Path::new(program).extension().is_some();
    for dir in dirs {
        if has_extension {
            let exact = dir.join(program);
            if exact.is_file() {
                return Some(exact);
            }
        }
        for ext in pathext {
            let candidate = dir.join(format!("{program}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The program to spawn. On Windows, PATHEXT resolves shims such as `npx.cmd`,
/// which `CreateProcess` does not find by bare name. Elsewhere the name passes through.
pub fn spawn_path(program: &str) -> OsString {
    if !cfg!(windows) {
        return OsString::from(program);
    }
    let dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect())
        .unwrap_or_default();
    let pathext: Vec<String> = match env::var("PATHEXT") {
        Ok(value) => value
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(_) => [".COM", ".EXE", ".BAT", ".CMD"].map(String::from).to_vec(),
    };
    resolve_in(program, &dirs, &pathext)
        .map_or_else(|| OsString::from(program), PathBuf::into_os_string)
}

fn finding(message: String) -> Finding {
    Finding {
        path: None,
        line: None,
        message,
        text: None,
    }
}

fn matching_lines(output: &Output, needle: &str) -> Vec<String> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    stdout
        .lines()
        .chain(stderr.lines())
        .filter(|line| line.contains(needle))
        .map(str::trim_end)
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
