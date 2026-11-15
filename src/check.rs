use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::config::{self, Inputs};
use crate::engine::{astgrep, diff, native, Finding, Outcome};
use crate::registry::Resolved;
use crate::rule::{Check, Status};
use crate::rules::{self, Failure};
use crate::sources::git;

pub struct Checked {
    pub id: String,
    pub outcome: Outcome,
}

pub fn run(inputs: &Inputs, base: &str) -> Result<Vec<Checked>, Failure> {
    let rules = rules::load(inputs, false)?;
    let repo = config::repo_root(&inputs.cwd)
        .ok_or_else(|| Failure::CouldNotRun(anyhow!("not inside a git repository")))?;
    let changed = changed_paths(inputs, &repo, base).map_err(Failure::CouldNotRun)?;
    Ok(run_rules(rules.iter(), &repo, &changed))
}

/// Runs the active rules among `rules` against the changed repo-relative paths.
pub fn run_rules<'a>(
    rules: impl IntoIterator<Item = &'a Resolved>,
    repo: &Path,
    changed: &[String],
) -> Vec<Checked> {
    rules
        .into_iter()
        .filter(|resolved| resolved.rule.status == Status::Active)
        .map(|resolved| Checked {
            id: resolved.id.clone(),
            outcome: run_rule(resolved, repo, changed),
        })
        .collect()
}

/// Runs `git -c core.quotePath=false <args>` in `repo` and returns its
/// non-empty output lines: one repo-relative `/`-separated path each.
pub fn git_paths(inputs: &Inputs, repo: &Path, args: &[&str]) -> Result<Vec<String>> {
    let mut full = vec!["-c", "core.quotePath=false"];
    full.extend_from_slice(args);
    Ok(git(&inputs.vars, repo, &full)?
        .lines()
        .filter(|line| !line.is_empty())
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
    }
}

fn changed_paths(inputs: &Inputs, repo: &Path, base: &str) -> Result<Vec<String>> {
    let mut paths = git_paths(inputs, repo, &["diff", "--name-only", base])?;
    paths.extend(git_paths(
        inputs,
        repo,
        &["ls-files", "--others", "--exclude-standard"],
    )?);
    Ok(paths)
}
