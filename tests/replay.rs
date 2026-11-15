use std::fs;
use std::path::{Path, PathBuf};

use oncebitten::replay::{replay_dir, Backend, LocalRunner, RawRun, ReplaySpec, Verdict};

const CHECKER: &str = env!("CARGO_BIN_EXE_oncebitten-checker");

fn example(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(path)
}

/// A rule directory laid out like a shipped example, with `corpus/` added.
fn rule_dir(corpus: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let src = example("py-utcnow");
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
    for (name, body) in corpus {
        fs::write(dir.path().join("corpus").join(name), body).unwrap();
    }
    dir
}

fn checker() -> PathBuf {
    PathBuf::from(CHECKER)
}

/// Returns canned runs, one per file, and never touches a checker.
struct Canned(Result<Vec<RawRun>, String>);

impl Backend for Canned {
    fn run(&self, _spec: &ReplaySpec, files: &[PathBuf]) -> Result<Vec<RawRun>, String> {
        match &self.0 {
            Ok(runs) => {
                assert_eq!(runs.len(), files.len());
                Ok(runs.clone())
            }
            Err(reason) => Err(reason.clone()),
        }
    }
}

// Build JSON with serde_json: Windows paths contain backslashes.
fn fired_json(target: &str) -> String {
    let value = serde_json::json!({
        "status": "fired",
        "file": target,
        "matches": [{"rule": "py-utcnow", "line": 2, "text": "x"}],
    });
    format!("{value}\n")
}

fn clean_json(target: &str) -> String {
    let value = serde_json::json!({"status": "clean", "file": target, "matches": []});
    format!("{value}\n")
}

fn run(target: &str, exit_code: Option<i32>, stdout: String) -> RawRun {
    RawRun {
        target: target.to_owned(),
        exit_code,
        stdout,
        stderr: String::new(),
    }
}

#[test]
fn bad_fires_fixed_and_corpus_pass_is_proven_with_the_real_checker() {
    let dir = rule_dir(&[("ok.py", "import os\n")]);
    let verdict = replay_dir(dir.path(), &checker(), &LocalRunner);
    assert_eq!(verdict, Verdict::Proven);
}

#[test]
fn rule_that_never_fires_on_bad_is_rejected() {
    let dir = rule_dir(&[("ok.py", "import os\n")]);
    fs::write(
        dir.path().join("check.yml"),
        "id: never\nlanguage: python\nrule:\n  pattern: nothing_matches_this()\n",
    )
    .unwrap();
    let verdict = replay_dir(dir.path(), &checker(), &LocalRunner);
    let Verdict::Rejected(reason) = verdict else {
        panic!("expected Rejected, got {verdict:?}");
    };
    assert!(reason.contains("bad.py did not fire"), "{reason}");
}

#[test]
fn corpus_file_that_fires_is_rejected() {
    let dir = rule_dir(&[(
        "noisy.py",
        "import datetime\nx = datetime.datetime.utcnow()\n",
    )]);
    let verdict = replay_dir(dir.path(), &checker(), &LocalRunner);
    let Verdict::Rejected(reason) = verdict else {
        panic!("expected Rejected, got {verdict:?}");
    };
    assert!(reason.contains("noisy.py fired"), "{reason}");
}

#[test]
fn missing_rule_file_is_could_not_run() {
    let dir = rule_dir(&[("ok.py", "x\n")]);
    fs::remove_file(dir.path().join("check.yml")).unwrap();
    let verdict = replay_dir(dir.path(), &checker(), &LocalRunner);
    let Verdict::CouldNotRun(reason) = verdict else {
        panic!("expected CouldNotRun, got {verdict:?}");
    };
    assert!(reason.contains("missing input"), "{reason}");
}

#[test]
fn missing_bad_fixture_is_could_not_run() {
    let dir = rule_dir(&[("ok.py", "x\n")]);
    fs::remove_file(dir.path().join("fixtures/bad.py")).unwrap();
    let verdict = replay_dir(dir.path(), &checker(), &LocalRunner);
    assert!(matches!(verdict, Verdict::CouldNotRun(ref r) if r.contains("bad.")));
}

#[test]
fn empty_corpus_is_could_not_run() {
    let dir = rule_dir(&[]);
    let verdict = replay_dir(dir.path(), &checker(), &LocalRunner);
    assert!(matches!(verdict, Verdict::CouldNotRun(ref r) if r.contains("missing input")));
}

#[test]
fn missing_checker_is_could_not_run() {
    let dir = rule_dir(&[("ok.py", "x\n")]);
    let missing = dir.path().join("no-such-checker");
    let verdict = replay_dir(dir.path(), &missing, &LocalRunner);
    let Verdict::CouldNotRun(reason) = verdict else {
        panic!("expected CouldNotRun, got {verdict:?}");
    };
    assert!(reason.contains("missing checker"), "{reason}");
}

#[test]
fn backend_failure_is_could_not_run() {
    let dir = rule_dir(&[("ok.py", "x\n")]);
    let spec = ReplaySpec::from_rule_dir(dir.path(), &checker()).unwrap();
    let backend = Canned(Err("sandbox create failed: gateway unreachable".to_owned()));
    let verdict = oncebitten::replay::replay(&spec, &backend);
    assert_eq!(
        verdict,
        Verdict::CouldNotRun("sandbox create failed: gateway unreachable".to_owned())
    );
}

#[test]
fn fired_json_with_nonzero_exit_is_not_fired() {
    let dir = rule_dir(&[("ok.py", "x\n")]);
    let spec = ReplaySpec::from_rule_dir(dir.path(), &checker()).unwrap();
    let bad = spec.bad.display().to_string();
    let runs = vec![
        run(&bad, Some(2), fired_json(&bad)),
        run(&spec.fixed.display().to_string(), Some(0), clean_json("x")),
        run(
            &spec.corpus[0].display().to_string(),
            Some(0),
            clean_json("x"),
        ),
    ];
    let verdict = oncebitten::replay::replay(&spec, &Canned(Ok(runs)));
    assert!(matches!(verdict, Verdict::CouldNotRun(ref r) if r.contains("exited with")));
}

#[test]
fn json_for_another_file_is_not_accepted() {
    let dir = rule_dir(&[("ok.py", "x\n")]);
    let spec = ReplaySpec::from_rule_dir(dir.path(), &checker()).unwrap();
    let bad = spec.bad.display().to_string();
    let fixed = spec.fixed.display().to_string();
    let corpus = spec.corpus[0].display().to_string();
    let runs = vec![
        run(&bad, Some(0), fired_json("/elsewhere/bad.py")),
        run(&fixed, Some(0), clean_json(&fixed)),
        run(&corpus, Some(0), clean_json(&corpus)),
    ];
    let verdict = oncebitten::replay::replay(&spec, &Canned(Ok(runs)));
    assert!(matches!(verdict, Verdict::CouldNotRun(ref r) if r.contains("another file")));
}

#[test]
fn checker_error_status_is_could_not_run() {
    let dir = rule_dir(&[("ok.py", "x\n")]);
    let spec = ReplaySpec::from_rule_dir(dir.path(), &checker()).unwrap();
    let bad = spec.bad.display().to_string();
    let error = serde_json::json!({"status": "error", "file": bad, "reason": "rule file broken"});
    let error = format!("{error}\n");
    let runs = vec![
        run(&bad, Some(0), error),
        run(&spec.fixed.display().to_string(), Some(0), clean_json("x")),
        run(
            &spec.corpus[0].display().to_string(),
            Some(0),
            clean_json("x"),
        ),
    ];
    let verdict = oncebitten::replay::replay(&spec, &Canned(Ok(runs)));
    assert!(matches!(verdict, Verdict::CouldNotRun(ref r) if r.contains("rule file broken")));
}

#[test]
fn garbage_stdout_is_could_not_run() {
    let dir = rule_dir(&[("ok.py", "x\n")]);
    let spec = ReplaySpec::from_rule_dir(dir.path(), &checker()).unwrap();
    let bad = spec.bad.display().to_string();
    let runs = vec![
        run(
            &bad,
            Some(0),
            "Traceback (most recent call last)\n".to_owned(),
        ),
        run(&spec.fixed.display().to_string(), Some(0), clean_json("x")),
        run(
            &spec.corpus[0].display().to_string(),
            Some(0),
            clean_json("x"),
        ),
    ];
    let verdict = oncebitten::replay::replay(&spec, &Canned(Ok(runs)));
    assert!(matches!(verdict, Verdict::CouldNotRun(ref r) if r.contains("not JSON")));
}
