//! Steer rules: guidance for lessons git cannot see. A steer rule matches a
//! normalized hook event (a tool call or a user prompt) and either adds its
//! message to the agent's context or refuses the tool call.

use std::path::Path;

use globset::{GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::replay::{Verdict, CORPUS_DIR, FIXTURES_DIR};
use crate::rule::{Steer, Trigger};

const BAD_FIXTURE: &str = "bad.json";
const FIXED_FIXTURE: &str = "fixed.json";
const FIXTURE_EXTENSION: &str = "json";

/// A hook event as steer rules see it. Fixtures hold this shape as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum SteerEvent {
    /// A tool call about to run. `command` is set for shell tools; `paths`
    /// holds the files an edit or write tool touches, repo-relative and
    /// `/`-separated when they lie inside the repository.
    Tool {
        tool: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
    },
    /// A prompt the user submitted.
    Prompt { prompt: String },
}

/// A compiled trigger.
pub struct Matcher {
    kind: Kind,
}

enum Kind {
    Tool {
        name: GlobMatcher,
        command: Option<Regex>,
    },
    Path(GlobSet),
    Prompt(Regex),
}

impl Matcher {
    pub fn new(steer: &Steer) -> Result<Matcher, String> {
        let kind = match &steer.trigger {
            Trigger::Tool { name, command } => Kind::Tool {
                name: GlobBuilder::new(name)
                    .case_insensitive(true)
                    .build()
                    .map_err(|error| format!("tool glob `{name}`: {error}"))?
                    .compile_matcher(),
                command: command.as_deref().map(regex).transpose()?,
            },
            Trigger::Path { globs } => {
                let mut builder = GlobSetBuilder::new();
                for glob in globs {
                    builder.add(
                        GlobBuilder::new(glob)
                            .literal_separator(true)
                            .build()
                            .map_err(|error| format!("path glob `{glob}`: {error}"))?,
                    );
                }
                Kind::Path(
                    builder
                        .build()
                        .map_err(|error| format!("path globs: {error}"))?,
                )
            }
            Trigger::Prompt { pattern } => Kind::Prompt(regex(pattern)?),
        };
        Ok(Matcher { kind })
    }

    pub fn matches(&self, event: &SteerEvent) -> bool {
        match (&self.kind, event) {
            (
                Kind::Tool { name, command },
                SteerEvent::Tool {
                    tool,
                    command: given,
                    ..
                },
            ) => {
                name.is_match(tool)
                    && match command {
                        None => true,
                        Some(pattern) => {
                            given.as_deref().is_some_and(|text| pattern.is_match(text))
                        }
                    }
            }
            (Kind::Path(globs), SteerEvent::Tool { paths, .. }) => {
                paths.iter().any(|path| globs.is_match(path))
            }
            (Kind::Prompt(pattern), SteerEvent::Prompt { prompt }) => pattern.is_match(prompt),
            _ => false,
        }
    }
}

/// Replays a steer rule against its fixtures: `fixtures/bad.json` must match,
/// `fixtures/fixed.json` and every `corpus/*.json` must not. Pure and local:
/// the matcher reads only the fixtures.
pub fn replay(dir: &Path, steer: &Steer) -> Verdict {
    let matcher = match Matcher::new(steer) {
        Ok(matcher) => matcher,
        Err(error) => return Verdict::CouldNotRun(error),
    };
    let fixtures = dir.join(FIXTURES_DIR);
    let mut clean = vec![fixtures.join(FIXED_FIXTURE)];
    match corpus(&dir.join(CORPUS_DIR)) {
        Ok(files) => clean.extend(files),
        Err(error) => return Verdict::CouldNotRun(error),
    }
    let mut rejected = Vec::new();
    for (path, expect) in std::iter::once((fixtures.join(BAD_FIXTURE), true))
        .chain(clean.into_iter().map(|path| (path, false)))
    {
        let event = match read_event(&path) {
            Ok(event) => event,
            Err(error) => return Verdict::CouldNotRun(error),
        };
        let name = path.display();
        match (matcher.matches(&event), expect) {
            (false, true) => rejected.push(format!("{name} did not trigger")),
            (true, false) => rejected.push(format!("{name} triggered")),
            _ => {}
        }
    }
    if rejected.is_empty() {
        Verdict::Proven
    } else {
        Verdict::Rejected(rejected.join("; "))
    }
}

pub fn read_event(path: &Path) -> Result<SteerEvent, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("missing input: {}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("{} is not a steer event: {error}", path.display()))
}

fn corpus(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries =
        std::fs::read_dir(dir).map_err(|error| format!("reading {}: {error}", dir.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("reading {}: {error}", dir.display()))?
            .path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == FIXTURE_EXTENSION) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn regex(pattern: &str) -> Result<Regex, String> {
    Regex::new(pattern).map_err(|error| format!("regex `{pattern}`: {error}"))
}
