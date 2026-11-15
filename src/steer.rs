//! Steer rules: guidance for lessons git cannot see. A steer rule matches a
//! normalized hook event (a tool call or a user prompt) and either adds its
//! message to the agent's context or refuses the tool call.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use globset::{GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::config::Inputs;
use crate::host::{self, Host};
use crate::replay::{Verdict, CORPUS_DIR, FIXTURES_DIR};
use crate::rule::{Action, Check, Status, Steer, Trigger};
use crate::rules::{self, Failure};

const BAD_FIXTURE: &str = "bad.json";
const FIXED_FIXTURE: &str = "fixed.json";
const FIXTURE_EXTENSION: &str = "json";
const TRIAGE_ENV: &str = "ONCEBITTEN_TRIAGE";

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

/// What the active steer rules make of one event.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Decision {
    /// `<rule id>: <message>` for each matching deny rule.
    pub deny: Vec<String>,
    /// `<rule id>: <message>` for each matching inform rule.
    pub inform: Vec<String>,
}

impl Decision {
    pub fn is_empty(&self) -> bool {
        self.deny.is_empty() && self.inform.is_empty()
    }
}

/// Matches `event` against `rules` (id and steer table). A deny rule on a
/// prompt event cannot load, so deny lines only come from tool events.
pub fn decide<'a>(
    event: &SteerEvent,
    rules: impl IntoIterator<Item = (&'a str, &'a Steer)>,
) -> Decision {
    let mut decision = Decision::default();
    for (id, steer) in rules {
        let Ok(matcher) = Matcher::new(steer) else {
            continue;
        };
        if !matcher.matches(event) {
            continue;
        }
        let line = format!("oncebitten {id}: {}", steer.message.trim());
        match steer.action {
            Action::Deny => decision.deny.push(line),
            Action::Inform => decision.inform.push(line),
        }
    }
    decision
}

/// The hook output `host` reads, or `None` when there is nothing to say or
/// the host has no way to say it (see docs/steer.md for each host).
pub fn render(host: Host, event: &SteerEvent, decision: &Decision) -> Option<String> {
    if decision.is_empty() {
        return None;
    }
    let deny = (!decision.deny.is_empty()).then(|| decision.deny.join("\n"));
    let inform = (!decision.inform.is_empty()).then(|| decision.inform.join("\n"));
    let is_tool = matches!(event, SteerEvent::Tool { .. });
    let value = match host {
        Host::Claude | Host::Codex => {
            let mut output = Map::new();
            let event_name = if is_tool {
                "PreToolUse"
            } else {
                "UserPromptSubmit"
            };
            output.insert("hookEventName".into(), json!(event_name));
            if let (true, Some(reason)) = (is_tool, &deny) {
                output.insert("permissionDecision".into(), json!("deny"));
                output.insert("permissionDecisionReason".into(), json!(reason));
            }
            if let Some(context) = &inform {
                output.insert("additionalContext".into(), json!(context));
            }
            if output.len() == 1 {
                return None;
            }
            json!({ "hookSpecificOutput": output })
        }
        Host::Cursor => {
            let reason = deny.filter(|_| is_tool)?;
            json!({ "permission": "deny", "user_message": reason, "agent_message": reason })
        }
        Host::Copilot => {
            let reason = deny.filter(|_| is_tool)?;
            json!({ "permissionDecision": "deny", "permissionDecisionReason": reason })
        }
        // The plugin scripts in packaging/<host>/oncebitten-steer.js read this
        // shape and act through the host's plugin API.
        Host::OpenCode | Host::Pi => {
            let mut output = Map::new();
            if let (true, Some(reason)) = (is_tool, &deny) {
                output.insert("deny".into(), json!(reason));
            }
            if let Some(context) = &inform {
                output.insert("context".into(), json!(context));
            }
            if output.is_empty() {
                return None;
            }
            Value::Object(output)
        }
    };
    Some(value.to_string())
}

/// Runs the steer hook for one payload: normalizes it, loads the active steer
/// rules for the agent's working directory and renders the host's output.
pub fn run(host: Host, payload: &str, inputs: &Inputs) -> Result<Option<String>> {
    let Some(input) = host::normalize_steer(payload)? else {
        return Ok(None);
    };
    let mut inputs = inputs.clone();
    if let Some(dir) = input.cwd.as_deref().map(PathBuf::from) {
        if dir.is_dir() {
            inputs.cwd = dir;
        }
    }
    let rules = match rules::load_skipping_broken(&inputs) {
        Ok((rules, skipped)) => {
            for error in skipped {
                eprintln!("oncebitten steer: skipped rule: {error:#}");
            }
            rules
        }
        Err(Failure::Invalid(errors)) => {
            let errors: Vec<String> = errors.iter().map(|error| format!("{error:#}")).collect();
            bail!("invalid rules: {}", errors.join("; "))
        }
        Err(Failure::CouldNotRun(error)) => return Err(error),
    };
    let active = rules
        .iter()
        .filter_map(|resolved| match &resolved.rule.check {
            Check::Steer(steer) if resolved.rule.status == Status::Active => {
                Some((resolved.id.as_str(), steer))
            }
            _ => None,
        });
    let decision = decide(&input.event, active);
    Ok(render(host, &input.event, &decision))
}

/// `oncebitten steer --host <host>`: reads the payload from stdin and prints
/// the host's output. Fails open: an error goes to stderr and the call exits 0
/// with no output, so the agent carries on unsteered.
pub fn run_from_process(host: &str) -> Result<()> {
    let mut payload = String::new();
    let read = std::io::stdin().read_to_string(&mut payload);
    if std::env::var(TRIAGE_ENV).as_deref() == Ok("1") {
        return Ok(());
    }
    read.context("cannot read hook payload from stdin")?;
    let host = Host::parse(host)?;
    if let Some(output) = run(host, &payload, &Inputs::from_process()?)? {
        println!("{output}");
    }
    Ok(())
}
