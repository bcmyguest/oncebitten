use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use oncebitten::triage::host::{Captured, Invocation, Reader};
use oncebitten::triage::run::{self, RunError, Runner};
use tempfile::TempDir;

/// A runner that never starts a process: it returns canned output.
struct Canned(Captured);

impl Runner for Canned {
    fn run(
        &self,
        _inv: &Invocation,
        _dir: &Path,
        _timeout: Duration,
    ) -> Result<Captured, RunError> {
        Ok(self.0.clone())
    }
}

#[test]
fn a_runner_is_a_seam_for_tests() {
    let runner = Canned(Captured {
        stdout: "hi".into(),
        answer: None,
    });
    let inv = Invocation {
        program: "unused".into(),
        args: vec![],
        files: vec![],
        answer_file: None,
        stdin_null: true,
        streaming: false,
        reader: Reader::CodexFile,
    };
    let got = runner
        .run(&inv, Path::new("."), Duration::from_secs(1))
        .unwrap();
    assert_eq!(got.stdout, "hi");
}

#[test]
fn a_missing_binary_is_reported_as_missing() {
    let dir = TempDir::new().unwrap();
    let inv = Invocation {
        program: "oncebitten-no-such-host-binary".into(),
        args: vec![],
        files: vec![],
        answer_file: None,
        stdin_null: true,
        streaming: false,
        reader: Reader::CodexFile,
    };
    let err = run::ProcessRunner
        .run(&inv, dir.path(), Duration::from_secs(5))
        .unwrap_err();
    assert!(matches!(err, RunError::Missing(_)), "{err:?}");
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// Writes a `#!/bin/sh` script into its own directory and returns its path.
    fn script(body: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("fake-host");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        (dir, path)
    }

    fn inv(program: PathBuf, streaming: bool) -> Invocation {
        Invocation {
            program: program.to_string_lossy().into_owned(),
            args: vec![],
            files: vec![],
            answer_file: None,
            stdin_null: true,
            streaming,
            reader: Reader::ClaudeJson,
        }
    }

    #[test]
    fn the_child_runs_in_the_workdir_with_the_triage_switch_set() {
        let (_keep, path) = script(r#"printf '%s|%s' "$PWD" "$ONCEBITTEN_TRIAGE""#);
        let work = TempDir::new().unwrap();
        let out = run::ProcessRunner
            .run(&inv(path, false), work.path(), Duration::from_secs(10))
            .unwrap();
        let expected = work.path().canonicalize().unwrap();
        assert_eq!(
            out.stdout,
            format!("{}|1", expected.display()),
            "stdout was {}",
            out.stdout
        );
    }

    #[test]
    fn a_hung_host_is_killed_at_the_timeout() {
        let (_keep, path) = script("sleep 30");
        let work = TempDir::new().unwrap();
        let started = Instant::now();
        let err = run::ProcessRunner
            .run(&inv(path, false), work.path(), Duration::from_secs(1))
            .unwrap_err();
        assert!(matches!(err, RunError::Timeout), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_streaming_host_is_killed_once_its_verdict_is_complete() {
        let verdict = r#"{"type":"text","part":{"type":"text","text":"```json\n{\"repeatable\":true,\"user_correction\":false,\"root_cause\":\"x\",\"proposed_fix_kind\":\"lint\",\"generality\":\"general\"}\n```"}}"#;
        let (_keep, path) = script(&format!("printf '%s\\n' '{verdict}'\nsleep 30"));
        let work = TempDir::new().unwrap();
        let mut inv = inv(path, true);
        inv.reader = Reader::OpenCodeJsonl;
        let started = Instant::now();
        let out = run::ProcessRunner
            .run(&inv, work.path(), Duration::from_secs(20))
            .unwrap();
        assert!(out.stdout.contains("root_cause"), "{}", out.stdout);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
