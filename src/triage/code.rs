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
const ADDED: char = '+';
const REMOVED: char = '-';
/// Characters that alone make an added line too trivial to count as a quote.
const TRIVIAL_CHARS: &[char] = &['{', '}', '[', ']', '(', ')', ';', ','];

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

    /// Whether `bad` quotes the changed code: some run of consecutive lines of
    /// one hunk's post-image (its context and added lines, in order) appears
    /// consecutively in `bad` and includes at least one substantive added line.
    /// A blank added line, or one of only braces, brackets, parentheses, `;`
    /// and `,`, does not count. Line endings are normalised to LF and trailing
    /// spaces are dropped on both sides.
    pub fn quoted_by(&self, bad: &str) -> bool {
        let bad = normalize(bad);
        self.post_images()
            .iter()
            .any(|hunk| quotes_a_change(hunk, &bad))
    }

    /// The post-image of each diff hunk: `(substantive added, text)` per line, with
    /// removed lines left out. Context lines carry no prefix character in the
    /// text, so the first character is dropped only on `+` and ` ` lines.
    fn post_images(&self) -> Vec<Vec<(bool, String)>> {
        let mut hunks = Vec::new();
        let mut current = Vec::new();
        let mut in_header = false;
        for line in normalize(&self.diff) {
            if line.starts_with(DIFF_MARKER) {
                if !current.is_empty() {
                    hunks.push(std::mem::take(&mut current));
                }
                in_header = true;
            } else if line.starts_with(HUNK_MARKER) {
                if !current.is_empty() {
                    hunks.push(std::mem::take(&mut current));
                }
                in_header = false;
            } else if !in_header {
                match line.chars().next() {
                    Some(ADDED) => {
                        let text = line[1..].to_string();
                        current.push((!is_trivial(&text), text));
                    }
                    Some(REMOVED) | Some('\\') => {}
                    _ => current.push((false, line.get(1..).unwrap_or("").to_string())),
                }
            }
        }
        if !current.is_empty() {
            hunks.push(current);
        }
        hunks
    }
}

/// Whether `line` holds nothing but whitespace and [`TRIVIAL_CHARS`].
fn is_trivial(line: &str) -> bool {
    line.chars()
        .all(|c| c.is_whitespace() || TRIVIAL_CHARS.contains(&c))
}

/// Whether a run of `hunk` that includes a substantive added line also appears
/// consecutively in `bad`.
fn quotes_a_change(hunk: &[(bool, String)], bad: &[String]) -> bool {
    for start in 0..hunk.len() {
        for (at, line) in bad.iter().enumerate() {
            if *line != hunk[start].1 {
                continue;
            }
            let mut added = false;
            let mut offset = 0;
            while start + offset < hunk.len()
                && at + offset < bad.len()
                && hunk[start + offset].1 == bad[at + offset]
            {
                added |= hunk[start + offset].0;
                if added {
                    return true;
                }
                offset += 1;
            }
        }
    }
    false
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

/// `raw` as a path relative to `repo`, with `/` separators. `None` when an
/// absolute `raw` lies outside `repo` or names no file.
fn repo_relative(repo: &Path, raw: &str) -> Option<String> {
    let repo = native(&repo.display().to_string());
    let raw = native(raw);
    let relative = if Path::new(&raw).is_absolute() {
        strip_repo(&raw, &repo)?
    } else {
        raw.as_str()
    };
    let mut parts = Vec::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// `path` with `\` as `/`, and on Windows an MSYS drive (`/c/...`) as `c:/...`.
/// The drive swap keeps the byte length, so offsets into the result match the input.
#[cfg(windows)]
fn native(path: &str) -> String {
    let text = path.replace('\\', "/");
    let bytes = text.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b'/' {
        format!("{}:{}", &text[1..2], &text[2..])
    } else {
        text
    }
}

#[cfg(not(windows))]
fn native(path: &str) -> String {
    path.to_string()
}

/// The part of `raw` after `repo` and its `/`, or `None` when `raw` is not
/// under `repo`. Windows compares case-insensitively.
fn strip_repo<'a>(raw: &'a str, repo: &str) -> Option<&'a str> {
    let repo = repo.trim_end_matches('/');
    let head = raw.get(..repo.len())?;
    let same = if cfg!(windows) {
        head.eq_ignore_ascii_case(repo)
    } else {
        head == repo
    };
    if !same {
        return None;
    }
    raw[repo.len()..].strip_prefix('/')
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
