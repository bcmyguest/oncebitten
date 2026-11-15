//! `.oncebitten/exceptions.toml`: findings a rule lets through on named paths
//! until a date, each with a reason. An expired exception lets nothing through.

use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use globset::{GlobBuilder, GlobMatcher};
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::Timestamp;
use serde::Deserialize;

use crate::check::Checked;
use crate::engine::{Finding, Outcome};

pub const EXCEPTIONS_FILE: &str = "exceptions.toml";
/// The glob that covers every path. A finding without a path, such as a
/// native rule's, is covered only by an exception that lists it.
pub const EVERY_PATH: &str = "**";
const DATE_FORMAT: &str = "%Y-%m-%d";
const TABLE: &str = "exception";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    exception: Vec<RawException>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawException {
    rule: String,
    paths: Vec<String>,
    reason: String,
    expires: String,
}

#[derive(Debug, Clone)]
pub struct Exception {
    pub rule: String,
    pub paths: Vec<String>,
    pub reason: String,
    /// The last day, in UTC, on which the exception applies.
    pub expires: Date,
    matchers: Vec<GlobMatcher>,
}

impl Exception {
    pub fn new(rule: &str, paths: &[String], reason: &str, expires: &str) -> Result<Self> {
        if rule.trim().is_empty() {
            bail!("rule is empty");
        }
        if paths.is_empty() {
            bail!("paths is empty; list globs, or `{EVERY_PATH}` for every path");
        }
        let matchers = paths
            .iter()
            .map(|pattern| {
                if pattern.trim().is_empty() {
                    bail!("paths holds an empty glob");
                }
                Ok(GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .build()
                    .with_context(|| format!("glob `{pattern}`"))?
                    .compile_matcher())
            })
            .collect::<Result<Vec<_>>>()?;
        if reason.trim().is_empty() {
            bail!("reason is empty");
        }
        let expires = jiff::fmt::strtime::parse(DATE_FORMAT, expires)
            .and_then(|parsed| parsed.to_date())
            .map_err(|error| anyhow!("expires `{expires}` is not a YYYY-MM-DD date: {error}"))?;
        Ok(Exception {
            rule: rule.to_owned(),
            paths: paths.to_vec(),
            reason: reason.to_owned(),
            expires,
            matchers,
        })
    }

    pub fn is_live(&self, today: Date) -> bool {
        today <= self.expires
    }

    pub fn covers(&self, rule: &str, finding: &Finding) -> bool {
        if self.rule != rule {
            return false;
        }
        match &finding.path {
            Some(path) => {
                let path = slashed(path);
                self.matchers.iter().any(|matcher| matcher.is_match(&path))
            }
            None => self.paths.iter().any(|glob| glob == EVERY_PATH),
        }
    }
}

pub fn path(repo: &Path) -> PathBuf {
    repo.join(".oncebitten").join(EXCEPTIONS_FILE)
}

/// The repo's exceptions; empty when the file does not exist.
pub fn read(repo: &Path) -> Result<Vec<Exception>> {
    let path = path(repo);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let raw: RawFile =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    raw.exception
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            Exception::new(&raw.rule, &raw.paths, &raw.reason, &raw.expires).with_context(|| {
                format!(
                    "{}: exception {} (rule `{}`)",
                    path.display(),
                    index + 1,
                    raw.rule
                )
            })
        })
        .collect()
}

/// Appends `exception` to the file as text, so the comments and layout
/// above it stay as they are. Refuses to touch a file that does not load.
pub fn add(repo: &Path, exception: &Exception) -> Result<PathBuf> {
    read(repo)?;
    let path = path(repo);
    let mut text = if path.exists() {
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };
    let mut table = toml_edit::Table::new();
    table.insert("rule", toml_edit::value(&exception.rule));
    table.insert(
        "paths",
        toml_edit::value(exception.paths.iter().collect::<toml_edit::Array>()),
    );
    table.insert("reason", toml_edit::value(&exception.reason));
    table.insert(
        "expires",
        toml_edit::value(exception.expires.strftime(DATE_FORMAT).to_string()),
    );
    let mut entry = toml_edit::ArrayOfTables::new();
    entry.push(table);
    let mut document = toml_edit::DocumentMut::new();
    document.insert(TABLE, toml_edit::Item::ArrayOfTables(entry));
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(&document.to_string());
    toml::from_str::<RawFile>(&text)
        .with_context(|| format!("{} does not take a new [[{TABLE}]]", path.display()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Today's date in UTC, so a laptop and CI agree on when an exception expires.
pub fn today() -> Date {
    Timestamp::now().to_zoned(TimeZone::UTC).date()
}

/// Removes the findings a live exception covers. Returns the remaining
/// outcomes and how many findings were excepted.
pub fn apply(
    checked: Vec<Checked>,
    exceptions: &[Exception],
    today: Date,
) -> (Vec<Checked>, usize) {
    let live: Vec<&Exception> = exceptions.iter().filter(|e| e.is_live(today)).collect();
    let mut excepted = 0;
    let checked = checked
        .into_iter()
        .map(|item| {
            let Outcome::Fired(findings) = item.outcome else {
                return item;
            };
            let total = findings.len();
            let kept: Vec<Finding> = findings
                .into_iter()
                .filter(|finding| !live.iter().any(|e| e.covers(&item.id, finding)))
                .collect();
            excepted += total - kept.len();
            Checked {
                outcome: if kept.is_empty() {
                    Outcome::Clean
                } else {
                    Outcome::Fired(kept)
                },
                id: item.id,
            }
        })
        .collect();
    (checked, excepted)
}

pub fn describe(exception: &Exception) -> String {
    format!(
        "{} on {} until {}: {}",
        exception.rule,
        exception.paths.join(", "),
        exception.expires,
        exception.reason
    )
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
