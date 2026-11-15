use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use include_dir::{include_dir, Dir};

use crate::config::{self, Inputs, Platform};
use crate::lockfile::{self, LockedSource, Lockfile};
use crate::registry::{resolve, Layer, Layers, Resolved};
use crate::rule::Status;
use crate::sources::{git, is_cloned, sources_root, sync_source};

const PROJECT_DIR: &str = ".oncebitten";
const PROJECT_RULES: &str = "rules";
/// Under the sources root; a leading dot keeps it apart from source names.
const SHIPPED_DIR: &str = ".shipped-examples";
static EXAMPLES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/examples");

#[derive(Debug)]
pub enum Failure {
    Invalid(Vec<anyhow::Error>),
    CouldNotRun(anyhow::Error),
}

pub fn sync(inputs: &Inputs) -> Result<()> {
    let loaded = config::load(inputs)?;
    let repo = repo_root(inputs)?;
    let root = sources_root(&inputs.vars, Platform::current())?;
    let mut lock = Lockfile::default();
    for source in &loaded.config.rules.sources {
        let commit = sync_source(
            &inputs.vars,
            &root,
            &source.name,
            &source.url,
            source.rev.as_deref(),
        )?;
        lock.source.push(LockedSource {
            name: source.name.clone(),
            url: source.url.clone(),
            commit,
        });
    }
    lockfile::write(&repo, &lock)
}

pub fn is_shipped_example(id: &str) -> bool {
    EXAMPLES
        .dirs()
        .any(|dir| dir.path() == std::path::Path::new(id))
}

pub fn load(inputs: &Inputs, ci: bool) -> Result<Vec<Resolved>, Failure> {
    let loaded = config::load(inputs).map_err(Failure::CouldNotRun)?;
    let repo = repo_root(inputs).map_err(Failure::CouldNotRun)?;
    let root = sources_root(&inputs.vars, Platform::current()).map_err(Failure::CouldNotRun)?;
    let lock = if ci {
        Some(lockfile::read(&repo).map_err(|error| {
            Failure::CouldNotRun(error.context("--ci needs rules.lock; run oncebitten rules sync"))
        })?)
    } else {
        None
    };
    let mut sources = Vec::new();
    for source in &loaded.config.rules.sources {
        let dir = root.join(&source.name);
        match &lock {
            Some(lock) => check_locked(inputs, lock, &source.name, &source.url, &dir)
                .map_err(Failure::CouldNotRun)?,
            None if !is_cloned(&dir) => {
                return Err(Failure::CouldNotRun(anyhow!(
                    "source `{}` is not cloned; run oncebitten rules sync",
                    source.name
                )));
            }
            None => {}
        }
        sources.push((source.name.clone(), dir));
    }
    let project = repo.join(PROJECT_DIR).join(PROJECT_RULES);
    let examples =
        examples_dir(&root, &loaded.config.rules.use_examples).map_err(Failure::CouldNotRun)?;
    let layers = Layers {
        project: project.is_dir().then_some(project),
        sources,
        examples,
    };
    let mut rules = resolve(&layers).map_err(Failure::Invalid)?;
    for resolved in &mut rules {
        if resolved.layer == Layer::Example {
            resolved.rule.status = Status::Active;
        }
    }
    Ok(rules)
}

/// Extracts the listed shipped examples to a directory that outlives `load`,
/// so the rule files stay readable for `check` and `replay`.
fn examples_dir(root: &std::path::Path, listed: &[String]) -> Result<Option<PathBuf>> {
    if listed.is_empty() {
        return Ok(None);
    }
    let dir = root.join(SHIPPED_DIR);
    for id in listed {
        let Some(example) = EXAMPLES
            .dirs()
            .find(|d| d.path() == std::path::Path::new(id))
        else {
            bail!("`{id}` names no shipped example");
        };
        std::fs::create_dir_all(dir.join(id))
            .with_context(|| format!("creating the directory for `{id}`"))?;
        example
            .extract(&dir)
            .with_context(|| format!("extracting the shipped example `{id}`"))?;
    }
    Ok(Some(dir))
}

fn check_locked(
    inputs: &Inputs,
    lock: &Lockfile,
    name: &str,
    url: &str,
    dir: &std::path::Path,
) -> Result<()> {
    let Some(entry) = lock.source.iter().find(|entry| entry.name == name) else {
        return Err(anyhow!("rules.lock has no entry for source `{name}`"));
    };
    if entry.url != url {
        return Err(anyhow!(
            "rules.lock pins `{}` for source `{name}`, but the config names `{url}`",
            entry.url
        ));
    }
    if !is_cloned(dir) {
        return Err(anyhow!(
            "source `{name}` is not cloned; run oncebitten rules sync"
        ));
    }
    let head = git(&inputs.vars, dir, &["rev-parse", "HEAD"])?;
    if head != entry.commit {
        return Err(anyhow!(
            "source `{name}` is at {head}, but rules.lock pins {}",
            entry.commit
        ));
    }
    Ok(())
}

fn repo_root(inputs: &Inputs) -> Result<PathBuf> {
    config::repo_root(&inputs.cwd).context("not inside a git repository")
}
