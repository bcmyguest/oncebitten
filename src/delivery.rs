//! `oncebitten proposals deliver`: commits a proposed rule to the current
//! branch (`pr`) or to a branch of the writable rules source (`rules-repo`).
//! Delivery never sets a rule's status; `proposals accept` does.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::{self, Inputs, Platform};
use crate::forge::{self, Forge, NewPr};
use crate::hits;
use crate::proof::{self, Replayed, Validity};
use crate::proposals;
use crate::rule::{self, Rule};
use crate::sources;
use crate::store::{DeliveryRow, HitsRow, Kind, State, Store};

const BRANCH_PREFIX: &str = "oncebitten/propose-";
const WRITABLE_KEY: &str = "rules.sources[].writable";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Pr,
    RulesRepo,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::Pr => "pr",
            Target::RulesRepo => "rules-repo",
        }
    }
}

/// Builds the forge backend for a kind, running its CLI from a directory.
pub type ForgeFor<'a> = &'a dyn Fn(forge::Kind, &Path) -> Box<dyn Forge>;

/// What one delivery did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    pub target: Target,
    pub commit: String,
    /// The `rules-repo` branch; `None` for `pr`.
    pub branch: Option<String>,
    pub pr_url: Option<String>,
    pub pushed: bool,
    /// The command that publishes a `rules-repo` branch that was not pushed.
    pub push_hint: Option<String>,
    pub proof: Proof,
}

/// The proof state a delivery states in its commit message and PR body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proof {
    Proven,
    Unproven(String),
}

impl Proof {
    fn text(&self) -> String {
        match self {
            Proof::Proven => "proven".to_owned(),
            Proof::Unproven(reason) => format!("unproven: {reason}"),
        }
    }
}

pub fn deliver(
    store: &Store,
    inputs: &Inputs,
    id: &str,
    to: Option<Target>,
    push: bool,
    prove: &dyn Fn(&Rule) -> Replayed,
    forge_for: ForgeFor,
) -> Result<Delivered> {
    let row = proposals::find(store, id)?;
    if row.kind == Kind::Finding {
        bail!("`{id}` is a finding; it has no rule to deliver");
    }
    if row.state == State::Rejected {
        bail!("proposal `{id}` is rejected; it cannot be delivered");
    }
    let dir = row
        .dir
        .clone()
        .context("the proposal names no rule directory")?;
    let repo = PathBuf::from(&row.repo);
    let mut rule =
        rule::load_rule(&dir).with_context(|| format!("loading the rule at {}", dir.display()))?;
    let proof = settle_proof(store, id, &mut rule, prove)?;
    let config = config::load(inputs)?.config;
    let message = Message {
        rule: rule.id.clone(),
        cluster: row.cluster.clone(),
        rationale: row.rationale.clone().unwrap_or_default(),
        proof: proof.clone(),
        hits: row.hits.clone(),
    };
    let committed = match choose(inputs, to, &repo, &config, forge_for)? {
        Chosen::Pr(pr_url) => {
            let branch = current_branch(inputs, &repo)?;
            let commit = commit_dir(inputs, &repo, &dir, &message)?;
            Committed {
                target: Target::Pr,
                commit,
                workdir: repo.clone(),
                branch,
                source_rev: None,
                pr_url,
            }
        }
        Chosen::RulesRepo => commit_to_rules_repo(inputs, &config, &dir, &message)?,
    };
    let mut delivered = Delivered {
        target: committed.target,
        commit: committed.commit.clone(),
        branch: (committed.target == Target::RulesRepo).then(|| committed.branch.clone()),
        pr_url: committed.pr_url.clone(),
        pushed: false,
        push_hint: None,
        proof,
    };
    record(store, id, &delivered)?;
    if push {
        let opened = publish(inputs, &config, &committed, &message, forge_for)?;
        delivered.pushed = true;
        delivered.pr_url = opened.or(delivered.pr_url);
        record(store, id, &delivered)?;
    } else if committed.target == Target::RulesRepo {
        delivered.push_hint = Some(format!(
            "git -C {} push -u origin {}",
            committed.workdir.display(),
            committed.branch
        ));
    }
    Ok(delivered)
}

fn record(store: &Store, id: &str, delivered: &Delivered) -> Result<()> {
    store.set_proposal_delivery(
        id,
        &DeliveryRow {
            target: delivered.target.as_str().to_owned(),
            commit: delivered.commit.clone(),
            pr_url: delivered.pr_url.clone(),
        },
    )
}

/// A commit made, and where it lives.
struct Committed {
    target: Target,
    commit: String,
    /// The working tree the commit is in.
    workdir: PathBuf,
    branch: String,
    /// The source's configured `rev`, the PR base when origin names no default.
    source_rev: Option<String>,
    pr_url: Option<String>,
}

/// Replays a rule whose proof is missing or stale and states the outcome.
fn settle_proof(
    store: &Store,
    id: &str,
    rule: &mut Rule,
    prove: &dyn Fn(&Rule) -> Replayed,
) -> Result<Proof> {
    if proof::validity(rule) == Validity::Valid {
        return Ok(Proof::Proven);
    }
    let replayed = prove(rule);
    *rule = rule::load_rule(rule.file.parent().context("rule file has no directory")?)?;
    let (proven, note) = proposals::proven(rule, &replayed);
    store.set_proposal_proof(id, proven, note.as_deref())?;
    Ok(match (proven, note) {
        (true, _) => Proof::Proven,
        (false, note) => Proof::Unproven(note.unwrap_or_else(|| "no valid proof".to_owned())),
    })
}

struct Message {
    rule: String,
    cluster: String,
    rationale: String,
    proof: Proof,
    hits: Option<HitsRow>,
}

impl Message {
    fn subject(&self) -> String {
        format!("chore(oncebitten): propose rule {}", self.rule)
    }

    fn body(&self) -> String {
        let mut body = format!("Cluster: {}\n", self.cluster);
        if !self.rationale.is_empty() {
            body.push_str(&format!("Rationale: {}\n", self.rationale));
        }
        body.push_str(&format!("Proof: {}\n", self.proof.text()));
        if let Some(hits) = &self.hits {
            body.push_str(&format!("Hits: {}\n", hits::summary_text(hits)));
            body.push_str("existing hits will be baselined\n");
        }
        body
    }
}

enum Chosen {
    /// The open PR's URL, when the forge was asked.
    Pr(Option<String>),
    RulesRepo,
}

/// `--to` wins. Otherwise: `pr` when the forge reports an open PR for the
/// current branch, else `rules-repo`. A forge failure falls back to
/// `rules-repo` when a writable source exists, and is the error otherwise.
fn choose(
    inputs: &Inputs,
    to: Option<Target>,
    repo: &Path,
    config: &config::Config,
    forge_for: ForgeFor,
) -> Result<Chosen> {
    match to {
        Some(Target::Pr) => return Ok(Chosen::Pr(None)),
        Some(Target::RulesRepo) => return Ok(Chosen::RulesRepo),
        None => {}
    }
    let found = current_branch(inputs, repo).and_then(|branch| {
        let kind = forge::select(config.forge, forge::origin_url(repo)?.as_deref())?;
        forge_for(kind, repo).find_open(&branch)
    });
    match found {
        Ok(Some(pr)) => Ok(Chosen::Pr(Some(pr.url))),
        Ok(None) => Ok(Chosen::RulesRepo),
        Err(_) if config.rules.sources.iter().any(|source| source.writable) => {
            Ok(Chosen::RulesRepo)
        }
        Err(error) => Err(error.context(format!(
            "cannot pick a target: no writable source (`{WRITABLE_KEY}`); pass --to"
        ))),
    }
}

fn current_branch(inputs: &Inputs, repo: &Path) -> Result<String> {
    let branch = sources::git(&inputs.vars, repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if branch == "HEAD" {
        bail!("HEAD is detached; check out a branch first");
    }
    Ok(branch)
}

/// Copies the rule into the writable source's clone on a new branch and
/// commits it there. The clone stays on that branch until the next
/// `oncebitten rules sync`.
fn commit_to_rules_repo(
    inputs: &Inputs,
    config: &config::Config,
    dir: &Path,
    message: &Message,
) -> Result<Committed> {
    let source = config
        .rules
        .sources
        .iter()
        .find(|source| source.writable)
        .with_context(|| {
            format!("no writable rules source; set `{WRITABLE_KEY}` on one rules source")
        })?;
    let clone = sources::sources_root(&inputs.vars, Platform::current())?.join(&source.name);
    if !sources::is_cloned(&clone) {
        bail!(
            "source `{}` is not cloned; run oncebitten rules sync",
            source.name
        );
    }
    let branch = format!("{BRANCH_PREFIX}{}", message.rule);
    sources::git(
        &inputs.vars,
        &clone,
        &["checkout", "--quiet", "-b", &branch],
    )
    .with_context(|| format!("creating branch `{branch}` in {}", clone.display()))?;
    let name = dir.file_name().context("the rule directory has no name")?;
    let target = clone.join(name);
    if target.exists() {
        fs::remove_dir_all(&target).with_context(|| format!("replacing {}", target.display()))?;
    }
    copy_dir(dir, &target)?;
    let commit = commit_dir(inputs, &clone, &target, message)?;
    Ok(Committed {
        target: Target::RulesRepo,
        commit,
        workdir: clone,
        branch,
        source_rev: source.rev.clone(),
        pr_url: None,
    })
}

/// Pushes the commit. A `rules-repo` push also opens a PR through the forge
/// of the clone's origin and returns its URL.
fn publish(
    inputs: &Inputs,
    config: &config::Config,
    committed: &Committed,
    message: &Message,
    forge_for: ForgeFor,
) -> Result<Option<String>> {
    let dir = &committed.workdir;
    if committed.target == Target::Pr {
        sources::git(&inputs.vars, dir, &["push"])?;
        return Ok(None);
    }
    sources::git(
        &inputs.vars,
        dir,
        &["push", "--set-upstream", "origin", &committed.branch],
    )?;
    let kind = forge::select(config.forge, forge::origin_url(dir)?.as_deref())?;
    let base = default_branch(inputs, dir, committed.source_rev.as_deref())?;
    let pr = forge_for(kind, dir).open(&NewPr {
        head: committed.branch.clone(),
        base,
        title: message.subject(),
        body: message.body(),
    })?;
    Ok(Some(pr.url))
}

/// The branch a rules-repo PR targets: the source's `rev`, else origin's HEAD.
fn default_branch(inputs: &Inputs, clone: &Path, rev: Option<&str>) -> Result<String> {
    if let Some(rev) = rev {
        return Ok(rev.to_owned());
    }
    let head = sources::git(
        &inputs.vars,
        clone,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .context("cannot tell the PR base branch; set `rev` on the writable source")?;
    Ok(head.strip_prefix("origin/").unwrap_or(&head).to_owned())
}

/// Stages and commits only `dir`, returning the new commit's sha.
fn commit_dir(inputs: &Inputs, repo: &Path, dir: &Path, message: &Message) -> Result<String> {
    let spec = pathspec(repo, dir)?;
    sources::git(&inputs.vars, repo, &["add", "--", &spec])?;
    let pending = sources::git(&inputs.vars, repo, &["status", "--porcelain", "--", &spec])?;
    if pending.is_empty() {
        bail!("nothing to commit: `{spec}` matches what the repository already has");
    }
    sources::git(
        &inputs.vars,
        repo,
        &[
            "commit",
            "--quiet",
            "-m",
            &message.subject(),
            "-m",
            &message.body(),
            "--",
            &spec,
        ],
    )?;
    sources::git(&inputs.vars, repo, &["rev-parse", "HEAD"])
}

/// `dir` relative to the repo top, for a git pathspec.
fn pathspec(repo: &Path, dir: &Path) -> Result<String> {
    let repo = fs::canonicalize(repo).with_context(|| format!("resolving {}", repo.display()))?;
    let dir = fs::canonicalize(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let relative = dir
        .strip_prefix(&repo)
        .with_context(|| format!("{} is outside {}", dir.display(), repo.display()))?;
    Ok(relative.to_string_lossy().into_owned())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("copying {}", entry.path().display()))?;
        }
    }
    Ok(())
}
