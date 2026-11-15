//! The runner writes `Invocation::stdin` to the host and closes it.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use oncebitten::triage::host::{Invocation, Reader};
use oncebitten::triage::run::{self, Runner};
use tempfile::TempDir;

#[test]
fn the_prompt_reaches_the_host_on_stdin_byte_for_byte() {
    let dir = TempDir::new().unwrap();
    let host = dir.path().join("fake-host");
    fs::write(&host, "#!/bin/sh\ncat > stdin.bin\n").unwrap();
    fs::set_permissions(&host, fs::Permissions::from_mode(0o755)).unwrap();
    let prompt = "Line one\nLine two with \"quotes\" and 100% ^caret^\n& | < > && ||\r\nlast";
    let inv = Invocation {
        program: host.to_string_lossy().into_owned(),
        args: vec![],
        files: vec![],
        answer_file: None,
        stdin: Some(prompt.to_string()),
        streaming: false,
        reader: Reader::ClaudeJson,
    };
    let work = TempDir::new().unwrap();
    run::ProcessRunner
        .run(&inv, work.path(), Duration::from_secs(10))
        .expect("host did not see stdin close");
    assert_eq!(
        fs::read_to_string(work.path().join("stdin.bin")).unwrap(),
        prompt
    );
}
