use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::Platform;

const APP_DIR: &str = "oncebitten";
const SOURCES_DIR: &str = "sources";
const GIT_CONFIG_GLOBAL: &str = "GIT_CONFIG_GLOBAL";

pub fn sources_root(vars: &BTreeMap<String, String>, platform: Platform) -> Result<PathBuf> {
    let base = match vars
        .get("XDG_DATA_HOME")
        .filter(|dir| platform.is_absolute(dir))
    {
        Some(dir) => PathBuf::from(dir),
        None => match platform {
            Platform::Windows => PathBuf::from(
                vars.get("LOCALAPPDATA")
                    .filter(|dir| platform.is_absolute(dir))
                    .context("neither XDG_DATA_HOME nor LOCALAPPDATA is an absolute path")?,
            ),
            Platform::Unix => PathBuf::from(
                vars.get("HOME")
                    .filter(|dir| platform.is_absolute(dir))
                    .context("neither XDG_DATA_HOME nor HOME is an absolute path")?,
            )
            .join(".local")
            .join("share"),
        },
    };
    Ok(base.join(APP_DIR).join(SOURCES_DIR))
}

pub fn is_cloned(dir: &Path) -> bool {
    dir.join(".git").exists()
}

pub fn git(vars: &BTreeMap<String, String>, dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    if let Some(config) = vars.get(GIT_CONFIG_GLOBAL) {
        command.env(GIT_CONFIG_GLOBAL, config);
    }
    let output = command
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn sync_source(
    vars: &BTreeMap<String, String>,
    root: &Path,
    name: &str,
    url: &str,
    rev: Option<&str>,
) -> Result<String> {
    let dir = root.join(name);
    if is_cloned(&dir) {
        git(vars, &dir, &["remote", "set-url", "origin", url])?;
        git(vars, &dir, &["fetch", "--quiet", "--prune", "origin"])?;
    } else {
        clone_source(vars, root, name, url)?;
    }
    let commit = resolve(vars, &dir, name, rev)?;
    git(vars, &dir, &["checkout", "--quiet", "--detach", &commit])?;
    Ok(commit)
}

/// Checks out `commit` without fetching. A missing clone is cloned first.
pub fn checkout_locked(
    vars: &BTreeMap<String, String>,
    root: &Path,
    name: &str,
    url: &str,
    commit: &str,
) -> Result<()> {
    let dir = root.join(name);
    if !is_cloned(&dir) {
        clone_source(vars, root, name, url)?;
    }
    git(vars, &dir, &["checkout", "--quiet", "--detach", commit])?;
    Ok(())
}

fn clone_source(vars: &BTreeMap<String, String>, root: &Path, name: &str, url: &str) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    git(vars, root, &["clone", "--quiet", url, name])?;
    Ok(())
}

fn resolve(
    vars: &BTreeMap<String, String>,
    dir: &Path,
    name: &str,
    rev: Option<&str>,
) -> Result<String> {
    let candidates = match rev {
        Some(rev) => vec![format!("origin/{rev}"), rev.to_owned()],
        None => vec!["origin/HEAD".to_owned()],
    };
    candidates
        .iter()
        .find_map(|candidate| {
            git(
                vars,
                dir,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{candidate}^{{commit}}"),
                ],
            )
            .ok()
            .filter(|sha| !sha.is_empty())
        })
        .with_context(|| {
            format!(
                "source `{name}` has no commit for {}",
                rev.unwrap_or("origin/HEAD")
            )
        })
}
