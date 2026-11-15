use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::{self, Inputs};
use crate::evidence::Cluster;
use crate::proof::{self, Replayed, Validity};
use crate::replay::Verdict;
use crate::rule::{self, Rule, Status};
use crate::rules::{self, Failure};
use crate::store::Store;
pub use crate::store::{Kind, ProposalRow, State};
use crate::triage::verdict::Answer;

pub const LIST_HINT: &str = "oncebitten proposals list";
const PROJECT_RULES: [&str; 2] = [".oncebitten", "rules"];
const PROPOSED_LINE: &str = "status = \"proposed\"";
const ACTIVE_LINE: &str = "status = \"active\"";

/// What `record` did with one triage answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// The draft was written as a proposed rule; `proven` when replay proved it.
    Proposed { proven: bool },
    /// The verdict drafted no rule; it waits in the queue as a finding.
    Finding,
    /// The cluster already has a queued result; nothing was written.
    AlreadyQueued,
    /// The cluster was rejected; nothing was written.
    Rejected,
}

/// Queues one triage answer for `cluster`. A draft becomes a rule directory
/// with `status = "proposed"` under the repo's `.oncebitten/rules/`, then
/// `prove` replays it. A cluster is queued at most once.
pub fn record(
    store: &Store,
    inputs: &Inputs,
    cluster: &Cluster,
    answer: &Answer,
    prove: &dyn Fn(&Rule) -> Replayed,
    ts: i64,
) -> Result<Recorded> {
    if cluster.rejection.is_some() {
        return Ok(Recorded::Rejected);
    }
    if store.proposal(&cluster.id)?.is_some() {
        return Ok(Recorded::AlreadyQueued);
    }
    let repo = config::repo_root(&inputs.cwd).context("not inside a git repository")?;
    let mut row = ProposalRow {
        cluster: cluster.id.clone(),
        ts,
        repo: repo_key(&repo),
        kind: Kind::Finding,
        state: State::Waiting,
        rule: None,
        dir: None,
        proven: false,
        note: None,
        verdict: serde_json::to_string(&answer.verdict)?,
        rationale: None,
        occurrences: cluster.occurrences.iter().map(|o| o.event.id).collect(),
    };
    let Some(draft) = &answer.draft else {
        store.insert_proposal(&row)?;
        return Ok(Recorded::Finding);
    };
    let existing = load_rules(inputs).context("the current rules do not load")?;
    if existing.iter().any(|resolved| resolved.rule.id == draft.id) {
        bail!("a rule named `{}` already exists", draft.id);
    }
    let dir = draft.write(&PROJECT_RULES.iter().fold(repo, |dir, part| dir.join(part)))?;
    if let Err(error) = load_rules(inputs) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(error.context(format!("proposal `{}` breaks the rule set", draft.id)));
    }
    let rule = rule::load_rule(&dir)?;
    let (proven, note) = proven(&rule, &prove(&rule));
    row.kind = Kind::Proposal;
    row.rule = Some(draft.id.clone());
    row.dir = Some(dir.clone());
    row.proven = proven;
    row.note = note;
    row.rationale = Some(draft.rationale.trim().to_owned());
    if !store.insert_proposal(&row)? {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(Recorded::AlreadyQueued);
    }
    Ok(Recorded::Proposed { proven })
}

/// Flips a waiting proposal to `status = "active"` when its proof stands.
/// Returns the rule directory.
pub fn accept(store: &Store, id: &str) -> Result<PathBuf> {
    let row = find(store, id)?;
    if row.kind == Kind::Finding {
        bail!("`{id}` is a finding; it has no rule to accept");
    }
    if row.state != State::Waiting {
        bail!("proposal `{id}` is already {}", row.state.as_str());
    }
    let dir = row.dir.context("the proposal names no rule directory")?;
    let rule = rule::load_rule(&dir)?;
    if rule.status != Status::Proposed {
        bail!(
            "rule `{}` is {}, not proposed",
            rule.id,
            rule.status.as_str()
        );
    }
    match proof::validity(&rule) {
        Validity::Valid => {}
        Validity::Missing => bail!(
            "rule `{}` has no proof; replay it in a sandbox first: oncebitten replay {}",
            rule.id,
            rule.id
        ),
        Validity::Stale(reason) => bail!("rule `{}` has a stale proof: {reason}", rule.id),
        Validity::NotProven(reason) => bail!("rule `{}` has no valid proof: {reason}", rule.id),
    }
    let original = std::fs::read_to_string(&rule.file)
        .with_context(|| format!("reading {}", rule.file.display()))?;
    let flipped = activate(&original)
        .with_context(|| format!("{} has no `{PROPOSED_LINE}` line", rule.file.display()))?;
    std::fs::write(&rule.file, &flipped)
        .with_context(|| format!("writing {}", rule.file.display()))?;
    let check = rule::load_rule(&dir).map(|active| (active.status, proof::validity(&active)));
    if !matches!(check, Ok((Status::Active, Validity::Valid))) {
        std::fs::write(&rule.file, original)
            .with_context(|| format!("restoring {}", rule.file.display()))?;
        bail!("activating `{}` did not keep its proof valid", rule.id);
    }
    store.set_proposal_state(id, State::Accepted)?;
    Ok(dir)
}

/// Rejects a queued result. The cluster is rejected in the evidence store, so
/// triage does not propose it again, and a still-proposed rule is removed.
pub fn reject(store: &Store, id: &str, reason: &str, ts: i64) -> Result<()> {
    if reason.trim().is_empty() {
        bail!("a rejection needs a non-empty reason");
    }
    let row = find(store, id)?;
    if let Some(dir) = &row.dir {
        let still_proposed =
            matches!(rule::load_rule(dir), Ok(rule) if rule.status == Status::Proposed);
        if still_proposed {
            std::fs::remove_dir_all(dir).with_context(|| format!("removing {}", dir.display()))?;
        }
    }
    store.reject(id, ts, reason.trim())?;
    store.set_proposal_state(id, State::Rejected)
}

/// The non-blocking line the gate prints when proposals wait in `repo`.
pub fn notice(store: &Store, repo: &Path) -> Result<Option<String>> {
    let key = repo_key(repo);
    let waiting = store
        .proposals()?
        .into_iter()
        .filter(|row| row.kind == Kind::Proposal && row.state == State::Waiting && row.repo == key)
        .count();
    Ok(match waiting {
        0 => None,
        1 => Some(format!("1 rule proposal waiting: {LIST_HINT}")),
        n => Some(format!("{n} rule proposals waiting: {LIST_HINT}")),
    })
}

/// The key a repo is queued under: its canonical path when it resolves.
pub fn repo_key(repo: &Path) -> String {
    std::fs::canonicalize(repo)
        .unwrap_or_else(|_| repo.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn find(store: &Store, id: &str) -> Result<ProposalRow> {
    match store.proposal(id)? {
        Some(row) => Ok(row),
        None => bail!("no proposal `{id}`; see {LIST_HINT}"),
    }
}

fn load_rules(inputs: &Inputs) -> Result<Vec<crate::registry::Resolved>> {
    rules::load(inputs, false).map_err(|failure| match failure {
        Failure::CouldNotRun(error) => error,
        Failure::Invalid(errors) => anyhow::anyhow!(errors
            .iter()
            .map(|error| format!("{error:#}"))
            .collect::<Vec<_>>()
            .join("; ")),
    })
}

/// Whether the replay left a valid proof, and why not when it did not.
fn proven(rule: &Rule, replayed: &Replayed) -> (bool, Option<String>) {
    match &replayed.verdict {
        Verdict::Proven if proof::validity(rule) == Validity::Valid => (true, None),
        Verdict::Proven => (
            false,
            Some("replay passed but wrote no sandbox proof".to_owned()),
        ),
        Verdict::Rejected(reason) => (false, Some(format!("replay rejected it: {reason}"))),
        Verdict::CouldNotRun(reason) => (false, Some(format!("replay could not run: {reason}"))),
    }
}

/// `text` with its `status = "proposed"` line set to active, line ending kept.
fn activate(text: &str) -> Option<String> {
    let mut found = false;
    let out = text
        .split_inclusive('\n')
        .map(|line| {
            let body = line.trim_end_matches(['\r', '\n']);
            if !found && body.trim() == PROPOSED_LINE {
                found = true;
                format!("{ACTIVE_LINE}{}", &line[body.len()..])
            } else {
                line.to_owned()
            }
        })
        .collect::<String>();
    found.then_some(out)
}
