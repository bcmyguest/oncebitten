//! How often a proposed rule fires on the repo at HEAD. The count informs
//! reviewers; nothing here blocks a proposal.

use std::path::Path;

use anyhow::{bail, Result};

use crate::check;
use crate::config::Inputs;
use crate::engine::{astgrep, Finding, Outcome};
use crate::rule::{self, Check};
use crate::store::HitsRow;

/// Sample lines kept per proposal.
const SAMPLE_LIMIT: usize = 3;
/// Lists every path in HEAD's tree, NUL-separated.
const HEAD_PATHS: [&str; 5] = ["ls-tree", "-r", "--name-only", "-z", "HEAD"];

/// Counts the hits that the rule in `rule_dir` makes over the tracked files
/// at HEAD. Returns `None` for an engine with no per-line hits: native,
/// diff, or steer.
pub fn measure(inputs: &Inputs, repo: &Path, rule_dir: &Path) -> Result<Option<HitsRow>> {
    let rule = rule::load_rule(rule_dir)?;
    let Check::AstGrep { file } = &rule.check else {
        return Ok(None);
    };
    let paths: Vec<String> = check::git_names(inputs, repo, &HEAD_PATHS)?
        .into_iter()
        .filter(|path| repo.join(path).is_file())
        .collect();
    match astgrep::run_in(file, repo, &paths) {
        Outcome::Clean => Ok(Some(summarize(&[]))),
        Outcome::Fired(findings) => Ok(Some(summarize(&findings))),
        Outcome::CouldNotRun(reason) => bail!("could not measure hits: {reason}"),
    }
}

fn summarize(findings: &[Finding]) -> HitsRow {
    let mut files: Vec<&Path> = findings.iter().filter_map(|f| f.path.as_deref()).collect();
    files.sort();
    files.dedup();
    HitsRow {
        count: findings.len() as i64,
        files: files.len() as i64,
        samples: findings.iter().take(SAMPLE_LIMIT).map(sample).collect(),
    }
}

fn sample(finding: &Finding) -> String {
    let path = finding
        .path
        .as_deref()
        .map_or(String::new(), |path| path.display().to_string());
    let line = finding.line.unwrap_or(0);
    let text = finding
        .text
        .as_deref()
        .unwrap_or(&finding.message)
        .lines()
        .next()
        .unwrap_or("")
        .trim();
    format!("{path}:{line}: {text}")
}

/// The one-line count reviewers see: `N hits in M files`.
pub fn summary_text(hits: &HitsRow) -> String {
    let hit = if hits.count == 1 { "hit" } else { "hits" };
    let file = if hits.files == 1 { "file" } else { "files" };
    format!("{} {hit} in {} {file}", hits.count, hits.files)
}
