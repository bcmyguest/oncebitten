//! `oncebitten proposals deliver`: commits a proposed rule to the current
//! branch (`pr`) or to a branch of the writable rules source (`rules-repo`).
//! Delivery never sets a rule's status; `proposals accept` does.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::baseline;
use crate::config::{self, Inputs, Platform};
use crate::exceptions;
use crate::forge::{self, Forge, NewPr};
use crate::hits;
use crate::proof::{self, Replayed, Validity};
use crate::proposals;
use crate::registry::Layer;
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
    let chosen = choose(inputs, to, &repo, &config, forge_for)?;
    let marks = store.proposal_marks(&row.cluster)?.unwrap_or_default();
    let message = Message {
        rule: rule.id.clone(),
        cluster: row.cluster.clone(),
        rationale: row.rationale.clone().unwrap_or_default(),
        proof: proof.clone(),
        hits: row.hits.clone(),
        rules_repo: matches!(chosen, Chosen::RulesRepo),
        marks: proposals::mark_lines(&marks),
    };
    let committed = match chosen {
        Chosen::Pr(pr_url) => {
            let branch = current_branch(inputs, &repo)?;
            let moved = promote_local_rule(store, inputs, id, &rule.id, &repo, &dir)?;
            let result = match &moved {
                Some(promoted) => commit_promoted(inputs, &repo, promoted, &message),
                None => commit_paths(inputs, &repo, std::slice::from_ref(&dir), &message),
            };
            let commit = match result {
                Ok(commit) => commit,
                Err(mut error) => {
                    if let Some(promoted) = moved {
                        if let Err(unstaged) = unstage(inputs, &repo, &promoted.paths()) {
                            error = error.context(format!("unstaging failed too: {unstaged:#}"));
                        }
                        if let Err(restore) = restore_local_rule(store, id, &promoted, &dir) {
                            return Err(
                                restore.context(format!("the commit failed too: {error:#}"))
                            );
                        }
                    }
                    return Err(error);
                }
            };
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

/// A local rule moved into the project layer, with the baseline and exception
/// files it dragged along.
struct Promoted {
    dir: PathBuf,
    /// Files the move may rewrite, with their content before it.
    saved: Vec<(PathBuf, Option<String>)>,
    changed: Vec<Changed>,
}

/// A file the move rewrote.
struct Changed {
    path: PathBuf,
    /// HEAD's content plus only the promoted rule's records.
    committed: String,
    /// The working content with the promotion applied.
    full: String,
}

impl Promoted {
    /// What the delivery commit stages.
    fn paths(&self) -> Vec<PathBuf> {
        std::iter::once(self.dir.clone())
            .chain(self.changed.iter().map(|changed| changed.path.clone()))
            .collect()
    }
}

/// Commits the rule directory and the rewritten files at their `committed`
/// content, so unrelated uncommitted edits stay out of the commit, then puts
/// the full content back. A failed commit leaves the files for
/// `restore_local_rule`.
fn commit_promoted(
    inputs: &Inputs,
    repo: &Path,
    promoted: &Promoted,
    message: &Message,
) -> Result<String> {
    for changed in &promoted.changed {
        fs::write(&changed.path, &changed.committed)
            .with_context(|| format!("writing {}", changed.path.display()))?;
    }
    let commit = commit_paths(inputs, repo, &promoted.paths(), message)?;
    for changed in &promoted.changed {
        fs::write(&changed.path, &changed.full).with_context(|| {
            format!(
                "committed {commit}, but restoring {} failed",
                changed.path.display()
            )
        })?;
    }
    Ok(commit)
}

/// A rule delivered to the team's own repo leaves the local layer and joins
/// the shared project rules, taking its baseline entries and exceptions
/// along. Returns the move, or `None` when the rule is not under `rules.local`.
fn promote_local_rule(
    store: &Store,
    inputs: &Inputs,
    id: &str,
    rule_id: &str,
    repo: &Path,
    dir: &Path,
) -> Result<Option<Promoted>> {
    let local_root = repo.join(".oncebitten").join("rules.local");
    let in_local = dir
        .parent()
        .and_then(|parent| {
            Some(fs::canonicalize(parent).ok()? == fs::canonicalize(&local_root).ok()?)
        })
        .unwrap_or(false);
    if !in_local {
        return Ok(None);
    }
    let name = dir.file_name().context("the rule directory has no name")?;
    let rules = repo.join(".oncebitten").join("rules");
    let target = rules.join(name);
    if target.exists() {
        bail!(
            "{} already exists; remove it before delivering `{id}`",
            target.display()
        );
    }
    let in_repo = Inputs {
        cwd: repo.to_path_buf(),
        vars: inputs.vars.clone(),
    };
    let clash = proposals::load_rules(&in_repo)?
        .into_iter()
        .find(|entry| entry.layer != Layer::Local && entry.rule.id == rule_id);
    if let Some(clash) = clash {
        bail!(
            "`{rule_id}` is also the id of `{}`; committing it would break the rule set. Rename the local rule first",
            clash.id
        );
    }
    fs::create_dir_all(&rules).with_context(|| format!("creating {}", rules.display()))?;
    fs::rename(dir, &target)
        .with_context(|| format!("moving {} to {}", dir.display(), target.display()))?;
    if let Err(error) = store.set_proposal_dir(id, &target) {
        return Err(match fs::rename(&target, dir) {
            Ok(()) => error,
            Err(back) => error.context(format!(
                "moving {} back to {} failed too: {back}",
                target.display(),
                dir.display()
            )),
        });
    }
    let mut promoted = Promoted {
        dir: target,
        saved: Vec::new(),
        changed: Vec::new(),
    };
    if let Err(error) = move_records(inputs, repo, rule_id, &mut promoted) {
        return Err(match restore_local_rule(store, id, &promoted, dir) {
            Ok(()) => error,
            Err(restore) => restore.context(format!("moving the records failed too: {error:#}")),
        });
    }
    Ok(Some(promoted))
}

/// Moves the rule's baseline entries and exceptions from `local/<id>` to
/// `<id>`, recording each file it rewrites in `promoted`.
fn move_records(
    inputs: &Inputs,
    repo: &Path,
    rule_id: &str,
    promoted: &mut Promoted,
) -> Result<()> {
    let files = [
        baseline::path(repo),
        baseline::local_path(repo),
        exceptions::path(repo),
    ];
    for file in &files {
        promoted
            .saved
            .push((file.clone(), fs::read_to_string(file).ok()));
    }
    let shared = baseline::path(repo);
    if let Some(moved) = baseline::promote(repo, rule_id)? {
        let mut head = match head_text(inputs, repo, &shared)? {
            Some(text) => baseline::parse(&text).context("parsing the committed baseline.json")?,
            None => baseline::Baseline::default(),
        };
        head.entries.extend(moved);
        promoted.changed.push(Changed {
            committed: baseline::render(&head)?,
            full: fs::read_to_string(&shared)
                .with_context(|| format!("reading {}", shared.display()))?,
            path: shared,
        });
    }
    let local_id = format!("{}{rule_id}", baseline::LOCAL_PREFIX);
    let file = exceptions::path(repo);
    let head = head_text(inputs, repo, &file)?.unwrap_or_default();
    if let Some(moved) = exceptions::move_rule(repo, &local_id, rule_id, &head)? {
        fs::write(&file, &moved.full).with_context(|| format!("writing {}", file.display()))?;
        promoted.changed.push(Changed {
            path: file,
            committed: moved.committed,
            full: moved.full,
        });
    }
    Ok(())
}

/// `path`'s content at HEAD, or `None` when HEAD does not have it.
fn head_text(inputs: &Inputs, repo: &Path, path: &Path) -> Result<Option<String>> {
    let spec = pathspec_of_missing(repo, path)?;
    let listed = sources::git(
        &inputs.vars,
        repo,
        &["ls-tree", "--name-only", "HEAD", "--", &spec],
    )?;
    if listed.is_empty() {
        return Ok(None);
    }
    Ok(Some(sources::git(
        &inputs.vars,
        repo,
        &["show", &format!("HEAD:{spec}")],
    )?))
}

/// Moves a promoted rule back to the local layer, and its baseline and
/// exception files back to their earlier content, after a failed commit.
fn restore_local_rule(store: &Store, id: &str, promoted: &Promoted, dir: &Path) -> Result<()> {
    for (file, text) in &promoted.saved {
        match text {
            Some(text) => fs::write(file, text),
            None if file.exists() => fs::remove_file(file),
            None => Ok(()),
        }
        .with_context(|| format!("restoring {}", file.display()))?;
    }
    fs::rename(&promoted.dir, dir).with_context(|| {
        format!(
            "moving {} back to {}",
            promoted.dir.display(),
            dir.display()
        )
    })?;
    store.set_proposal_dir(id, dir)
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
    /// The rule goes to the shared rules repo, whose other consumers get no
    /// baseline from `accept`.
    rules_repo: bool,
    marks: Vec<String>,
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
            if self.rules_repo {
                body.push_str(&format!(
                    "existing hits will be baselined only in the proposal's repo, by `oncebitten proposals accept {}`; other repos using this rule get no baseline\n",
                    self.cluster
                ));
            } else {
                body.push_str("existing hits will be baselined\n");
            }
        }
        for line in &self.marks {
            body.push_str(&format!("{line}\n"));
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
    let commit = commit_paths(inputs, &clone, std::slice::from_ref(&target), message)?;
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

/// Stages and commits only `paths`, returning the new commit's sha.
fn commit_paths(
    inputs: &Inputs,
    repo: &Path,
    paths: &[PathBuf],
    message: &Message,
) -> Result<String> {
    let specs = pathspecs(repo, paths)?;
    let spec_list = specs.join("`, `");
    sources::git(&inputs.vars, repo, &with_specs(&["add"], &specs))?;
    let pending = sources::git(
        &inputs.vars,
        repo,
        &with_specs(&["status", "--porcelain"], &specs),
    )?;
    if pending.is_empty() {
        bail!("nothing to commit: `{spec_list}` matches what the repository already has");
    }
    let subject = message.subject();
    let body = message.body();
    sources::git(
        &inputs.vars,
        repo,
        &with_specs(&["commit", "--quiet", "-m", &subject, "-m", &body], &specs),
    )?;
    sources::git(&inputs.vars, repo, &["rev-parse", "HEAD"])
}

/// Removes `paths` from the index, leaving the working tree alone.
fn unstage(inputs: &Inputs, repo: &Path, paths: &[PathBuf]) -> Result<()> {
    let specs = pathspecs(repo, paths)?;
    sources::git(&inputs.vars, repo, &with_specs(&["reset", "-q"], &specs))?;
    Ok(())
}

fn pathspecs(repo: &Path, paths: &[PathBuf]) -> Result<Vec<String>> {
    paths.iter().map(|path| pathspec(repo, path)).collect()
}

/// `args`, then `--` and the pathspecs.
fn with_specs<'a>(args: &[&'a str], specs: &'a [String]) -> Vec<&'a str> {
    let mut all = args.to_vec();
    all.push("--");
    all.extend(specs.iter().map(String::as_str));
    all
}

/// `pathspec` for a file that may not exist yet, `/`-separated.
fn pathspec_of_missing(repo: &Path, file: &Path) -> Result<String> {
    let parent = file.parent().context("the file has no directory")?;
    let name = file.file_name().context("the file has no name")?;
    let spec = pathspec(repo, parent)?;
    Ok(Path::new(&spec)
        .join(name)
        .to_string_lossy()
        .replace('\\', "/"))
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
