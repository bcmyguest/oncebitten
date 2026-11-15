//! `oncebitten gate`: the blocking check behind the pre-push hook and CI. It
//! runs the active block rules on the paths a push or pull request changes,
//! requires replay proofs, and lets baselined violations through.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::baseline::{self, Baseline};
use crate::check::{self, Checked};
use crate::config::{self, Inputs};
use crate::engine::Outcome;
use crate::proof::{self, Validity};
use crate::registry::Resolved;
use crate::rule::{Check, Mode, Status};
use crate::rules::{self, Failure};

/// Set by the pre-commit framework in the pre-push stage.
pub const FROM_REF: &str = "PRE_COMMIT_FROM_REF";
pub const TO_REF: &str = "PRE_COMMIT_TO_REF";
const RULES_PREFIX: &str = ".oncebitten/rules/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    PrePush,
    Pr,
}

/// What the gate checked and which paths it checked them on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Range {
    Diff { from: String, to: String },
    AllTracked,
}

pub struct Report {
    pub range: Range,
    /// Rules the gate ran whose proof does not stand.
    pub proofs: Vec<(String, Validity)>,
    /// Outcomes with baselined findings removed.
    pub checked: Vec<Checked>,
    /// Findings the baseline matched.
    pub known: usize,
}

impl Report {
    /// 2 if a rule could not run, or for `pr` a proof is missing or stale;
    /// 1 if a rule fired on a new violation; 0 otherwise.
    pub fn exit_code(&self, stage: Stage) -> i32 {
        let code = check::exit_code(&self.checked);
        if stage == Stage::Pr && !self.proofs.is_empty() {
            2
        } else {
            code
        }
    }
}

pub fn run(inputs: &Inputs, stage: Stage, base: Option<&str>) -> Result<Report, Failure> {
    let rules = rules::load(inputs, stage == Stage::Pr)?;
    let repo = config::repo_root(&inputs.cwd)
        .ok_or_else(|| Failure::CouldNotRun(anyhow!("not inside a git repository")))?;
    let range = range(inputs, &repo, stage, base).map_err(Failure::CouldNotRun)?;
    let changed = changed(inputs, &repo, &range).map_err(Failure::CouldNotRun)?;
    let baseline = baseline::read(&repo).map_err(Failure::CouldNotRun)?;
    let gated = gated(&rules);
    let proofs = gated
        .iter()
        .filter(|resolved| needs_proof(&resolved.rule.check))
        .filter_map(|resolved| match proof::validity(&resolved.rule) {
            Validity::Valid => None,
            other => Some((resolved.id.clone(), other)),
        })
        .collect();
    let checked = check::run_rules(gated.iter().copied(), &repo, &changed);
    let (checked, known) = apply_baseline(checked, &gated, &repo, &baseline);
    Ok(Report {
        range,
        proofs,
        checked,
        known,
    })
}

/// The violations the gated rules find in every tracked file now. Diff rules
/// are skipped: their findings are never baselined. A rule that could not run
/// fails the whole update, so a partial baseline is never written.
pub fn current_baseline(inputs: &Inputs) -> Result<(PathBuf, Baseline), Failure> {
    let rules = rules::load(inputs, false)?;
    let repo = config::repo_root(&inputs.cwd)
        .ok_or_else(|| Failure::CouldNotRun(anyhow!("not inside a git repository")))?;
    let tracked = changed(inputs, &repo, &Range::AllTracked).map_err(Failure::CouldNotRun)?;
    let gated: Vec<&Resolved> = gated(&rules)
        .into_iter()
        .filter(|resolved| !matches!(resolved.rule.check, Check::Diff { .. }))
        .collect();
    let checks: HashMap<&str, &Check> = gated
        .iter()
        .map(|resolved| (resolved.id.as_str(), &resolved.rule.check))
        .collect();
    let mut baseline = Baseline::default();
    for item in check::run_rules(gated.iter().copied(), &repo, &tracked) {
        match item.outcome {
            Outcome::Clean => {}
            Outcome::Fired(findings) => {
                baseline
                    .entries
                    .extend(findings.iter().filter_map(|finding| {
                        baseline::entry(&item.id, checks[item.id.as_str()], &repo, finding)
                    }))
            }
            Outcome::CouldNotRun(reason) => {
                return Err(Failure::CouldNotRun(anyhow!(
                    "{}: could not run: {reason}",
                    item.id
                )))
            }
        }
    }
    Ok((repo, baseline))
}

/// The rules the gate enforces: active and in block mode.
pub fn gated(rules: &[Resolved]) -> Vec<&Resolved> {
    rules
        .iter()
        .filter(|resolved| {
            resolved.rule.status == Status::Active && resolved.rule.mode == Mode::Block
        })
        .collect()
}

/// Replay supports ast-grep rules only, so only they can carry a proof.
pub fn needs_proof(check: &Check) -> bool {
    matches!(check, Check::AstGrep { .. })
}

pub fn describe(id: &str, validity: &Validity) -> String {
    match validity {
        Validity::Valid => format!("{id}: proof is valid"),
        Validity::Missing => {
            format!("{id}: no replay proof (proof.json); run oncebitten replay {id}")
        }
        Validity::Stale(reason) => {
            format!("{id}: stale replay proof: {reason}; run oncebitten replay {id}")
        }
        Validity::NotProven(reason) => format!("{id}: replay proof not accepted: {reason}"),
    }
}

fn range(inputs: &Inputs, repo: &Path, stage: Stage, base: Option<&str>) -> Result<Range> {
    if let Some(base) = base {
        return since_merge_base(inputs, repo, base);
    }
    if stage == Stage::Pr {
        return Err(anyhow!("gate pr needs --base <ref>"));
    }
    if let (Some(from), Some(to)) = (inputs.vars.get(FROM_REF), inputs.vars.get(TO_REF)) {
        return Ok(Range::Diff {
            from: from.clone(),
            to: to.clone(),
        });
    }
    let upstream = check::git_paths(
        inputs,
        repo,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    );
    match upstream.ok().and_then(|lines| lines.into_iter().next()) {
        Some(upstream) => since_merge_base(inputs, repo, &upstream),
        None => Ok(Range::AllTracked),
    }
}

fn since_merge_base(inputs: &Inputs, repo: &Path, base: &str) -> Result<Range> {
    let from = check::git_paths(inputs, repo, &["merge-base", base, "HEAD"])?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("{base} and HEAD share no commit"))?;
    Ok(Range::Diff {
        from,
        to: "HEAD".to_owned(),
    })
}

/// The range's paths, minus the project's rule directories: their
/// `fixtures/bad.*` exist to violate the rule.
fn changed(inputs: &Inputs, repo: &Path, range: &Range) -> Result<Vec<String>> {
    let paths = match range {
        Range::Diff { from, to } => {
            check::git_paths(inputs, repo, &["diff", "--name-only", from, to])?
        }
        Range::AllTracked => check::git_paths(inputs, repo, &["ls-files"])?,
    };
    Ok(paths
        .into_iter()
        .filter(|path| !path.starts_with(RULES_PREFIX))
        .collect())
}

fn apply_baseline(
    checked: Vec<Checked>,
    gated: &[&Resolved],
    repo: &Path,
    baseline: &Baseline,
) -> (Vec<Checked>, usize) {
    let checks: HashMap<&str, &Check> = gated
        .iter()
        .map(|resolved| (resolved.id.as_str(), &resolved.rule.check))
        .collect();
    let mut known = 0;
    let checked = checked
        .into_iter()
        .map(|item| {
            let Outcome::Fired(findings) = item.outcome else {
                return item;
            };
            let check = checks[item.id.as_str()];
            let total = findings.len();
            let new: Vec<_> = findings
                .into_iter()
                .filter(|finding| {
                    baseline::entry(&item.id, check, repo, finding)
                        .is_none_or(|entry| !baseline.contains(&entry))
                })
                .collect();
            known += total - new.len();
            Checked {
                outcome: if new.is_empty() {
                    Outcome::Clean
                } else {
                    Outcome::Fired(new)
                },
                id: item.id,
            }
        })
        .collect();
    (checked, known)
}
