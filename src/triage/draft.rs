use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::engine::{astgrep, diff};
use crate::rule::{self, is_fixture_named, is_kebab_case, Mode, Status};
use crate::steer::SteerEvent;

pub const MAX_ID_CHARS: usize = 64;
pub const MAX_TEXT_CHARS: usize = 1000;
pub const MAX_FILE_BYTES: usize = 64 * 1024;
pub const MAX_CORPUS_FILES: usize = 10;
/// Characters Windows refuses in a file name; draft files are rejected on every
/// platform so a rule written on Linux checks out on Windows.
const WINDOWS_FORBIDDEN: [char; 6] = ['<', '>', '"', '|', '?', '*'];
const WINDOWS_DEVICES: &[&str] = &["con", "prn", "aux", "nul"];
const RULE_FILE: &str = "rule.toml";
const CHECK_FILE: &str = "check.yml";
const FIXTURES_DIR: &str = "fixtures";
const CORPUS_DIR: &str = "corpus";

/// The rule a triage call drafts beside its verdict. Every optional key may be
/// absent or `null`; the schema lists them all so strict hosts accept it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub id: String,
    pub description: String,
    pub mode: Mode,
    pub engine: Engine,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub language: Option<String>,
    /// The ast-grep `check.yml` text.
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub check: Option<String>,
    #[serde(default, deserialize_with = "empty_vec_as_none")]
    pub command: Option<Vec<String>>,
    #[serde(default, deserialize_with = "empty_vec_as_none")]
    pub fired_exit_codes: Option<Vec<i32>>,
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub fired_output_contains: Option<String>,
    #[serde(default, deserialize_with = "empty_vec_as_none")]
    pub when_changed: Option<Vec<String>>,
    #[serde(default, deserialize_with = "empty_vec_as_none")]
    pub require_changed: Option<Vec<String>>,
    pub bad: DraftFile,
    pub fixed: DraftFile,
    #[serde(default, deserialize_with = "empty_vec_as_none")]
    pub corpus: Option<Vec<DraftFile>>,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftFile {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Engine {
    #[serde(rename = "ast-grep")]
    AstGrep,
    #[serde(rename = "diff")]
    Diff,
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "steer")]
    Steer,
}

/// `rule.toml` as a proposal writes it. Field order matches docs/rules.md.
#[derive(Serialize)]
struct RuleToml<'a> {
    id: &'a str,
    description: &'a str,
    mode: &'a str,
    status: &'a str,
    engine: Engine,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fired_exit_codes: Option<&'a [i32]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fired_output_contains: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    when_changed: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    require_changed: Option<&'a [String]>,
    /// Tables come last so TOML writes them after the scalar keys.
    #[serde(skip_serializing_if = "Option::is_none")]
    steer: Option<SteerToml<'a>>,
}

/// A steer rule's `[steer]` table. A steer draft always informs.
#[derive(Serialize)]
struct SteerToml<'a> {
    action: &'a str,
    message: &'a str,
    trigger: TriggerToml,
}

#[derive(Serialize)]
struct TriggerToml {
    tool: String,
    command: String,
}

/// The `[steer]` table for a steer draft: the recorded command's tool, and a
/// regex matching its program plus distinguishing arguments, so near-variants
/// of the command fire too. `None` for other engines.
fn steer_table(draft: &Draft) -> Option<SteerToml<'_>> {
    if draft.engine != Engine::Steer {
        return None;
    }
    let event = steer_event(&draft.bad.content).ok()?;
    let SteerEvent::Tool {
        tool,
        command: Some(command),
        ..
    } = event
    else {
        return None;
    };
    Some(SteerToml {
        action: "inform",
        message: draft.rationale.trim(),
        trigger: TriggerToml {
            tool,
            command: command_trigger(&command),
        },
    })
}

/// A regex for `command`'s program, its first subcommand (a word before any
/// flag) and its first flag, in that order with anything between. Leading
/// `NAME=value` assignments are skipped and the program may carry a path, so
/// `git push --force origin main` becomes a trigger that also matches
/// `git -C repo push origin dev --force` but not `git push origin main`.
pub fn command_trigger(command: &str) -> String {
    let mut words = command
        .split_whitespace()
        .skip_while(|word| is_assignment(word));
    let Some(program) = words.next() else {
        return format!("^{}$", regex::escape(command));
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let rest: Vec<&str> = words.collect();
    let flag = rest.iter().position(|word| word.starts_with('-'));
    let subcommand = rest[..flag.unwrap_or(rest.len())].first();
    let mut pattern = format!(r"^\s*(?:\S*/)?{}", regex::escape(program));
    for word in subcommand.into_iter().chain(flag.map(|index| &rest[index])) {
        pattern.push_str(&format!(r"(?:\s.*)?\s{}", regex::escape(word)));
    }
    pattern.push_str(r"(?:\s.*)?$");
    pattern
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn steer_event(content: &str) -> Result<SteerEvent> {
    serde_json::from_str(content).context("the steer fixture is not a recorded tool event")
}

impl Draft {
    /// The command a steer draft's `bad` fixture names, when it names one.
    pub fn bad_command(&self) -> Option<String> {
        match steer_event(&self.bad.content).ok()? {
            SteerEvent::Tool { command, .. } => command,
            SteerEvent::Prompt { .. } => None,
        }
    }

    /// Sets a steer draft's fixtures from recorded commands: `command` run with
    /// `tool` is `bad`, and each of `corpus` is a clean event. Empty `corpus`
    /// keeps the model's own corpus.
    pub fn with_recorded_command(
        &mut self,
        tool: &str,
        command: &str,
        corpus: &[String],
    ) -> Result<()> {
        self.bad = DraftFile {
            name: "bad.json".to_owned(),
            content: serde_json::to_string(&SteerEvent::Tool {
                tool: tool.to_owned(),
                command: Some(command.to_owned()),
                paths: Vec::new(),
            })?,
        };
        if !corpus.is_empty() {
            let mut files = Vec::with_capacity(corpus.len());
            for (index, text) in corpus.iter().enumerate() {
                files.push(DraftFile {
                    name: format!("corpus-{index}.json"),
                    content: serde_json::to_string(&SteerEvent::Tool {
                        tool: tool.to_owned(),
                        command: Some(text.clone()),
                        paths: Vec::new(),
                    })?,
                });
            }
            self.corpus = Some(files);
        }
        Ok(())
    }

    /// Checks the draft as strictly as a hand-written rule: it must load with
    /// `rule::load_rule`, its check file must compile and its globs must parse.
    pub fn validate(&self) -> Result<()> {
        if !is_kebab_case(&self.id) || self.id.chars().count() > MAX_ID_CHARS {
            bail!(
                "draft id `{}` must be lowercase kebab-case of at most {MAX_ID_CHARS} characters",
                self.id
            );
        }
        bounded_text("description", &self.description)?;
        bounded_text("rationale", &self.rationale)?;
        self.check_engine_keys()?;
        check_file_name(&self.bad, Some("bad"))?;
        check_file_name(&self.fixed, Some("fixed"))?;
        let corpus = self.corpus.as_deref().unwrap_or_default();
        if corpus.len() > MAX_CORPUS_FILES {
            bail!("draft corpus lists more than {MAX_CORPUS_FILES} files");
        }
        let mut names = std::collections::BTreeSet::new();
        for file in corpus {
            check_file_name(file, None)?;
            if !names.insert(file.name.as_str()) {
                bail!("draft corpus names `{}` twice", file.name);
            }
        }
        let scratch = tempfile::Builder::new()
            .prefix("oncebitten-draft-")
            .tempdir()
            .context("cannot create a scratch directory for the draft")?;
        let dir = self.write(scratch.path())?;
        let loaded = rule::load_rule(&dir).context("the drafted rule does not load")?;
        if loaded.status != Status::Proposed {
            bail!("a drafted rule must be proposed");
        }
        Ok(())
    }

    /// Writes the rule directory `<parent>/<id>` with `status = "proposed"`.
    /// Fails when the directory exists, so a proposal never replaces a rule.
    pub fn write(&self, parent: &Path) -> Result<PathBuf> {
        let dir = parent.join(&self.id);
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
        if let Err(error) = self.write_contents(&dir) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(error);
        }
        Ok(dir)
    }

    fn write_contents(&self, dir: &Path) -> Result<()> {
        let rule = RuleToml {
            id: &self.id,
            description: self.description.trim(),
            mode: self.mode.as_str(),
            status: Status::Proposed.as_str(),
            engine: self.engine,
            language: self.language.as_deref(),
            command: self.command.as_deref(),
            fired_exit_codes: self.fired_exit_codes.as_deref(),
            fired_output_contains: self.fired_output_contains.as_deref(),
            when_changed: self.when_changed.as_deref(),
            require_changed: self.require_changed.as_deref(),
            steer: steer_table(self),
        };
        write_file(&dir.join(RULE_FILE), &toml::to_string(&rule)?)?;
        if let Some(check) = &self.check {
            write_file(&dir.join(CHECK_FILE), check)?;
        }
        let fixtures = dir.join(FIXTURES_DIR);
        std::fs::create_dir(&fixtures)
            .with_context(|| format!("creating {}", fixtures.display()))?;
        write_file(&fixtures.join(&self.bad.name), &self.bad.content)?;
        write_file(&fixtures.join(&self.fixed.name), &self.fixed.content)?;
        let corpus = self.corpus.as_deref().unwrap_or_default();
        if !corpus.is_empty() {
            let corpus_dir = dir.join(CORPUS_DIR);
            std::fs::create_dir(&corpus_dir)
                .with_context(|| format!("creating {}", corpus_dir.display()))?;
            for file in corpus {
                write_file(&corpus_dir.join(&file.name), &file.content)?;
            }
        }
        Ok(())
    }

    fn check_engine_keys(&self) -> Result<()> {
        let globs = self.when_changed.is_some() || self.require_changed.is_some();
        let native_keys = self.command.is_some()
            || self.fired_exit_codes.is_some()
            || self.fired_output_contains.is_some();
        if matches!(self.engine, Engine::Steer) && !matches!(self.mode, Mode::Steer) {
            bail!("engine = \"steer\" needs mode = \"steer\"");
        }
        match self.engine {
            Engine::Steer => {
                if self.check.is_some() || globs || native_keys {
                    bail!("a steer draft takes only its recorded fixtures");
                }
                let event = steer_event(&self.bad.content)?;
                match event {
                    SteerEvent::Tool {
                        command: Some(ref c),
                        ..
                    } if !c.trim().is_empty() => {}
                    _ => bail!("a steer draft's bad fixture needs a recorded command"),
                }
                steer_event(&self.fixed.content)?;
            }
            Engine::AstGrep => {
                if globs || native_keys {
                    bail!("an ast-grep draft takes only `check`");
                }
                let Some(check) = &self.check else {
                    bail!("an ast-grep draft needs `check`, the check.yml text");
                };
                bounded_file("check", check)?;
                let rules = astgrep::parse_rules(check, CHECK_FILE).map_err(anyhow::Error::msg)?;
                astgrep::collection(rules, CHECK_FILE).map_err(anyhow::Error::msg)?;
            }
            Engine::Diff => {
                if self.check.is_some() || native_keys {
                    bail!("a diff draft takes only `when_changed` and `require_changed`");
                }
                for (key, patterns) in [
                    ("when_changed", &self.when_changed),
                    ("require_changed", &self.require_changed),
                ] {
                    let Some(patterns) = patterns.as_deref().filter(|p| !p.is_empty()) else {
                        bail!("a diff draft needs a non-empty `{key}`");
                    };
                    diff::check_globs(patterns).map_err(anyhow::Error::msg)?;
                }
            }
            Engine::Native => {
                if self.check.is_some() || globs {
                    bail!("a native draft takes only `command` and its fired keys");
                }
                match self.command.as_deref() {
                    Some([program, ..]) if !program.trim().is_empty() => {}
                    _ => bail!("a native draft needs a non-empty `command`"),
                }
                match self.fired_exit_codes.as_deref() {
                    Some(codes) if !codes.is_empty() && !codes.contains(&0) => {}
                    _ => bail!("a native draft needs non-zero `fired_exit_codes`"),
                }
            }
        }
        Ok(())
    }
}

fn bounded_text(key: &str, text: &str) -> Result<()> {
    if text.trim().is_empty() {
        bail!("draft {key} is empty");
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        bail!("draft {key} exceeds {MAX_TEXT_CHARS} characters");
    }
    Ok(())
}

fn bounded_file(key: &str, content: &str) -> Result<()> {
    if content.len() > MAX_FILE_BYTES {
        bail!("draft {key} exceeds {MAX_FILE_BYTES} bytes");
    }
    Ok(())
}

fn is_windows_device(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    WINDOWS_DEVICES.contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.ends_with(|c: char| c.is_ascii_digit() && c != '0'))
}

/// A draft file name is one plain path component; fixtures carry their stem.
fn check_file_name(file: &DraftFile, stem: Option<&str>) -> Result<()> {
    let name = file.name.as_str();
    let mut components = Path::new(name).components();
    let plain = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !name.contains(['/', '\\', ':'])
        && !name.starts_with('.')
        && !name.contains(WINDOWS_FORBIDDEN)
        && !name.chars().any(char::is_control)
        && !name.ends_with(['.', ' '])
        && !is_windows_device(name);
    if !plain {
        bail!("draft file name `{name}` must be a plain file name");
    }
    if let Some(stem) = stem {
        if !is_fixture_named(name, stem) || Path::new(name).extension().is_none() {
            bail!("draft fixture `{name}` must be named {stem}.<ext>");
        }
    }
    bounded_file(name, &file.content)
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

/// Hosts that enforce strict JSON schemas send every draft key, and a model
/// often fills an unused one with `""` rather than `null`. Both mean absent.
fn empty_string_as_none<'de, D>(deserializer: D) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: Option<String> = Option::deserialize(deserializer)?;
    Ok(value.filter(|text| !text.trim().is_empty()))
}

/// As `empty_string_as_none`, for `[]`.
fn empty_vec_as_none<'de, D, T>(deserializer: D) -> std::result::Result<Option<Vec<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let value: Option<Vec<T>> = Option::deserialize(deserializer)?;
    Ok(value.filter(|items| !items.is_empty()))
}
