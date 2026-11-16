//! Test double for `claude -p`. A test copies this binary into a temp directory
//! under the host's name (`claude`, or `claude.cmd` wrapping it on Windows) and
//! sets `FAKE_HOST_DIR` to a directory of its own. Each run reads all of stdin
//! into `stdin.<N>` and its argv, as a JSON array, into `args.<N>`, where N is
//! the number of `stdin.*` files already in that directory. Unless argv holds
//! `{"disableAllHooks": true}`, it runs every `hooks.UserPromptSubmit[].hooks[].command`
//! from `.claude/settings.json` and `.claude/settings.local.json` in its working
//! directory, through `cmd /C` on Windows and `sh -c` elsewhere, waiting for
//! each. Then it prints the file `reply` to stdout and exits 0. Hook output is
//! discarded, so stdout holds the reply and nothing else.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const HOOKS_OFF: &str = r#"{"disableAllHooks": true}"#;
const SETTINGS_FILES: [&str; 2] = [".claude/settings.json", ".claude/settings.local.json"];

fn next_call(dir: &Path) -> usize {
    fs::read_dir(dir)
        .expect("FAKE_HOST_DIR is readable")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("stdin."))
        .count()
}

fn user_prompt_commands(workdir: &Path) -> Vec<String> {
    SETTINGS_FILES
        .iter()
        .filter_map(|file| fs::read_to_string(workdir.join(file)).ok())
        .flat_map(|text| {
            let settings: Value = serde_json::from_str(&text).expect("settings are JSON");
            settings["hooks"]["UserPromptSubmit"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
                .filter_map(|hook| hook["command"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(windows)]
fn shell(command: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut shell = Command::new("cmd");
    shell.raw_arg(format!("/C {command}"));
    shell
}

#[cfg(not(windows))]
fn shell(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.args(["-c", command]);
    shell
}

fn run_hook(command: &str) {
    shell(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("hook command starts");
}

fn main() {
    let dir = PathBuf::from(env::var_os("FAKE_HOST_DIR").expect("FAKE_HOST_DIR is set"));
    let args: Vec<String> = env::args().skip(1).collect();
    let mut stdin = Vec::new();
    io::stdin()
        .read_to_end(&mut stdin)
        .expect("stdin is readable");
    let call = next_call(&dir);
    fs::write(dir.join(format!("stdin.{call}")), &stdin).expect("stdin file writes");
    let argv = serde_json::to_string(&args).expect("argv serialises");
    fs::write(dir.join(format!("args.{call}")), argv).expect("args file writes");
    if !args.iter().any(|arg| arg == HOOKS_OFF) {
        let workdir = env::current_dir().expect("working directory is readable");
        for command in user_prompt_commands(&workdir) {
            run_hook(&command);
        }
    }
    let reply = fs::read(dir.join("reply")).expect("reply file exists");
    io::stdout().write_all(&reply).expect("stdout writes");
}
