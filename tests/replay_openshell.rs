use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::replay::openshell::OpenShell;
use oncebitten::replay::{replay_dir, Verdict};

/// A rule directory with the py-utcnow rule, its fixtures and one corpus file.
fn rule_dir() -> tempfile::TempDir {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/py-utcnow");
    let dir = tempfile::tempdir().unwrap();
    fs::copy(src.join("check.yml"), dir.path().join("check.yml")).unwrap();
    fs::create_dir_all(dir.path().join("fixtures")).unwrap();
    fs::copy(
        src.join("fixtures/bad.py"),
        dir.path().join("fixtures/bad.py"),
    )
    .unwrap();
    fs::copy(
        src.join("fixtures/fixed.py"),
        dir.path().join("fixtures/fixed.py"),
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("corpus")).unwrap();
    fs::write(dir.path().join("corpus/ok.py"), "import os\n").unwrap();
    dir
}

#[test]
fn unreachable_gateway_is_could_not_run() {
    let dir = rule_dir();
    let checker = PathBuf::from(env!("CARGO_BIN_EXE_oncebitten-checker"));
    let backend = OpenShell::new(Some("http://127.0.0.1:1".to_owned()));

    let verdict = replay_dir(dir.path(), &checker, &backend);

    assert!(
        matches!(verdict, Verdict::CouldNotRun(_)),
        "expected CouldNotRun, got {verdict:?}"
    );
}

/// Runs the replay against a real OpenShell gateway. Needs:
/// - `ONCEBITTEN_OPENSHELL_GATEWAY`, the gateway endpoint (e.g. `http://127.0.0.1:17670`)
/// - `ONCEBITTEN_CHECKER`, the static musl build of `oncebitten-checker`
/// - Docker, to build the per-replay image
#[test]
#[ignore = "needs a live OpenShell gateway, Docker and a musl checker build"]
fn replay_proves_py_utcnow_in_a_real_sandbox() {
    let gateway =
        std::env::var("ONCEBITTEN_OPENSHELL_GATEWAY").expect("set ONCEBITTEN_OPENSHELL_GATEWAY");
    let checker =
        PathBuf::from(std::env::var("ONCEBITTEN_CHECKER").expect("set ONCEBITTEN_CHECKER"));
    let dir = rule_dir();

    let verdict = replay_dir(dir.path(), &checker, &OpenShell::new(Some(gateway)));

    assert_eq!(verdict, Verdict::Proven);
}

#[test]
#[ignore = "needs a live OpenShell gateway, Docker and a musl checker build"]
fn rule_that_never_fires_is_rejected_in_a_real_sandbox() {
    let gateway =
        std::env::var("ONCEBITTEN_OPENSHELL_GATEWAY").expect("set ONCEBITTEN_OPENSHELL_GATEWAY");
    let checker =
        PathBuf::from(std::env::var("ONCEBITTEN_CHECKER").expect("set ONCEBITTEN_CHECKER"));
    let dir = rule_dir();
    fs::write(
        dir.path().join("check.yml"),
        "id: never\nlanguage: python\nrule:\n  pattern: nothing_matches_this()\n",
    )
    .unwrap();

    let verdict = replay_dir(dir.path(), &checker, &OpenShell::new(Some(gateway)));

    assert!(
        matches!(verdict, Verdict::Rejected(ref r) if r.contains("bad.py did not fire")),
        "{verdict:?}"
    );
}

#[test]
#[ignore = "needs a live OpenShell gateway, Docker and a musl checker build"]
fn shipped_py_utcnow_example_is_proven_in_a_real_sandbox() {
    let gateway =
        std::env::var("ONCEBITTEN_OPENSHELL_GATEWAY").expect("set ONCEBITTEN_OPENSHELL_GATEWAY");
    let checker =
        PathBuf::from(std::env::var("ONCEBITTEN_CHECKER").expect("set ONCEBITTEN_CHECKER"));
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/py-utcnow");

    let verdict = replay_dir(&dir, &checker, &OpenShell::new(Some(gateway)));

    assert_eq!(verdict, Verdict::Proven);
}

/// Writes an executable POSIX shell script named `name` into `dir`.
#[cfg(unix)]
fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn refused_sandbox_create_reports_the_gateway_reason() {
    let dir = rule_dir();
    let bin = tempfile::tempdir().unwrap();
    let docker = stub(bin.path(), "docker", "exit 0");
    let openshell = stub(
        bin.path(),
        "openshell",
        "echo 'gateway refused: stub' >&2\nexit 3",
    );
    let checker = PathBuf::from(env!("CARGO_BIN_EXE_oncebitten-checker"));
    let mut backend = OpenShell::new(Some("http://127.0.0.1:1".to_owned()));
    backend.binary = openshell.display().to_string();
    backend.docker = docker.display().to_string();

    let verdict = replay_dir(dir.path(), &checker, &backend);

    match verdict {
        Verdict::CouldNotRun(reason) => {
            assert!(reason.contains("sandbox create failed"), "{reason}");
            assert!(reason.contains("gateway refused: stub"), "{reason}");
        }
        other => panic!("expected CouldNotRun, got {other:?}"),
    }
}
