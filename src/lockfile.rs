use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

const LOCK_DIR: &str = ".oncebitten";
const LOCK_FILE: &str = "rules.lock";
const SHA_LEN: usize = 40;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Lockfile {
    pub source: Vec<LockedSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedSource {
    pub name: String,
    pub url: String,
    pub commit: String,
}

pub fn path(repo: &Path) -> PathBuf {
    repo.join(LOCK_DIR).join(LOCK_FILE)
}

pub fn read(repo: &Path) -> Result<Lockfile> {
    let file = path(repo);
    let text =
        std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let lock: Lockfile =
        toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
    for source in &lock.source {
        let full_sha =
            source.commit.len() == SHA_LEN && source.commit.bytes().all(|b| b.is_ascii_hexdigit());
        if !full_sha {
            bail!(
                "{}: commit `{}` for source `{}` is not a full SHA",
                file.display(),
                source.commit,
                source.name
            );
        }
    }
    Ok(lock)
}

pub fn write(repo: &Path, lock: &Lockfile) -> Result<()> {
    let file = path(repo);
    std::fs::create_dir_all(repo.join(LOCK_DIR))
        .with_context(|| format!("creating {}", repo.join(LOCK_DIR).display()))?;
    let text = toml::to_string(lock)?;
    std::fs::write(&file, text).with_context(|| format!("writing {}", file.display()))
}
