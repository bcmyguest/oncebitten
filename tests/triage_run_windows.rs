//! Windows-only: npm-installed hosts are `.cmd` wrappers run through cmd.exe.
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

use oncebitten::config::Host;
use oncebitten::triage::host::{invocation, Invocation, Reader};
use oncebitten::triage::run::{self, RunError, Runner};
use tempfile::TempDir;

const DUMPER: &str = r#"
use std::io::{Read, Write};
fn main() {
    let mut out = Vec::new();
    for arg in std::env::args().skip(1) {
        out.extend_from_slice(arg.as_bytes());
        out.push(0);
    }
    std::fs::File::create("args.bin").unwrap().write_all(&out).unwrap();
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    std::fs::File::create("stdin.bin").unwrap().write_all(&input).unwrap();
}
"#;

fn inv(program: &Path, args: Vec<String>) -> Invocation {
    Invocation {
        program: program.to_string_lossy().into_owned(),
        args,
        files: vec![],
        answer_file: None,
        stdin: None,
        streaming: false,
        reader: Reader::ClaudeJson,
    }
}

fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

fn build_dumper(dir: &Path) -> PathBuf {
    let src = dir.join("dumper.rs");
    let exe = dir.join("dumper.exe");
    fs::write(&src, DUMPER).unwrap();
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let status = Command::new(rustc)
        .arg(&src)
        .arg("-o")
        .arg(&exe)
        .status()
        .unwrap();
    assert!(status.success());
    exe
}

#[test]
fn a_timeout_kills_the_grandchild_behind_a_cmd_wrapper() {
    let dir = TempDir::new().unwrap();
    let pid_file = dir.path().join("pid.txt");
    let cmd = dir.path().join("fake-host.cmd");
    let script = format!(
        "@echo off\r\npowershell -NoProfile -Command \"$p = Start-Process ping -ArgumentList '-n','120','127.0.0.1' -PassThru -WindowStyle Hidden; $p.Id | Out-File -Encoding ascii '{}'; $p.WaitForExit()\"\r\n",
        pid_file.display()
    );
    fs::write(&cmd, script).unwrap();
    let work = TempDir::new().unwrap();
    let err = run::ProcessRunner
        .run(&inv(&cmd, vec![]), work.path(), Duration::from_secs(8))
        .unwrap_err();
    assert!(matches!(err, RunError::Timeout), "{err:?}");
    let pid: u32 = fs::read_to_string(&pid_file)
        .expect("fake host never recorded the grandchild pid")
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < deadline {
        sleep(Duration::from_millis(200));
    }
    let survived = alive(pid);
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
    assert!(!survived, "grandchild {pid} outlived the timeout");
}

/// The claude call, the one with the most quoted and empty arguments, run
/// through a `.cmd` wrapper: the arguments survive Rust's batch-file check and
/// cmd.exe, and the prompt reaches the host's stdin unchanged.
#[test]
fn a_realistic_call_reaches_a_cmd_wrapper_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    let dumper = build_dumper(dir.path());
    let cmd = dir.path().join("fake-host.cmd");
    fs::write(
        &cmd,
        format!("@echo off\r\n\"{}\" %*\r\n", dumper.display()),
    )
    .unwrap();
    let prompt = "Line one\nLine two with \"quotes\" and 100% ^caret^\n& | < > && ||\r\nlast";
    let work = TempDir::new().unwrap();
    let mut call = invocation(Host::Claude, "haiku", prompt, work.path()).unwrap();
    call.program = cmd.to_string_lossy().into_owned();
    run::ProcessRunner
        .run(&call, work.path(), Duration::from_secs(60))
        .expect("runner refused or failed the realistic call");
    let got = fs::read(work.path().join("args.bin")).unwrap();
    let expected: Vec<u8> = call
        .args
        .iter()
        .flat_map(|a| a.bytes().chain([0]))
        .collect();
    assert_eq!(
        String::from_utf8_lossy(&got),
        String::from_utf8_lossy(&expected)
    );
    let input = fs::read(work.path().join("stdin.bin")).unwrap();
    assert_eq!(String::from_utf8_lossy(&input), prompt);
}
