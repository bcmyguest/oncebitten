//! `oncebitten gate`: the blocking check behind the pre-push hook and CI. It
//! runs the active block rules on the paths a push or pull request changes,
//! requires replay proofs (for active steer rules too), and lets baselined
//! violations through.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::attachments;
use crate::baseline::{self, Baseline};
use crate::check::{self, Checked};
use crate::config::{self, Inputs};
use crate::engine::Outcome;
use crate::exceptions::{self, Exception};
use crate::forge::{self, Forge};
use crate::proof::{self, Validity};
use crate::registry::Resolved;
use crate::replay::Verdict;
use crate::rule::{Check, Mode, Rule, Status};
use crate::rules::{self, Failure};
use crate::sources;

/// Set by the pre-commit framework in the pre-push stage.
pub const FROM_REF: &str = "PRE_COMMIT_FROM_REF";
pub const TO_REF: &str = "PRE_COMMIT_TO_REF";

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
    /// Findings a live exception covered.
    pub excepted: usize,
    /// Exceptions past their `expires` date; they cover nothing.
    pub expired: Vec<Exception>,
    /// Problems that did not stop the gate, such as an unreadable pull request.
    pub warnings: Vec<String>,
}

/// Builds the forge backend for a kind, running its CLI from a directory.
pub type ForgeFor<'a> = &'a dyn Fn(forge::Kind, &Path) -> Box<dyn Forge>;

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
    run_with_forge(inputs, stage, base, &forge::backend)
}

/// `run`, with the forge `gate pr` reads attachment evidence from. `pre-push`
/// never calls it.
pub fn run_with_forge(
    inputs: &Inputs,
    stage: Stage,
    base: Option<&str>,
    forge_for: ForgeFor,
) -> Result<Report, Failure> {
    let rules = rules::load(inputs, stage == Stage::Pr)?;
    let repo = config::repo_root(&inputs.cwd)
        .ok_or_else(|| Failure::CouldNotRun(anyhow!("not inside a git repository")))?;
    let range = range(inputs, &repo, stage, base).map_err(Failure::CouldNotRun)?;
    let changed = changed(inputs, &repo, &range).map_err(Failure::CouldNotRun)?;
    let baseline = baseline::read(&repo).map_err(Failure::CouldNotRun)?;
    let exceptions = exceptions::read(&repo).map_err(Failure::CouldNotRun)?;
    let today = exceptions::today();
    let gated = gated(&rules);
    let proofs = proved(&rules)
        .into_iter()
        .filter(|resolved| needs_proof(&resolved.rule.check))
        .filter_map(|resolved| match proof_validity(&resolved.rule) {
            Validity::Valid => None,
            other => Some((resolved.id.clone(), other)),
        })
        .collect();
    let mut warnings = Vec::new();
    let mut checked = check::run_rules(gated.iter().copied(), &repo, &changed);
    if stage == Stage::Pr {
        accept_attachments(
            inputs,
            &repo,
            &gated,
            &mut checked,
            forge_for,
            &mut warnings,
        );
    }
    let (checked, known) = apply_baseline(checked, &gated, &repo, &baseline);
    let (checked, excepted) = exceptions::apply(checked, &exceptions, today);
    let expired = exceptions
        .into_iter()
        .filter(|exception| !exception.is_live(today))
        .collect();
    Ok(Report {
        range,
        proofs,
        checked,
        known,
        excepted,
        expired,
        warnings,
    })
}

/// A diff rule that fired because `require_changed` found no changed path is
/// satisfied when the pull request's description or comments link an
/// attachment of a type `require_changed` names. Any failure to read the
/// pull request leaves the rule fired and adds one warning.
fn accept_attachments(
    inputs: &Inputs,
    repo: &Path,
    gated: &[&Resolved],
    checked: &mut [Checked],
    forge_for: ForgeFor,
    warnings: &mut Vec<String>,
) {
    let mut text: Option<Option<String>> = None;
    for item in checked.iter_mut() {
        if !matches!(item.outcome, Outcome::Fired(_)) {
            continue;
        }
        let Some(Check::Diff {
            require_changed, ..
        }) = gated
            .iter()
            .find(|resolved| resolved.id == item.id)
            .map(|resolved| &resolved.rule.check)
        else {
            continue;
        };
        let text = text.get_or_insert_with(|| match pr_text(inputs, repo, forge_for) {
            Ok(text) => Some(text),
            Err(error) => {
                warnings.push(format!(
                    "could not read the pull request for attachment evidence: {error:#}; diff rules fire on the diff alone"
                ));
                None
            }
        });
        if text
            .as_deref()
            .is_some_and(|text| attachments::satisfies(text, require_changed))
        {
            item.outcome = Outcome::Clean;
        }
    }
}

/// The description and comments of the open pull request for the current branch.
fn pr_text(inputs: &Inputs, repo: &Path, forge_for: ForgeFor) -> Result<String> {
    let branch = sources::git(&inputs.vars, repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        return Err(anyhow!(
            "HEAD is detached, so the branch has no pull request"
        ));
    }
    let config = config::load(inputs)?.config;
    let kind = forge::select(config.forge, forge::origin_url(repo)?.as_deref())?;
    let backend = forge_for(kind, repo);
    let pr = backend
        .find_open(&branch)?
        .ok_or_else(|| anyhow!("no open pull request for branch `{branch}`"))?;
    let text = backend.read(pr.number)?;
    Ok(std::iter::once(text.description)
        .chain(text.comments)
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The violations the gated rules find in every tracked file now. Diff rules
/// are skipped: their findings are never baselined. Findings a live exception
/// covers are left out, so they fail the gate once the exception expires. A
/// rule that could not run fails the whole update, so a partial baseline is
/// never written.
pub fn current_baseline(inputs: &Inputs) -> Result<(PathBuf, Baseline), Failure> {
    let rules = rules::load(inputs, false)?;
    let repo = config::repo_root(&inputs.cwd)
        .ok_or_else(|| Failure::CouldNotRun(anyhow!("not inside a git repository")))?;
    let tracked = changed(inputs, &repo, &Range::AllTracked).map_err(Failure::CouldNotRun)?;
    let exceptions = exceptions::read(&repo).map_err(Failure::CouldNotRun)?;
    let gated: Vec<&Resolved> = gated(&rules)
        .into_iter()
        .filter(|resolved| !matches!(resolved.rule.check, Check::Diff { .. }))
        .collect();
    let checks: HashMap<&str, &Check> = gated
        .iter()
        .map(|resolved| (resolved.id.as_str(), &resolved.rule.check))
        .collect();
    let mut baseline = Baseline::default();
    let (checked, _) = exceptions::apply(
        check::run_rules(gated.iter().copied(), &repo, &tracked),
        &exceptions,
        exceptions::today(),
    );
    for item in checked {
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

/// The rules whose proofs the gate checks: the gated rules plus the active
/// steer rules, which the gate never runs but which act in every agent.
pub fn proved(rules: &[Resolved]) -> Vec<&Resolved> {
    rules
        .iter()
        .filter(|resolved| {
            resolved.rule.status == Status::Active
                && (resolved.rule.mode == Mode::Block
                    || matches!(resolved.rule.check, Check::Steer(_)))
        })
        .collect()
}

/// Whether the rule's proof stands for this binary. A steer rule also replays
/// its fixtures with the local matcher, whatever `proof.json` says.
pub fn proof_validity(rule: &Rule) -> Validity {
    let validity = proof::validity_with(rule, proof::local_checker(rule).as_deref());
    match (&rule.check, validity) {
        (Check::Steer(steer), Validity::Valid) => {
            match crate::steer::replay(proof::rule_dir(rule), steer) {
                Verdict::Proven => Validity::Valid,
                Verdict::Rejected(reason) => {
                    Validity::NotProven(format!("matcher replay: {reason}"))
                }
                Verdict::CouldNotRun(reason) => {
                    Validity::NotProven(format!("matcher replay could not run: {reason}"))
                }
            }
        }
        (_, validity) => validity,
    }
}

/// Native-linter rules are exempt from replay proofs until replay can run their
/// toolchain in the sandbox. The gate still runs them.
pub fn needs_proof(check: &Check) -> bool {
    !matches!(check, Check::Native { .. })
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
            check::git_names(inputs, repo, &["diff", "-z", "--name-only", from, to])?
        }
        Range::AllTracked => check::git_names(inputs, repo, &["ls-files", "-z"])?,
    };
    Ok(check::without_rule_dirs(paths))
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
