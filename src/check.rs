use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::config::{self, Inputs};
use crate::engine::{astgrep, diff, native, Finding, Outcome};
use crate::exceptions;
use crate::registry::Resolved;
use crate::rule::{Check, Mode, Status};
use crate::rules::{self, Failure};
use crate::sources::git;

/// The project's rule directories. Their `fixtures/bad.*` exist to violate
/// the rule, so `check` and the gate never scan them.
pub const RULES_PREFIX: &str = ".oncebitten/rules/";

pub struct Checked {
    pub id: String,
    pub outcome: Outcome,
}

/// Outcomes with excepted findings removed, and how many were excepted.
pub struct Ran {
    pub checked: Vec<Checked>,
    pub excepted: usize,
}

pub fn run(inputs: &Inputs, base: &str) -> Result<Ran, Failure> {
    let rules = rules::load(inputs, false)?;
    let repo = config::repo_root(&inputs.cwd)
        .ok_or_else(|| Failure::CouldNotRun(anyhow!("not inside a git repository")))?;
    let exceptions = exceptions::read(&repo).map_err(Failure::CouldNotRun)?;
    let changed = changed_paths(inputs, &repo, base).map_err(Failure::CouldNotRun)?;
    let (checked, excepted) = exceptions::apply(
        run_rules(rules.iter(), &repo, &changed),
        &exceptions,
        exceptions::today(),
    );
    Ok(Ran { checked, excepted })
}

/// Runs the active block rules among `rules` against the changed repo-relative
/// paths. Steer rules are skipped: they act in the agent, not on git changes.
pub fn run_rules<'a>(
    rules: impl IntoIterator<Item = &'a Resolved>,
    repo: &Path,
    changed: &[String],
) -> Vec<Checked> {
    rules
        .into_iter()
        .filter(|resolved| {
            resolved.rule.status == Status::Active && resolved.rule.mode == Mode::Block
        })
        .map(|resolved| Checked {
            id: resolved.id.clone(),
            outcome: run_rule(resolved, repo, changed),
        })
        .collect()
}

/// Runs `git -c core.quotePath=false <args>` in `repo` and returns its
/// non-empty output lines, such as a ref.
pub fn git_paths(inputs: &Inputs, repo: &Path, args: &[&str]) -> Result<Vec<String>> {
    let mut full = vec!["-c", "core.quotePath=false"];
    full.extend_from_slice(args);
    Ok(git(&inputs.vars, repo, &full)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Runs `git -c core.quotePath=false <args>` in `repo` and returns the
/// NUL-separated paths in its output. `args` must include `-z`, so names
/// with tabs or newlines arrive intact. Each path is repo-relative and `/`-separated.
pub fn git_names(inputs: &Inputs, repo: &Path, args: &[&str]) -> Result<Vec<String>> {
    let mut full = vec!["-c", "core.quotePath=false"];
    full.extend_from_slice(args);
    Ok(git(&inputs.vars, repo, &full)?
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

pub fn exit_code(checked: &[Checked]) -> i32 {
    let mut code = 0;
    for item in checked {
        match item.outcome {
            Outcome::Clean => {}
            Outcome::Fired(_) => code = code.max(1),
            Outcome::CouldNotRun(_) => code = 2,
        }
    }
    code
}

pub fn format_finding(id: &str, finding: &Finding) -> String {
    match (&finding.path, finding.line) {
        (Some(path), Some(line)) => format!("{id}: {}:{line}: {}", path.display(), finding.message),
        (Some(path), None) => format!("{id}: {}: {}", path.display(), finding.message),
        (None, _) => format!("{id}: {}", finding.message),
    }
}

fn run_rule(resolved: &Resolved, repo: &Path, changed: &[String]) -> Outcome {
    let rule = &resolved.rule;
    match &rule.check {
        Check::AstGrep { file } => {
            let files: Vec<String> = changed
                .iter()
                // git reports `/`-separated paths; join natively for Windows.
                .filter(|path| repo.join(path.split('/').collect::<PathBuf>()).is_file())
                .cloned()
                .collect();
            astgrep::run_in(file, repo, &files)
        }
        Check::Native { command } => native::run_matching(
            command,
            &rule.fired_exit_codes,
            rule.fired_output_contains.as_deref(),
            repo,
        ),
        Check::Diff {
            when_changed,
            require_changed,
        } => diff::run(when_changed, require_changed, changed),
        Check::Steer(_) => {
            Outcome::CouldNotRun("steer rules have nothing to check in git".to_owned())
        }
    }
}

fn changed_paths(inputs: &Inputs, repo: &Path, base: &str) -> Result<Vec<String>> {
    let mut paths = git_names(inputs, repo, &["diff", "-z", "--name-only", base])?;
    paths.extend(git_names(
        inputs,
        repo,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )?);
    Ok(without_rule_dirs(paths))
}

/// Drops paths under [`RULES_PREFIX`].
pub fn without_rule_dirs(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|path| !path.starts_with(RULES_PREFIX))
        .collect()
}
