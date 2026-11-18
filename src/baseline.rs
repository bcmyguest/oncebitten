//! `.oncebitten/baseline.json`: violations that predate the gate. A violation
//! listed there is known and does not fail `oncebitten gate`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::engine::Finding;
use crate::proof::text_sha256;
use crate::rule::Check;

pub const BASELINE_FILE: &str = "baseline.json";
/// Gitignored; holds only the entries of local rules.
pub const LOCAL_BASELINE_FILE: &str = "baseline.local.json";
/// The id prefix of a local rule.
pub const LOCAL_PREFIX: &str = "local/";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub entries: BTreeSet<Entry>,
}

/// One known violation. An ast-grep entry names the file and the SHA-256 of
/// the matched text, so two identical matches in one file share an entry. A
/// native entry has neither: the native engine reports one finding for the
/// whole repository, so the entry covers every firing of that rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

pub fn path(repo: &Path) -> PathBuf {
    repo.join(".oncebitten").join(BASELINE_FILE)
}

pub fn local_path(repo: &Path) -> PathBuf {
    repo.join(".oncebitten").join(LOCAL_BASELINE_FILE)
}

/// The repo's shared baseline; empty when the file does not exist.
pub fn read(repo: &Path) -> Result<Baseline> {
    read_file(&path(repo))
}

/// The repo's local baseline; empty when the file does not exist.
pub fn read_local(repo: &Path) -> Result<Baseline> {
    read_file(&local_path(repo))
}

/// The shared baseline, plus the local one unless `ci`.
pub fn read_all(repo: &Path, ci: bool) -> Result<Baseline> {
    let mut all = read(repo)?;
    if !ci {
        all.entries.extend(read_local(repo)?.entries);
    }
    Ok(all)
}

fn read_file(path: &Path) -> Result<Baseline> {
    if !path.exists() {
        return Ok(Baseline::default());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn is_local(entry: &Entry) -> bool {
    entry.rule.starts_with(LOCAL_PREFIX)
}

/// The baseline entry for a finding, or `None` for a finding that is never
/// baselined: a diff-engine finding, which belongs to one change.
pub fn entry(rule_id: &str, check: &Check, repo: &Path, finding: &Finding) -> Option<Entry> {
    match check {
        Check::AstGrep { .. } => Some(Entry {
            rule: rule_id.to_owned(),
            path: Some(relative(repo, finding.path.as_deref()?)),
            sha256: Some(text_sha256(finding.text.as_deref()?)),
        }),
        Check::Native { .. } => Some(Entry {
            rule: rule_id.to_owned(),
            path: None,
            sha256: None,
        }),
        Check::Diff { .. } | Check::Steer(_) => None,
    }
}

impl Baseline {
    pub fn contains(&self, entry: &Entry) -> bool {
        self.entries.contains(entry)
    }
}

/// Writes the baseline as pretty JSON with entries sorted by rule, path and
/// hash, so a change to it reads as a short diff.
pub fn write(repo: &Path, baseline: &Baseline) -> Result<PathBuf> {
    write_file(&path(repo), baseline)
}

/// Writes the local-rule entries of `all` to the local file and the rest to
/// the shared one. The local file is left alone when it does not exist and
/// has no entries to hold. Returns the shared path and the local path when
/// written.
pub fn write_split(repo: &Path, all: &Baseline) -> Result<(PathBuf, Option<PathBuf>)> {
    let (local, shared): (BTreeSet<Entry>, BTreeSet<Entry>) =
        all.entries.iter().cloned().partition(is_local);
    let shared_path = write(repo, &Baseline { entries: shared })?;
    let local_path = local_path(repo);
    if local.is_empty() && !local_path.exists() {
        return Ok((shared_path, None));
    }
    let written = write_local(repo, &Baseline { entries: local })?;
    Ok((shared_path, Some(written)))
}

/// Replaces `rule_id`'s entries in the file that matches its layer with
/// `entries`. Returns the file written.
pub fn replace_rule(repo: &Path, rule_id: &str, entries: Vec<Entry>) -> Result<PathBuf> {
    let local = rule_id.starts_with(LOCAL_PREFIX);
    let mut merged = if local {
        read_local(repo)?
    } else {
        read(repo)?
    };
    merged.entries.retain(|entry| entry.rule != rule_id);
    merged.entries.extend(entries);
    if local {
        write_local(repo, &merged)
    } else {
        write(repo, &merged)
    }
}

/// Writes the local file and makes sure git ignores it.
fn write_local(repo: &Path, baseline: &Baseline) -> Result<PathBuf> {
    crate::rules::ensure_local_ignored(repo)?;
    write_file(&local_path(repo), baseline)
}

/// Moves the local file's entries for `local/<id>` into the shared file as
/// `<id>`. Returns the entries moved, under their new id, or `None`.
pub fn promote(repo: &Path, id: &str) -> Result<Option<Vec<Entry>>> {
    let local_id = format!("{LOCAL_PREFIX}{id}");
    let mut local = read_local(repo)?;
    let moved: Vec<Entry> = local
        .entries
        .iter()
        .filter(|entry| entry.rule == local_id)
        .cloned()
        .collect();
    if moved.is_empty() {
        return Ok(None);
    }
    local.entries.retain(|entry| entry.rule != local_id);
    let moved: Vec<Entry> = moved
        .into_iter()
        .map(|entry| Entry {
            rule: id.to_owned(),
            ..entry
        })
        .collect();
    let mut shared = read(repo)?;
    shared.entries.extend(moved.iter().cloned());
    write(repo, &shared)?;
    write_local(repo, &local)?;
    Ok(Some(moved))
}

/// A baseline file's text as `write` lays it out.
pub fn render(baseline: &Baseline) -> Result<String> {
    let mut text = serde_json::to_string_pretty(baseline)?;
    text.push('\n');
    Ok(text)
}

/// Parses a baseline file's text.
pub fn parse(text: &str) -> Result<Baseline> {
    Ok(serde_json::from_str(text)?)
}

fn write_file(path: &Path, baseline: &Baseline) -> Result<PathBuf> {
    let path = path.to_path_buf();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = render(baseline)?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// `path` relative to `repo`, `/`-separated.
fn relative(repo: &Path, path: &Path) -> String {
    path.strip_prefix(repo)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}
