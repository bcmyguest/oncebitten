//! Replays an ast-grep or diff rule against its fixtures inside a sandbox.
//!
//! A rule is proven when the checker reports `fired` for `fixtures/bad.*`, and
//! `clean` for `fixtures/fixed.*` and every file in `corpus/`. Exit codes are
//! evidence only: a result counts when the JSON line, the target path and exit
//! code 0 agree.
//!
//! A diff rule's fixtures are lists of changed paths, one per line. The checker
//! reads them in `diff` mode, so the same replay runs for both engines.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::rule::{is_fixture_named, read_diff_rule};

pub mod openshell;

pub const RULE_FILE: &str = "check.yml";
pub const DIFF_RULE_FILE: &str = "rule.toml";
pub const FIXTURES_DIR: &str = "fixtures";
pub const CORPUS_DIR: &str = "corpus";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Proven,
    Rejected(String),
    CouldNotRun(String),
}

/// Which checker mode a replay runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    AstGrep,
    Diff,
}

impl CheckKind {
    /// The arguments that select the checker mode, before the rule and the target.
    pub fn checker_args(self) -> &'static [&'static str] {
        match self {
            CheckKind::AstGrep => &[],
            CheckKind::Diff => &["diff"],
        }
    }
}

/// The files one replay checks, all on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaySpec {
    pub kind: CheckKind,
    pub rule: PathBuf,
    pub checker: PathBuf,
    pub bad: PathBuf,
    pub fixed: PathBuf,
    pub corpus: Vec<PathBuf>,
}

impl ReplaySpec {
    /// The rule file's name, `check.yml` or `rule.toml`. The sandbox keeps it.
    pub fn rule_name(&self) -> String {
        self.rule
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// One checker invocation as the backend observed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRun {
    pub target: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs the checker on each file and returns the results in input order.
/// An `Err` means the sandbox or runner failed, not that the rule failed.
pub trait Backend {
    fn run(&self, spec: &ReplaySpec, files: &[PathBuf]) -> Result<Vec<RawRun>, String>;
}

/// Runs the checker on the host. No isolation: used by tests only.
pub struct LocalRunner;

impl Backend for LocalRunner {
    fn run(&self, spec: &ReplaySpec, files: &[PathBuf]) -> Result<Vec<RawRun>, String> {
        files
            .iter()
            .map(|file| {
                let output = Command::new(&spec.checker)
                    .args(spec.kind.checker_args())
                    .arg(&spec.rule)
                    .arg(file)
                    .output()
                    .map_err(|error| format!("starting the checker: {error}"))?;
                Ok(RawRun {
                    target: file.display().to_string(),
                    exit_code: output.status.code(),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Check {
    Fired(usize),
    Clean,
    CouldNotRun(String),
}

/// Reads the checker's single JSON line and accepts it only when it agrees
/// with the target path and exit code 0.
fn classify(run: &RawRun) -> Check {
    let lines: Vec<&str> = run
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let [line] = lines.as_slice() else {
        return Check::CouldNotRun(format!(
            "expected one JSON line from the checker, got {}; stderr: {}",
            lines.len(),
            run.stderr.trim()
        ));
    };
    let Ok(report) = serde_json::from_str::<serde_json::Value>(line) else {
        return Check::CouldNotRun(format!("checker output is not JSON: {line}"));
    };
    if run.exit_code != Some(0) {
        return Check::CouldNotRun(format!("checker exited with {:?}", run.exit_code));
    }
    if report["file"].as_str() != Some(run.target.as_str()) {
        return Check::CouldNotRun(format!("checker reported another file: {line}"));
    }
    match report["status"].as_str() {
        Some("fired") => match report["matches"].as_array().map(Vec::len) {
            Some(count) if count > 0 => Check::Fired(count),
            _ => Check::CouldNotRun(format!("fired without matches: {line}")),
        },
        Some("clean") => Check::Clean,
        Some("error") => Check::CouldNotRun(format!("checker error: {}", report["reason"])),
        _ => Check::CouldNotRun(format!("unknown status: {line}")),
    }
}

/// Checks the rule against the replay files. The checker must be a file on the host.
pub fn replay(spec: &ReplaySpec, backend: &dyn Backend) -> Verdict {
    if !spec.checker.is_file() {
        return Verdict::CouldNotRun(format!(
            "missing checker: {} is not a file",
            spec.checker.display()
        ));
    }
    let mut files = vec![spec.bad.clone(), spec.fixed.clone()];
    files.extend(spec.corpus.iter().cloned());
    let runs = match backend.run(spec, &files) {
        Ok(runs) if runs.len() == files.len() => runs,
        Ok(runs) => {
            return Verdict::CouldNotRun(format!(
                "backend returned {} results for {} files",
                runs.len(),
                files.len()
            ))
        }
        Err(reason) => return Verdict::CouldNotRun(reason),
    };
    let mut rejected = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        let expect_fired = index == 0;
        match classify(run) {
            Check::CouldNotRun(reason) => {
                return Verdict::CouldNotRun(format!("{}: {reason}", run.target))
            }
            Check::Fired(count) if !expect_fired => {
                rejected.push(format!("{} fired ({count} matches)", run.target))
            }
            Check::Clean if expect_fired => rejected.push(format!("{} did not fire", run.target)),
            _ => {}
        }
    }
    if rejected.is_empty() {
        Verdict::Proven
    } else {
        Verdict::Rejected(rejected.join("; "))
    }
}

/// Builds the spec from a rule directory and runs the replay.
pub fn replay_dir(rule_dir: &Path, checker: &Path, backend: &dyn Backend) -> Verdict {
    match ReplaySpec::from_rule_dir(rule_dir, checker) {
        Ok(spec) => replay(&spec, backend),
        Err(reason) => Verdict::CouldNotRun(reason),
    }
}

impl ReplaySpec {
    /// Finds the rule file, `fixtures/bad.*`, `fixtures/fixed.*` and `corpus/*`.
    /// A `rule.toml` that declares `engine = "diff"` selects the diff checker;
    /// otherwise the rule file is `check.yml`.
    pub fn from_rule_dir(rule_dir: &Path, checker: &Path) -> Result<Self, String> {
        let (kind, rule) = match diff_rule_file(rule_dir)? {
            Some(file) => (CheckKind::Diff, file),
            None => (CheckKind::AstGrep, rule_dir.join(RULE_FILE)),
        };
        if !rule.is_file() {
            return Err(format!("missing input: {} not found", rule.display()));
        }
        let fixtures = rule_dir.join(FIXTURES_DIR);
        let bad = single_named(&fixtures, "bad")?;
        let fixed = single_named(&fixtures, "fixed")?;
        let corpus = files_in(&rule_dir.join(CORPUS_DIR))?;
        if corpus.is_empty() {
            return Err(format!(
                "missing input: {} has no files",
                rule_dir.join(CORPUS_DIR).display()
            ));
        }
        Ok(Self {
            kind,
            rule,
            checker: checker.to_path_buf(),
            bad,
            fixed,
            corpus,
        })
    }
}

/// The `rule.toml` of a diff rule directory, or `None` for any other engine.
fn diff_rule_file(rule_dir: &Path) -> Result<Option<PathBuf>, String> {
    let file = rule_dir.join(DIFF_RULE_FILE);
    if !file.is_file() {
        return Ok(None);
    }
    match read_diff_rule(&file) {
        Ok(Some(_)) => Ok(Some(file)),
        Ok(None) => Ok(None),
        Err(error) => Err(format!("{error:#}")),
    }
}

fn single_named(dir: &Path, stem: &str) -> Result<PathBuf, String> {
    let found: Vec<PathBuf> = files_in(dir)?
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| is_fixture_named(name, stem))
        })
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(format!(
            "missing input: {} needs exactly one {stem}.* file, found {}",
            dir.display(),
            found.len()
        )),
    }
}

fn files_in(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("missing input: {}: {error}", dir.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("reading {}: {error}", dir.display()))?
            .path();
        if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
