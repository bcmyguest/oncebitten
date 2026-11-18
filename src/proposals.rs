use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::{self, Inputs, SessionTrigger, Triggers};
use crate::duplicates::{first_match, Candidate, Mark};
use crate::evidence::{self, Cluster};
use crate::hits;
use crate::proof::{self, Replayed, Validity};
use crate::replay::Verdict;
use crate::rule::{self, Check, Rule, Status, Trigger};
use crate::rules::{self, Failure};
use crate::signal;
pub use crate::store::{Kind, ProposalMarks, ProposalRow, State};
use crate::store::{Outcome, Store};
use crate::triage::draft::{command_trigger, Draft, Engine};
use crate::triage::flow::corrected_turns;
use crate::triage::prompt::{classes_block, context_block, KnownClass};
use crate::triage::verdict::Answer;

pub const LIST_HINT: &str = "oncebitten proposals list";
const PROJECT_RULES: [&str; 2] = [".oncebitten", "rules"];
const PROPOSED_LINE: &str = "status = \"proposed\"";
const ACTIVE_LINE: &str = "status = \"active\"";
const DENY_ACTION: &str = "action = \"deny\"";
/// Most clean commands a steer draft takes from the store as its corpus.
const STEER_CORPUS: usize = 5;
/// A triage lock older than this is taken over. It outlasts two host calls
/// at the triage timeout plus a sandbox replay.
pub const TRIAGE_LOCK_STALE_MS: i64 = 30 * 60 * 1000;

/// A cluster due for background triage, and the repo it is proposed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub cluster: String,
    pub repo: PathBuf,
    /// The host that captured the event, as the store names it.
    pub host: String,
}

/// What `triage_queued` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    Recorded(Recorded),
    /// Another triage of the cluster holds the lock.
    Locked,
    /// The cluster is already queued or rejected; the host was not asked.
    Skipped,
}

/// What `record` did with one triage answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// The draft was written as a proposed rule; `proven` when replay proved it.
    Proposed { proven: bool },
    /// The verdict drafted no rule; it waits in the queue as a finding.
    Finding,
    /// A manual triage drafted no rule either; the queued finding stays.
    FindingKept,
    /// The cluster already has a queued result; nothing was written.
    AlreadyQueued,
    /// The cluster was rejected; nothing was written.
    Rejected,
}

/// Clusters that event `event_id` brought to their threshold, not yet queued
/// or rejected, whose event ran inside a git repository.
pub fn due(store: &Store, event_id: i64) -> Result<Vec<Due>> {
    due_gated(store, event_id, &Triggers::default())
}

/// The session trigger that starts a cluster of this class, or `None` when
/// the class starts from a gate or a correction and never waits on a session
/// trigger. Failure clusters come from tool-failure and repeat-failure, so
/// both map to the repeat-failure trigger.
fn session_trigger_for(class: &str) -> Option<SessionTrigger> {
    match class {
        evidence::CLASS_FAILURE => Some(SessionTrigger::RepeatFailure),
        signal::USER_CORRECTION => Some(SessionTrigger::UserCorrection),
        signal::TEST_FAIL_THEN_PASS => Some(SessionTrigger::TestFailThenPass),
        signal::REVERT => Some(SessionTrigger::Revert),
        _ => None,
    }
}

fn session_trigger_on(class: &str, triggers: &Triggers) -> bool {
    match session_trigger_for(class) {
        Some(trigger) => triggers.session.contains(&trigger),
        None => true,
    }
}

/// Clusters a budget skip left behind, in `repo`, oldest skip first. A cluster
/// qualifies while its latest `triage_runs` row is `skipped: budget` and it
/// has no proposal or rejection yet. Each due item carries the skip's host.
pub fn pickup(store: &Store, repo: &Path, triggers: &Triggers) -> Result<Vec<Due>> {
    let clusters = evidence::load(store)?;
    let mut out = Vec::new();
    for run in store.budget_skipped()? {
        let Some(cluster) = clusters.iter().find(|c| c.id == run.cluster) else {
            continue;
        };
        if cluster.rejection.is_some() || store.proposal(&cluster.id)?.is_some() {
            continue;
        }
        if !session_trigger_on(cluster.class, triggers) {
            continue;
        }
        let Some(event) = cluster.occurrences.last().map(|o| &o.event) else {
            continue;
        };
        let event_repo = event
            .repo
            .as_deref()
            .map(Path::new)
            .filter(|r| r.is_absolute())
            .and_then(config::repo_root);
        if event_repo.as_deref() != Some(repo) {
            continue;
        }
        out.push(Due {
            cluster: cluster.id.clone(),
            repo: repo.to_path_buf(),
            host: run.host.unwrap_or_default(),
        });
    }
    Ok(out)
}

/// `due`, but a cluster whose class has its `triggers.session` entry off
/// starts nothing. The cluster stays captured.
pub fn due_gated(store: &Store, event_id: i64, triggers: &Triggers) -> Result<Vec<Due>> {
    let clusters = evidence::load(store)?;
    let mut out = Vec::new();
    for cluster in evidence::reached_threshold(&clusters, event_id) {
        if cluster.rejection.is_some() || store.proposal(&cluster.id)?.is_some() {
            continue;
        }
        if !session_trigger_on(cluster.class, triggers) {
            continue;
        }
        let Some(event) = cluster
            .occurrences
            .iter()
            .find(|o| o.event.id == event_id)
            .map(|o| &o.event)
        else {
            continue;
        };
        let repo = event
            .repo
            .as_deref()
            .map(Path::new)
            .filter(|repo| repo.is_absolute())
            .and_then(config::repo_root);
        if let Some(repo) = repo {
            out.push(Due {
                cluster: cluster.id.clone(),
                repo,
                host: event.host.clone(),
            });
        }
    }
    Ok(out)
}

/// Triages one cluster under its lock and queues the answer. `ask` runs the
/// host; the lock is released whether or not it succeeds.
pub fn triage_queued(
    store: &Store,
    inputs: &Inputs,
    id: &str,
    ask: &dyn Fn(&Cluster) -> Result<Answer>,
    prove: &dyn Fn(&Rule) -> Replayed,
    now: i64,
) -> Result<Ran> {
    if !store.try_lock_triage(id, now, TRIAGE_LOCK_STALE_MS)? {
        return Ok(Ran::Locked);
    }
    let ran = triage_locked(store, inputs, id, ask, prove, now);
    let unlocked = store.unlock_triage(id);
    let ran = ran?;
    unlocked?;
    Ok(ran)
}

fn triage_locked(
    store: &Store,
    inputs: &Inputs,
    id: &str,
    ask: &dyn Fn(&Cluster) -> Result<Answer>,
    prove: &dyn Fn(&Rule) -> Replayed,
    now: i64,
) -> Result<Ran> {
    let clusters = evidence::load(store)?;
    let cluster = evidence::find(&clusters, id)?;
    if cluster.rejection.is_some() || store.proposal(id)?.is_some() {
        return Ok(Ran::Skipped);
    }
    let answer = ask(cluster)?;
    record(store, inputs, cluster, &answer, prove, now).map(Ran::Recorded)
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
    record_answer(store, inputs, cluster, answer, prove, ts, false)
}

/// `record` for a triage the user ran by hand: a cluster queued only as a
/// waiting finding takes the answer's draft as its proposal, replacing the
/// finding. Without a draft the finding stays.
pub fn record_upgrading(
    store: &Store,
    inputs: &Inputs,
    cluster: &Cluster,
    answer: &Answer,
    prove: &dyn Fn(&Rule) -> Replayed,
    ts: i64,
) -> Result<Recorded> {
    record_answer(store, inputs, cluster, answer, prove, ts, true)
}

fn record_answer(
    store: &Store,
    inputs: &Inputs,
    cluster: &Cluster,
    answer: &Answer,
    prove: &dyn Fn(&Rule) -> Replayed,
    ts: i64,
    upgrade: bool,
) -> Result<Recorded> {
    if cluster.rejection.is_some() {
        return Ok(Recorded::Rejected);
    }
    let replaces = match store.proposal(&cluster.id)? {
        None => false,
        Some(queued)
            if upgrade && queued.kind == Kind::Finding && queued.state == State::Waiting =>
        {
            if answer.draft.is_none() {
                return Ok(Recorded::FindingKept);
            }
            true
        }
        Some(_) => return Ok(Recorded::AlreadyQueued),
    };
    let repo = config::repo_root(&inputs.cwd).context("not inside a git repository")?;
    let row = ProposalRow {
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
        target: None,
        commit: None,
        pr_url: None,
        hits: None,
    };
    let class = answer.verdict.mistake_class.as_deref();
    let text = answer
        .draft
        .as_ref()
        .map_or(answer.verdict.root_cause.as_str(), |draft| {
            draft.rationale.as_str()
        });
    let mark = possible_duplicate(store, &cluster.id, class, Some(text))?.map(|m| m.to_string());
    let Some(answered) = &answer.draft else {
        store.insert_proposal(&row)?;
        store.set_proposal_marks(&cluster.id, class, mark.as_deref())?;
        return Ok(Recorded::Finding);
    };
    let mut steered = answered.clone();
    if steered.engine == Engine::Steer && steered.prompt.is_none() {
        steer_fixtures(store, &repo, cluster, &mut steered)?;
    }
    let draft = &steered;
    let existing = load_rules(inputs).context("the current rules do not load")?;
    if existing.iter().any(|resolved| resolved.rule.id == draft.id) {
        bail!("a rule named `{}` already exists", draft.id);
    }
    let dir = draft.write(&PROJECT_RULES.iter().fold(repo, |dir, part| dir.join(part)))?;
    let queued = queue_written(store, inputs, draft, &dir, row, prove, replaces);
    if !matches!(queued, Ok(Recorded::Proposed { .. })) {
        let _ = std::fs::remove_dir_all(&dir);
    } else {
        store.set_proposal_marks(&cluster.id, class, mark.as_deref())?;
    }
    queued
}

/// The first queued or rejected proposal that `class` or `text` matches, other
/// than `id` itself. Matching is advisory: the user decides what to merge.
pub fn possible_duplicate(
    store: &Store,
    id: &str,
    class: Option<&str>,
    text: Option<&str>,
) -> Result<Option<Mark>> {
    let rows = store.mark_candidates()?;
    let others: Vec<Candidate> = rows
        .iter()
        .filter(|row| row.cluster != id)
        .map(|row| Candidate {
            id: &row.cluster,
            class: row.mistake_class.as_deref(),
            text: row.rationale.as_deref(),
        })
        .collect();
    Ok(first_match(&Candidate { id, class, text }, &others))
}

/// The context block for the triage prompt: loaded rule ids with descriptions,
/// and past rejection reasons. Both lists are bounded by `context_block`. A rule
/// that fails to load is left out, so a broken rule never blocks triage.
pub fn triage_context(store: &Store, inputs: &Inputs) -> Result<String> {
    let rules: Vec<(String, String)> = match rules::load_skipping_broken(inputs) {
        Ok((resolved, _)) => resolved
            .into_iter()
            .map(|entry| (entry.id, entry.rule.description))
            .collect(),
        Err(_) => Vec::new(),
    };
    let rejections: Vec<String> = store
        .rejections()?
        .into_iter()
        .map(|rejection| rejection.reason)
        .collect();
    Ok(context_block(&rules, &rejections))
}

/// `triage_context` for one cluster, followed by the mistake classes triage
/// named on other clusters, so the same mistake phrased differently reuses a
/// class and counts as recurring.
pub fn triage_context_for(store: &Store, inputs: &Inputs, cluster: &str) -> Result<String> {
    let classes: Vec<KnownClass> = store
        .mistake_classes(cluster)?
        .into_iter()
        .map(|(class, clusters, verdict)| KnownClass {
            class,
            clusters,
            root_cause: serde_json::from_str::<serde_json::Value>(&verdict)
                .ok()
                .and_then(|verdict| verdict["root_cause"].as_str().map(str::to_owned))
                .unwrap_or_default(),
        })
        .collect();
    Ok(format!(
        "{}{}",
        triage_context(store, inputs)?,
        classes_block(&classes)
    ))
}

/// Lines that show a proposal's duplicate marks in `list`, `show` and PR bodies.
pub fn mark_lines(marks: &ProposalMarks) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(possible) = &marks.possible_duplicate {
        lines.push(possible.clone());
    }
    if let Some(of) = &marks.duplicate_of {
        lines.push(format!("confirmed duplicate of {of}"));
    }
    lines
}

/// Fills a command steer draft's fixtures from a recorded command, which
/// becomes `bad`, and from the repo's other successful commands, which become
/// the corpus. Fails when the evidence holds no such command.
fn steer_fixtures(store: &Store, repo: &Path, cluster: &Cluster, draft: &mut Draft) -> Result<()> {
    let (tool, command) = if cluster.class == signal::USER_CORRECTION {
        corrected_command(store, cluster, draft)?
    } else {
        let latest = cluster
            .occurrences
            .iter()
            .map(|occurrence| &occurrence.event)
            .filter(|event| {
                event
                    .tool
                    .as_deref()
                    .is_some_and(crate::host::is_command_tool)
            })
            .max_by_key(|event| event.id)
            .context("a steer draft needs a cluster whose evidence is a command")?;
        (
            latest.tool.clone().unwrap_or_default(),
            latest.summary.clone(),
        )
    };
    let corpus: Vec<String> = store
        .successful_commands(&repo_key(repo), &command, STEER_CORPUS)?
        .into_iter()
        .map(|event| event.summary)
        .collect();
    draft.with_recorded_command(&tool, &command, &corpus)
}

/// Proves and queues the rule directory `dir`. The caller removes `dir`
/// unless this returns `Proposed`.
/// The recorded command a correction's steer draft is about: the latest
/// command of the corrected turns that the draft's own `bad` command, turned
/// into a trigger, matches. The model picks the command; the store supplies it.
fn corrected_command(store: &Store, cluster: &Cluster, draft: &Draft) -> Result<(String, String)> {
    let named = draft
        .bad_command()
        .context("a steer draft's bad fixture needs a command")?;
    let trigger = regex::Regex::new(&command_trigger(&named))
        .context("the drafted bad command gives no trigger")?;
    corrected_turns(store, cluster)?
        .into_iter()
        .flat_map(|turn| turn.actions)
        .rfind(|(tool, summary)| crate::host::is_command_tool(tool) && trigger.is_match(summary))
        .context(
            "a steer draft for a correction needs a recorded command in a corrected turn that its bad fixture matches",
        )
}

fn queue_written(
    store: &Store,
    inputs: &Inputs,
    draft: &Draft,
    dir: &Path,
    mut row: ProposalRow,
    prove: &dyn Fn(&Rule) -> Replayed,
    replaces: bool,
) -> Result<Recorded> {
    if let Err(error) = load_rules(inputs) {
        return Err(error.context(format!("proposal `{}` breaks the rule set", draft.id)));
    }
    let rule = rule::load_rule(dir)?;
    let (proven, note) = proven(&rule, &prove(&rule));
    row.kind = Kind::Proposal;
    row.rule = Some(draft.id.clone());
    row.dir = Some(dir.to_path_buf());
    row.proven = proven;
    row.note = note;
    row.rationale = Some(draft.rationale.trim().to_owned());
    // A rule whose hits fail to count stays unmeasured; it never blocks a proposal.
    row.hits = config::repo_root(&inputs.cwd)
        .and_then(|repo| hits::measure(inputs, &repo, dir).ok().flatten());
    let queued = if replaces {
        store.replace_finding(&row)?
    } else {
        store.insert_proposal(&row)?
    };
    if !queued {
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
    let dir = row
        .dir
        .clone()
        .context("the proposal names no rule directory")?;
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
    record_accepted(store, id, &row, &rule.id)?;
    Ok(dir)
}

/// Accepts a queued steer proposal with `action = "deny"`. The rewritten rule
/// must load, activate and prove under `prove`; otherwise the file is restored
/// and the proposal stays waiting. Denying needs a tool trigger.
pub fn accept_deny<F>(store: &Store, id: &str, prove: F) -> Result<PathBuf>
where
    F: Fn(&Rule) -> Replayed,
{
    let row = find(store, id)?;
    if row.kind == Kind::Finding {
        bail!("`{id}` is a finding; it has no rule to accept");
    }
    if row.state != State::Waiting {
        bail!("proposal `{id}` is already {}", row.state.as_str());
    }
    let dir = row
        .dir
        .clone()
        .context("the proposal names no rule directory")?;
    let rule = rule::load_rule(&dir)?;
    let Check::Steer(steer) = &rule.check else {
        bail!("`{id}` is not a steer rule; --deny applies only to steer proposals");
    };
    if matches!(steer.trigger, Trigger::Prompt { .. }) {
        bail!("`{id}` is a prompt steer rule; --deny needs a tool trigger, and a prompt rule can only inform");
    }
    if rule.status != Status::Proposed {
        bail!(
            "rule `{}` is {}, not proposed",
            rule.id,
            rule.status.as_str()
        );
    }
    let original = std::fs::read_to_string(&rule.file)
        .with_context(|| format!("reading {}", rule.file.display()))?;
    let denied = deny_action(&original)
        .with_context(|| format!("{} has no [steer] table", rule.file.display()))?;
    let flipped = activate(&denied)
        .with_context(|| format!("{} has no `{PROPOSED_LINE}` line", rule.file.display()))?;
    let restore = |error: anyhow::Error| -> anyhow::Error {
        match std::fs::write(&rule.file, &original) {
            Ok(()) => error,
            Err(write) => error.context(format!("restoring {}: {write}", rule.file.display())),
        }
    };
    std::fs::write(&rule.file, &flipped)
        .with_context(|| format!("writing {}", rule.file.display()))?;
    let active = match rule::load_rule(&dir) {
        Ok(active) => active,
        Err(error) => return Err(restore(error)),
    };
    let (proven_now, note) = proven(&active, &prove(&active));
    if !proven_now {
        let reason = note.unwrap_or_else(|| "no valid proof".to_owned());
        return Err(restore(anyhow::anyhow!(
            "steer rule `{}` with deny did not prove: {reason}",
            active.id
        )));
    }
    record_accepted(store, id, &row, &active.id)?;
    Ok(dir)
}

/// Marks an accepted proposal in the store and writes its decision record.
fn record_accepted(store: &Store, id: &str, row: &ProposalRow, rule_id: &str) -> Result<()> {
    store.set_proposal_state(id, State::Accepted)?;
    evidence::decide(
        store,
        id,
        Outcome::Accepted,
        Some(rule_id),
        None,
        now_millis()?,
        row.occurrences.len(),
    )
}

/// Rejects a queued result. The cluster is rejected in the evidence store, so
/// triage does not propose it again, and a still-proposed rule is removed.
/// The cluster's events are deleted; its decision record keeps the counts.
pub fn reject(store: &Store, id: &str, reason: &str, ts: i64) -> Result<()> {
    reject_with_reason(store, id, reason, ts)
}

/// Rejects `id` as a duplicate of `other`, which must be a queued or rejected
/// proposal. The link is kept for the user; nothing merges automatically.
pub fn reject_duplicate(store: &Store, id: &str, other: &str, ts: i64) -> Result<()> {
    if id == other {
        bail!("proposal `{id}` cannot be a duplicate of itself");
    }
    if store.proposal(other)?.is_none() {
        bail!("no proposal `{other}`; see {LIST_HINT}");
    }
    reject_with_reason(store, id, &format!("duplicate of {other}"), ts)?;
    store.set_duplicate_of(id, other)
}

fn reject_with_reason(store: &Store, id: &str, reason: &str, ts: i64) -> Result<()> {
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
    store.set_proposal_state(id, State::Rejected)?;
    evidence::decide(
        store,
        id,
        Outcome::Rejected,
        row.rule.as_deref(),
        Some(reason.trim()),
        ts,
        row.occurrences.len(),
    )
}

/// The rule proposals waiting in `repo`, oldest first. Findings do not count.
pub fn waiting(store: &Store, repo: &Path) -> Result<Vec<ProposalRow>> {
    let key = repo_key(repo);
    let mut rows = store.proposals()?;
    rows.retain(|row| row.kind == Kind::Proposal && row.state == State::Waiting && row.repo == key);
    Ok(rows)
}

/// `waiting` for the repository holding `inputs.cwd`. Empty outside a
/// repository and when no store exists yet; it never creates one.
pub fn waiting_here(inputs: &Inputs) -> Result<Vec<ProposalRow>> {
    let path = crate::store::events_db_path_for(&inputs.vars, config::Platform::current())?;
    let Some(repo) = config::repo_root(&inputs.cwd).filter(|_| path.is_file()) else {
        return Ok(Vec::new());
    };
    waiting(&Store::open(&path)?, &repo)
}

/// The non-blocking line the gate prints when proposals wait in `repo`.
pub fn notice(store: &Store, repo: &Path) -> Result<Option<String>> {
    Ok(notice_line(waiting(store, repo)?.len()))
}

pub fn notice_line(waiting: usize) -> Option<String> {
    match waiting {
        0 => None,
        1 => Some(format!("1 rule proposal waiting: {LIST_HINT}")),
        n => Some(format!("{n} rule proposals waiting: {LIST_HINT}")),
    }
}

/// The key a repo is queued under: its canonical path when it resolves.
pub fn repo_key(repo: &Path) -> String {
    std::fs::canonicalize(repo)
        .unwrap_or_else(|_| repo.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

pub fn find(store: &Store, id: &str) -> Result<ProposalRow> {
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

fn now_millis() -> Result<i64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the unix epoch")?
        .as_millis() as i64)
}

/// Whether the replay left a valid proof, and why not when it did not.
pub fn proven(rule: &Rule, replayed: &Replayed) -> (bool, Option<String>) {
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

/// `text` with its `[steer]` table's action set to deny, replacing any action
/// line there. `None` when the file has no `[steer]` table.
fn deny_action(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len() + DENY_ACTION.len() + 2);
    let mut in_steer = false;
    let mut found = false;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let trimmed = body.trim();
        if trimmed.starts_with('[') {
            in_steer = trimmed == "[steer]";
            out.push_str(line);
            if in_steer {
                found = true;
                if !line.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(DENY_ACTION);
                out.push('\n');
            }
        } else if in_steer && trimmed.starts_with("action") && trimmed.contains('=') {
            continue;
        } else {
            out.push_str(line);
        }
    }
    found.then_some(out)
}
