//! `rules retire` and `rules rollback`: status changes to a project or local
//! rule's `rule.toml`, made in place so its comments and layout survive.

use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use crate::config::{self, Inputs};
use crate::registry::{Layer, Resolved};
use crate::rule::Status;
use crate::rules::{self, Failure};

pub const RETIRED_REASON: &str = "retired_reason";
const STATUS: &str = "status";

/// The rule.toml a rollback changed, and the git commands that restore an
/// earlier version of its directory.
pub struct RolledBack {
    pub file: PathBuf,
    pub restore: Vec<String>,
}

/// Sets `status = "retired"` and `retired_reason` on a project or local rule.
pub fn retire(inputs: &Inputs, id: &str, reason: &str) -> Result<PathBuf, Failure> {
    if reason.trim().is_empty() {
        return Err(Failure::CouldNotRun(anyhow!("--reason is empty")));
    }
    let resolved = editable_rule(inputs, id, "retire it")?;
    if resolved.rule.status == Status::Retired {
        return Err(Failure::CouldNotRun(anyhow!("`{id}` is already retired")));
    }
    edit(&resolved.rule.file, |document| {
        document[STATUS] = toml_edit::value(Status::Retired.as_str());
        document[RETIRED_REASON] = toml_edit::value(reason);
    })
    .map_err(Failure::CouldNotRun)?;
    Ok(resolved.rule.file)
}

/// Returns an active project or local rule to `proposed`. Its files and proof stay.
pub fn rollback(inputs: &Inputs, id: &str) -> Result<RolledBack, Failure> {
    let resolved = editable_rule(inputs, id, "roll it back")?;
    if resolved.rule.status != Status::Active {
        return Err(Failure::CouldNotRun(anyhow!(
            "`{id}` is {}; only an active rule rolls back",
            resolved.rule.status.as_str()
        )));
    }
    edit(&resolved.rule.file, |document| {
        document[STATUS] = toml_edit::value(Status::Proposed.as_str());
    })
    .map_err(Failure::CouldNotRun)?;
    let repo = config::repo_root(&inputs.cwd)
        .ok_or_else(|| Failure::CouldNotRun(anyhow!("not inside a git repository")))?;
    let dir = resolved
        .rule
        .file
        .parent()
        .and_then(|dir| dir.strip_prefix(&repo).ok())
        .map(slashed)
        .ok_or_else(|| Failure::CouldNotRun(anyhow!("`{id}` lies outside the repository")))?;
    // A gitignored local rule has no history to restore from.
    let restore = match resolved.layer {
        Layer::Local => Vec::new(),
        _ => vec![
            format!("git log --oneline -- {dir}"),
            format!("git restore --source <commit> -- {dir}"),
        ],
    };
    Ok(RolledBack {
        file: resolved.rule.file,
        restore,
    })
}

/// `action` reads as an instruction, such as "retire it".
fn editable_rule(inputs: &Inputs, id: &str, action: &str) -> Result<Resolved, Failure> {
    let resolved = rules::load(inputs, false)?;
    let Some(found) = resolved.into_iter().find(|entry| entry.id == id) else {
        return Err(Failure::CouldNotRun(anyhow!(
            "no rule named `{id}`; see oncebitten rules list"
        )));
    };
    match &found.layer {
        Layer::Local | Layer::Project => Ok(found),
        Layer::Source(name) => Err(Failure::CouldNotRun(anyhow!(
            "`{id}` comes from rules source `{name}`; {action} in that rules repo, \
             or override it with a project rule that sets overrides = \"{id}\""
        ))),
        Layer::Example => Err(Failure::CouldNotRun(anyhow!(
            "`{id}` is a shipped example; remove it from [rules] use_examples"
        ))),
    }
}

fn edit(file: &Path, change: impl FnOnce(&mut toml_edit::DocumentMut)) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .with_context(|| format!("parsing {}", file.display()))?;
    change(&mut document);
    if !document.contains_key(STATUS) {
        bail!("{} lost its status key", file.display());
    }
    std::fs::write(file, document.to_string())
        .with_context(|| format!("writing {}", file.display()))
}

fn slashed(path: &Path) -> String {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}
