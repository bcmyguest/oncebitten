use std::fmt::Display;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

const RULE_FILE: &str = "rule.toml";
const CHECK_FILE: &str = "check.yml";
const FIXTURES_DIR: &str = "fixtures";
const GLOBS_ONLY_FOR_DIFF: &str = "when_changed and require_changed are only for engine = \"diff\"";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Block,
    Steer,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Block => "block",
            Mode::Steer => "steer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Proposed,
    Active,
    Retired,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Proposed => "proposed",
            Status::Active => "active",
            Status::Retired => "retired",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum Engine {
    #[serde(rename = "ast-grep")]
    AstGrep,
    #[serde(rename = "native")]
    Native,
    #[serde(rename = "diff")]
    Diff,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    description: String,
    mode: Mode,
    status: Status,
    engine: Engine,
    language: Option<String>,
    overrides: Option<String>,
    reason: Option<String>,
    retired_reason: Option<String>,
    command: Option<Vec<String>>,
    fired_exit_codes: Option<Vec<i32>>,
    fired_output_contains: Option<String>,
    when_changed: Option<Vec<String>>,
    require_changed: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub description: String,
    pub mode: Mode,
    pub status: Status,
    pub language: Option<String>,
    pub overrides: Option<Override>,
    pub check: Check,
    pub fired_exit_codes: Vec<i32>,
    pub fired_output_contains: Option<String>,
    pub file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub target: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    AstGrep {
        file: PathBuf,
    },
    Native {
        command: Vec<String>,
    },
    Diff {
        when_changed: Vec<String>,
        require_changed: Vec<String>,
    },
}

/// Whether a fixture file name is `stem` plus one extension, such as `bad.py`.
/// `bad.test.py` is not a `bad` fixture. Rule validation and replay share it.
pub fn is_fixture_named(file_name: &str, stem: &str) -> bool {
    Path::new(file_name).file_stem().and_then(|s| s.to_str()) == Some(stem)
}

pub fn is_kebab_case(name: &str) -> bool {
    !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

pub fn load_rule(dir: &Path) -> Result<Rule> {
    let file = dir.join(RULE_FILE);
    let text =
        std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
    let raw: RawRule =
        toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
    build_rule(dir, &file, raw)
}

/// The parts of a diff rule that replay and the checker evaluate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRule {
    pub id: String,
    pub when_changed: Vec<String>,
    pub require_changed: Vec<String>,
}

#[derive(Deserialize)]
struct DiffHead {
    id: String,
    engine: String,
    #[serde(default)]
    when_changed: Vec<String>,
    #[serde(default)]
    require_changed: Vec<String>,
}

/// Reads `file` as a diff rule. Returns `None` when it declares another engine.
/// Skips the directory-name and layout checks of `load_rule`, so a copy of
/// `rule.toml` in a sandbox directory reads the same.
pub fn read_diff_rule(file: &Path) -> Result<Option<DiffRule>> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let head: DiffHead =
        toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
    if head.engine != "diff" {
        return Ok(None);
    }
    Ok(Some(DiffRule {
        id: head.id,
        when_changed: head.when_changed,
        require_changed: head.require_changed,
    }))
}

fn invalid(file: &Path, message: impl Display) -> anyhow::Error {
    anyhow!("{}: {message}", file.display())
}

fn is_qualified(target: &str) -> bool {
    target
        .split_once('/')
        .is_some_and(|(source, id)| is_kebab_case(source) && is_kebab_case(id))
}

fn build_rule(dir: &Path, file: &Path, raw: RawRule) -> Result<Rule> {
    let dir_name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if raw.id != dir_name {
        return Err(invalid(
            file,
            format!("id `{}` must equal the directory name `{dir_name}`", raw.id),
        ));
    }
    if !is_kebab_case(&raw.id) {
        return Err(invalid(
            file,
            format!("id `{}` must be lowercase kebab-case", raw.id),
        ));
    }
    if raw.description.trim().is_empty() {
        return Err(invalid(file, "description is empty"));
    }
    if raw
        .retired_reason
        .as_ref()
        .is_some_and(|reason| reason.trim().is_empty())
    {
        return Err(invalid(file, "retired_reason is empty"));
    }
    let overrides = match (raw.overrides, raw.reason) {
        (None, None) => None,
        (None, Some(_)) => return Err(invalid(file, "reason needs overrides")),
        (Some(target), None) => {
            return Err(invalid(
                file,
                format!("overrides `{target}` needs a reason"),
            ))
        }
        (Some(target), Some(reason)) => {
            if !is_qualified(&target) {
                return Err(invalid(
                    file,
                    format!("overrides `{target}` must be <source>/<id>"),
                ));
            }
            if reason.trim().is_empty() {
                return Err(invalid(file, "reason is empty"));
            }
            Some(Override { target, reason })
        }
    };
    if raw.fired_exit_codes.is_some() && raw.engine != Engine::Native {
        return Err(invalid(
            file,
            "fired_exit_codes is only for engine = \"native\"",
        ));
    }
    let fired_exit_codes = raw.fired_exit_codes.unwrap_or_default();
    if fired_exit_codes.contains(&0) {
        return Err(invalid(file, "fired_exit_codes must not contain 0"));
    }
    if raw.fired_output_contains.is_some() && raw.engine != Engine::Native {
        return Err(invalid(
            file,
            "fired_output_contains is only for engine = \"native\"",
        ));
    }
    if raw.fired_output_contains.as_deref() == Some("") {
        return Err(invalid(file, "fired_output_contains is empty"));
    }
    if raw.fired_output_contains.is_some() && fired_exit_codes.is_empty() {
        return Err(invalid(
            file,
            "fired_output_contains needs fired_exit_codes",
        ));
    }
    let globs_given = raw.when_changed.is_some() || raw.require_changed.is_some();
    let check = match raw.engine {
        Engine::AstGrep => {
            if raw.command.is_some() {
                return Err(invalid(file, "command is only for engine = \"native\""));
            }
            if globs_given {
                return Err(invalid(file, GLOBS_ONLY_FOR_DIFF));
            }
            let check_file = dir.join(CHECK_FILE);
            if !check_file.is_file() {
                return Err(invalid(&check_file, "check file is missing"));
            }
            Check::AstGrep { file: check_file }
        }
        Engine::Native => {
            if globs_given {
                return Err(invalid(file, GLOBS_ONLY_FOR_DIFF));
            }
            match raw.command {
                Some(command) if !command.is_empty() => Check::Native { command },
                _ => {
                    return Err(invalid(
                        file,
                        "engine = \"native\" needs a non-empty command",
                    ))
                }
            }
        }
        Engine::Diff => {
            if raw.command.is_some() {
                return Err(invalid(file, "command is only for engine = \"native\""));
            }
            let when_changed = non_empty_globs(file, "when_changed", raw.when_changed)?;
            let require_changed = non_empty_globs(file, "require_changed", raw.require_changed)?;
            Check::Diff {
                when_changed,
                require_changed,
            }
        }
    };
    check_fixtures(&dir.join(FIXTURES_DIR))?;
    Ok(Rule {
        id: raw.id,
        description: raw.description,
        mode: raw.mode,
        status: raw.status,
        language: raw.language,
        overrides,
        check,
        fired_exit_codes,
        fired_output_contains: raw.fired_output_contains,
        file: file.to_path_buf(),
    })
}

fn non_empty_globs(file: &Path, key: &str, globs: Option<Vec<String>>) -> Result<Vec<String>> {
    match globs {
        Some(globs) if !globs.is_empty() => Ok(globs),
        _ => Err(invalid(
            file,
            format!("engine = \"diff\" needs a non-empty {key}"),
        )),
    }
}

fn check_fixtures(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Err(invalid(
            dir,
            "fixtures needs exactly one bad.* and one fixed.* file; the directory is missing",
        ));
    }
    let names = std::fs::read_dir(dir)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect::<std::io::Result<Vec<String>>>()
        })
        .with_context(|| format!("reading {}", dir.display()))?;
    let bad = names
        .iter()
        .filter(|name| is_fixture_named(name, "bad"))
        .count();
    let fixed = names
        .iter()
        .filter(|name| is_fixture_named(name, "fixed"))
        .count();
    if names.len() != 2 || bad != 1 || fixed != 1 {
        return Err(invalid(
            dir,
            format!(
                "fixtures needs exactly one bad.* and one fixed.* file; found [{}]",
                names.join(", ")
            ),
        ));
    }
    Ok(())
}
