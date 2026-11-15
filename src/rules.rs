use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use include_dir::{include_dir, Dir};
use tempfile::TempDir;

use crate::config::{self, Inputs, Platform};
use crate::lockfile::{self, LockedSource, Lockfile};
use crate::registry::{resolve, Layers, Resolved};
use crate::sources::{git, is_cloned, sources_root, sync_source};

const PROJECT_DIR: &str = ".oncebitten";
const PROJECT_RULES: &str = "rules";
static EXAMPLES: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/examples");

pub struct Loaded {
    pub rules: Vec<Resolved>,
    _examples: Option<TempDir>,
}

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

pub fn load(inputs: &Inputs, ci: bool) -> Result<Vec<Resolved>, Failure> {
    load_layers(inputs, ci, false).map(|loaded| loaded.rules)
}

pub fn load_for_check(inputs: &Inputs) -> Result<Loaded, Failure> {
    load_layers(inputs, false, true)
}

fn load_layers(inputs: &Inputs, ci: bool, with_examples: bool) -> Result<Loaded, Failure> {
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
    let examples = if with_examples {
        Some(examples_dir().map_err(Failure::CouldNotRun)?)
    } else {
        None
    };
    let layers = Layers {
        project: project.is_dir().then_some(project),
        sources,
        examples: examples.as_ref().map(|dir| dir.path().to_path_buf()),
    };
    let rules = resolve(&layers).map_err(Failure::Invalid)?;
    Ok(Loaded {
        rules,
        _examples: examples,
    })
}

fn examples_dir() -> Result<TempDir> {
    let dir = TempDir::new().context("creating a directory for the shipped examples")?;
    EXAMPLES
        .extract(dir.path())
        .context("extracting the shipped examples")?;
    Ok(dir)
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
