use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};
use std::process::Command;

use anyhow::Result;

use crate::evidence::Occurrence;
use crate::store::Store;
use crate::triage::prompt::bound;

const GIT: &str = "git";
const DIFF_MARKER: &str = "diff --git ";
const HUNK_MARKER: &str = "@@";

/// The changed code a triage call sees: unified diff text from each occurrence's
/// captured commit to the working tree, bounded by `triage.max_code_bytes`.
/// Nothing here reaches the store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Code {
    pub diff: String,
}

impl Code {
    pub fn is_empty(&self) -> bool {
        self.diff.is_empty()
    }

    /// Whether `bad` holds every added line of one hunk, in order and
    /// consecutively. Line endings are normalised to LF and trailing spaces
    /// are dropped on both sides.
    pub fn quoted_by(&self, bad: &str) -> bool {
        let bad = normalize(bad);
        self.added_blocks()
            .iter()
            .any(|block| contains_run(&bad, block))
    }

    /// The added lines of each diff hunk, without the leading `+`.
    fn added_blocks(&self) -> Vec<Vec<String>> {
        let mut blocks = Vec::new();
        let mut current = Vec::new();
        let mut in_header = false;
        for line in normalize(&self.diff) {
            if line.starts_with(DIFF_MARKER) {
                if !current.is_empty() {
                    blocks.push(std::mem::take(&mut current));
                }
                in_header = true;
            } else if line.starts_with(HUNK_MARKER) {
                if !current.is_empty() {
                    blocks.push(std::mem::take(&mut current));
                }
                in_header = false;
            } else if !in_header {
                if let Some(added) = line.strip_prefix('+') {
                    current.push(added.to_string());
                }
            }
        }
        if !current.is_empty() {
            blocks.push(current);
        }
        blocks
    }
}

/// The diff of every file the sessions behind `occurrences` edited, taken from
/// each occurrence's `git_head` to the working tree. Occurrences without a
/// repo or a head add nothing. A git failure skips its diff.
pub fn collect(store: &Store, occurrences: &[Occurrence], max_bytes: usize) -> Result<Code> {
    let mut sessions: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut groups: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for occurrence in occurrences {
        let event = &occurrence.event;
        let (Some(repo), Some(head)) = (&event.repo, &event.git_head) else {
            continue;
        };
        let key = (event.host.clone(), event.session_id.clone());
        if !sessions.contains_key(&key) {
            let paths = store.session_edit_paths(&event.host, &event.session_id)?;
            sessions.insert(key.clone(), paths);
        }
        let files = groups.entry((repo.clone(), head.clone())).or_default();
        for raw in &sessions[&key] {
            if let Some(path) = repo_relative(Path::new(repo), raw) {
                files.insert(path);
            }
        }
    }
    let mut diff = String::new();
    for ((repo, head), files) in &groups {
        if files.is_empty() {
            continue;
        }
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        if let Some(text) = git_diff(Path::new(repo), head, &files) {
            diff.push_str(&text);
        }
    }
    Ok(Code {
        diff: bound(&diff, max_bytes),
    })
}

/// Splits text into lines with CRLF folded to LF and trailing spaces dropped.
fn normalize(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect()
}

fn contains_run(haystack: &[String], needle: &[String]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// `raw` as a path relative to `repo`, with `/` separators. `None` when an
/// absolute `raw` lies outside `repo` or names no file.
fn repo_relative(repo: &Path, raw: &str) -> Option<String> {
    let path = Path::new(raw);
    let relative = if path.is_absolute() {
        path.strip_prefix(repo).ok()?
    } else {
        path
    };
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// `git diff <head>` for `files` in `repo`. Literal pathspecs, and no external
/// diff or textconv drivers, so the output is the file text.
fn git_diff(repo: &Path, head: &str, files: &[&str]) -> Option<String> {
    let output = Command::new(GIT)
        .arg("-C")
        .arg(repo)
        .args([
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--unified=3",
            head,
            "--",
        ])
        .args(files)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}
