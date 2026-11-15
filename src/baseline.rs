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

/// The repo's baseline; empty when the file does not exist.
pub fn read(repo: &Path) -> Result<Baseline> {
    let path = path(repo);
    if !path.exists() {
        return Ok(Baseline::default());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
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
        Check::Diff { .. } => None,
    }
}

impl Baseline {
    pub fn contains(&self, entry: &Entry) -> bool {
        self.entries.contains(entry)
    }
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
